//! Share a localhost server with other devices on the same Wi-Fi / LAN.
//! A server that already listens on every address (`*:5173`, `0.0.0.0`) is reachable as is: just hand out the LAN URL.
//! One bound to 127.0.0.1 only (vite, next dev… by default) gets a small TCP relay on 0.0.0.0:<public port> that pipes
//! each connection to localhost:<port>. Nothing leaves the local network; relays end when the app quits or the server stops.
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv6Addr, Shutdown, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// This Mac's address on the local network (the one a phone on the same Wi-Fi would use). No packet is sent:
/// "connecting" a UDP socket only asks the OS which interface it would route through.
pub fn lan_ip() -> Option<IpAddr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.168.0.1:9").or_else(|_| s.connect("10.0.0.1:9")).ok()?;
    let ip = s.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then_some(ip)
}

/// Public port for a relay: port + 10000 (easy to read: 5173 → 15173), else whatever the OS gives.
fn bind_public(port: u16) -> io::Result<TcpListener> {
    let nice = port.checked_add(10000).filter(|p| *p < 65535);
    nice.and_then(|p| TcpListener::bind(("0.0.0.0", p)).ok()).map(Ok).unwrap_or_else(|| TcpListener::bind(("0.0.0.0", 0)))
}

fn pipe(mut from: TcpStream, mut to: TcpStream) {
    let mut buf = [0u8; 16 * 1024];
    loop {
        match from.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if to.write_all(&buf[..n]).is_err() {
                    break;
                }
            }
        }
    }
    let _ = to.shutdown(Shutdown::Write);
}

fn local(port: u16) -> io::Result<TcpStream> {
    let t = Duration::from_secs(3);
    TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), t).or_else(|_| TcpStream::connect_timeout(&(Ipv6Addr::LOCALHOST, port).into(), t))
}

struct Relay {
    public: u16,
    stop: Arc<AtomicBool>,
}

#[derive(Default)]
pub struct Shares {
    relays: Mutex<HashMap<u16, Relay>>,
}

impl Shares {
    /// Start relaying localhost:`port` to the LAN (or return the running relay). → public port
    pub fn start(&self, port: u16) -> Result<u16, String> {
        let mut g = self.relays.lock().unwrap();
        if let Some(r) = g.get(&port) {
            return Ok(r.public);
        }
        local(port).map_err(|_| format!("nothing answers on localhost:{port}"))?;
        let l = bind_public(port).map_err(|e| format!("could not open a LAN port: {e}"))?;
        l.set_nonblocking(true).map_err(|e| e.to_string())?;
        let public = l.local_addr().map_err(|e| e.to_string())?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        std::thread::spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                match l.accept() {
                    Ok((client, _)) => {
                        let _ = client.set_nonblocking(false);
                        std::thread::spawn(move || {
                            let Ok(server) = local(port) else { return };
                            let (Ok(c2), Ok(s2)) = (client.try_clone(), server.try_clone()) else { return };
                            std::thread::spawn(move || pipe(c2, s2));
                            pipe(server, client);
                        });
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(80)),
                    Err(_) => std::thread::sleep(Duration::from_millis(200)),
                }
            }
        });
        g.insert(port, Relay { public, stop });
        Ok(public)
    }

    /// Close the relay (open connections finish on their own). Already-LAN servers have no relay to stop.
    pub fn stop(&self, port: u16) -> bool {
        match self.relays.lock().unwrap().remove(&port) {
            Some(r) => {
                r.stop.store(true, Ordering::Relaxed);
                true
            }
            None => false,
        }
    }

    pub fn public_port(&self, port: u16) -> Option<u16> {
        self.relays.lock().unwrap().get(&port).map(|r| r.public)
    }

    /// Drop relays whose server went away (it was stopped, or crashed).
    pub fn prune(&self, open: &[u16]) {
        let gone: Vec<u16> = self.relays.lock().unwrap().keys().copied().filter(|p| !open.contains(p)).collect();
        for p in gone {
            self.stop(p);
        }
    }

    pub fn relay_ports(&self) -> Vec<u16> {
        self.relays.lock().unwrap().values().map(|r| r.public).collect()
    }
}

/// `http://192.168.1.20:15173` — the link to paste on another device.
pub fn url(ip: Option<IpAddr>, port: u16) -> Option<String> {
    ip.map(|ip| match ip {
        IpAddr::V4(v) => format!("http://{v}:{port}"),
        IpAddr::V6(v) => format!("http://[{v}]:{port}"),
    })
}

pub fn is_public_addr(addr: &str) -> bool {
    // lsof names: `*:5173`, `0.0.0.0:5173`, `[::]:5173`, `192.168.1.20:5173` (public) vs `127.0.0.1:5173`, `[::1]:5173`, `localhost:…`
    let host = addr.rsplit_once(':').map(|(h, _)| h).unwrap_or(addr).trim_matches(|c| c == '[' || c == ']');
    !(host.starts_with("127.") || host == "::1" || host == "localhost" || host.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_public_from_loopback() {
        assert!(is_public_addr("*:5173"));
        assert!(is_public_addr("0.0.0.0:3000"));
        assert!(is_public_addr("[::]:8080"));
        assert!(is_public_addr("192.168.1.20:4321"));
        assert!(!is_public_addr("127.0.0.1:5173"));
        assert!(!is_public_addr("[::1]:5173"));
        assert!(!is_public_addr("localhost:1431"));
    }

    #[test]
    fn relays_a_loopback_server_to_every_address() {
        let srv = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = srv.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for s in srv.incoming().flatten().take(2) {
                let mut s = s;
                let mut b = [0u8; 64];
                let n = s.read(&mut b).unwrap_or(0);
                let _ = s.write_all(&b[..n]); // echo
            }
        });
        let shares = Shares::default();
        let public = shares.start(port).unwrap();
        assert_eq!(shares.start(port).unwrap(), public, "second start reuses the relay");
        assert_ne!(public, port);
        // the relay listens on 0.0.0.0, so it also answers on 127.0.0.1
        let mut c = TcpStream::connect(("127.0.0.1", public)).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        c.write_all(b"ping").unwrap();
        let mut b = [0u8; 4];
        c.read_exact(&mut b).unwrap();
        assert_eq!(&b, b"ping");
        shares.prune(&[]);
        assert_eq!(shares.public_port(port), None);
    }
}

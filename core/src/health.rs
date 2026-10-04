//! Is a listening port actually usable? Send `HEAD /` and read the status line.
//! healthy = HTTP < 500 · unhealthy = HTTP 5xx, connection refused or no reply · tcp = answers, but not HTTP (DB, socket…)
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_millis(700);
const CACHE: Duration = Duration::from_secs(4);

#[derive(Clone, Debug, PartialEq)]
pub struct Health {
    /// healthy | unhealthy | tcp
    pub state: &'static str,
    pub code: Option<u16>,
    pub ms: u64,
}

/// `HTTP/1.1 404 Not Found` → Some(404)
pub fn status_code(reply: &[u8]) -> Option<u16> {
    let s = std::str::from_utf8(reply.get(..reply.len().min(32))?).ok()?;
    let rest = s.strip_prefix("HTTP/")?;
    rest.split_whitespace().nth(1)?.parse().ok()
}

pub fn classify(code: Option<u16>, got_bytes: bool) -> &'static str {
    match code {
        Some(c) if c < 500 => "healthy",
        Some(_) => "unhealthy",
        None if got_bytes => "tcp",
        None => "unhealthy",
    }
}

pub fn probe(port: u16) -> Health {
    let t0 = Instant::now();
    let addrs: [SocketAddr; 2] = [([127, 0, 0, 1], port).into(), (std::net::Ipv6Addr::LOCALHOST, port).into()];
    let Some(mut s) = addrs.iter().find_map(|a| TcpStream::connect_timeout(a, TIMEOUT).ok()) else {
        return Health { state: "unhealthy", code: None, ms: t0.elapsed().as_millis() as u64 };
    };
    let _ = s.set_read_timeout(Some(TIMEOUT));
    let _ = s.set_write_timeout(Some(TIMEOUT));
    let _ = s.write_all(format!("HEAD / HTTP/1.1\r\nHost: localhost:{port}\r\nUser-Agent: ServerHub\r\nConnection: close\r\n\r\n").as_bytes());
    let mut buf = [0u8; 64];
    let n = s.read(&mut buf).unwrap_or(0);
    let code = status_code(&buf[..n]);
    // a silent socket that stays open (Postgres, Redis wait for their own protocol) is alive, just not a web page
    let state = if n == 0 && code.is_none() { "tcp" } else { classify(code, n > 0) };
    Health { state, code, ms: t0.elapsed().as_millis() as u64 }
}

/// Probe many ports at once, reuse answers younger than 4 s so polling every 2.5 s stays cheap.
pub fn check(ports: &[u16]) -> HashMap<u16, Health> {
    static MEMO: Mutex<Option<HashMap<u16, (Instant, Health)>>> = Mutex::new(None);
    let mut out = HashMap::new();
    let mut todo = vec![];
    {
        let g = MEMO.lock().unwrap();
        for &p in ports {
            match g.as_ref().and_then(|m| m.get(&p)).filter(|(t, _)| t.elapsed() < CACHE) {
                Some((_, h)) => {
                    out.insert(p, h.clone());
                }
                None => todo.push(p),
            }
        }
    }
    let fresh: Vec<(u16, Health)> = std::thread::scope(|sc| {
        let hs: Vec<_> = todo.iter().map(|&p| sc.spawn(move || (p, probe(p)))).collect();
        hs.into_iter().filter_map(|h| h.join().ok()).collect()
    });
    let mut g = MEMO.lock().unwrap();
    let m = g.get_or_insert_with(HashMap::new);
    m.retain(|_, (t, _)| t.elapsed() < Duration::from_secs(60));
    for (p, h) in fresh {
        m.insert(p, (Instant::now(), h.clone()));
        out.insert(p, h);
    }
    out
}

/// What kind of server this is, from its command line: vite, next, astro, http.server…
pub fn tool(exe: &str, args: &str) -> String {
    let a = args.to_lowercase();
    let known = [
        ("vite", "vite"), ("next", "next"), ("astro", "astro"), ("nuxt", "nuxt"), ("webpack", "webpack"), ("remix", "remix"),
        ("svelte-kit", "sveltekit"), ("storybook", "storybook"), ("expo", "expo"), ("metro", "metro"), ("parcel", "parcel"),
        ("http.server", "http.server"), ("uvicorn", "uvicorn"), ("gunicorn", "gunicorn"), ("flask", "flask"), ("manage.py", "django"),
        ("rails", "rails"), ("jekyll", "jekyll"), ("hugo", "hugo"), ("tsx ", "tsx"), ("ng ", "angular"), ("ng serve", "angular"), ("nodemon", "nodemon"), ("wrangler", "wrangler"),
    ];
    for (k, name) in known {
        if a.contains(&format!("/{k}")) || a.contains(&format!(" {k}")) || a.starts_with(k) {
            return name.into();
        }
    }
    let first = exe.split_whitespace().next().unwrap_or(exe);
    let base = first.rsplit('/').next().unwrap_or(first);
    base.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.').to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_status_lines() {
        assert_eq!(status_code(b"HTTP/1.1 200 OK\r\n"), Some(200));
        assert_eq!(status_code(b"HTTP/1.0 503 Service Unavailable"), Some(503));
        assert_eq!(status_code(b"\x00\x00\x00 garbage"), None);
        assert_eq!(classify(Some(404), true), "healthy");
        assert_eq!(classify(Some(502), true), "unhealthy");
        assert_eq!(classify(None, true), "tcp");
    }

    #[test]
    fn names_the_tool() {
        assert_eq!(tool("node", "node /x/node_modules/.bin/vite --port 5173"), "vite");
        assert_eq!(tool("python3.12", "python3 -m http.server 8765"), "http.server");
        assert_eq!(tool("/opt/homebrew/bin/python3.12", "python3.12 app.py"), "python");
    }

    #[test]
    fn probes_a_real_server_and_a_closed_port() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = l.accept() {
                let mut b = [0u8; 256];
                let _ = s.read(&mut b);
                let _ = s.write_all(b"HTTP/1.1 500 Internal Server Error\r\n\r\n");
            }
        });
        let h = probe(port);
        assert_eq!((h.state, h.code), ("unhealthy", Some(500)));
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert_eq!(probe(closed).state, "unhealthy");
    }
}

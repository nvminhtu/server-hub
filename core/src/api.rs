//! One entry point for the UI: `App::call(cmd, args) -> JSON`. The Tauri shell exposes it as the `api` command,
//! `server-hub bridge` exposes it over HTTP (POST /api/<cmd>) so the same UI runs in a browser.
use crate::health;
use crate::procs::{self, Listener, Proc};
use crate::scan::{self, Custom, Entry};
use crate::share::{self, Shares};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{BufRead, BufReader, Read};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const LOG_LINES: usize = 2000;
const RESCAN: Duration = Duration::from_secs(30);

/// A server this app started: its own process group, so Stop takes npm + vite + esbuild down together.
struct Run {
    pgid: i32,
    started: Instant,
    logs: VecDeque<String>,
    exit: Option<i32>,
    stopping: bool,
}

pub struct App {
    roots: Mutex<Vec<PathBuf>>,
    custom_file: PathBuf,
    settings_file: PathBuf,
    entries: Mutex<(Option<Instant>, Vec<Entry>)>,
    runs: Arc<Mutex<HashMap<String, Run>>>,
    shares: Shares,
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\x1b' {
            if it.peek() == Some(&'[') {
                it.next();
                for d in it.by_ref() {
                    if d.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        if c != '\r' {
            out.push(c);
        }
    }
    out
}

/// The PATH a terminal would have (Homebrew, nvm, fvm…). Apps opened from Finder only get /usr/bin:/bin.
fn login_path() -> &'static str {
    static P: OnceLock<String> = OnceLock::new();
    P.get_or_init(|| {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
        let mut child = Command::new(&shell)
            .args(["-ilc", "printf '__PATH__%s' \"$PATH\""])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok();
        let mut found = None;
        if let Some(c) = child.as_mut() {
            let t0 = Instant::now();
            while t0.elapsed() < Duration::from_secs(5) {
                if let Ok(Some(_)) = c.try_wait() {
                    let mut s = String::new();
                    let _ = c.stdout.take().map(|mut o| o.read_to_string(&mut s));
                    found = s.rsplit_once("__PATH__").map(|(_, p)| p.trim().to_string());
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            let _ = c.kill();
        }
        found.filter(|p| !p.is_empty()).unwrap_or_else(|| {
            format!("/opt/homebrew/bin:/usr/local/bin:{}", std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into()))
        })
    })
}

fn arg<'a>(args: &'a Value, k: &str) -> Result<&'a str, String> {
    args[k].as_str().filter(|s| !s.is_empty()).ok_or_else(|| format!("missing `{k}`"))
}

fn port_arg(args: &Value) -> Result<u16, String> {
    args["port"].as_u64().and_then(|p| u16::try_from(p).ok()).filter(|p| *p > 0).ok_or_else(|| "missing `port`".into())
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
}

/// Where projects live when nobody chose a folder: $SERVER_HUB_ROOT, else the first usual dev folder that exists.
pub fn default_root() -> PathBuf {
    if let Ok(r) = std::env::var("SERVER_HUB_ROOT") {
        return PathBuf::from(r);
    }
    let h = home();
    ["DATA/PROJECTS", "Developer", "Projects", "projects", "code", "Code", "dev", "src", "GitHub", "Documents/GitHub", "Sites"]
        .iter()
        .map(|d| h.join(d))
        .find(|p| p.is_dir())
        .unwrap_or_else(|| h.join("Developer"))
}

impl App {
    /// `root` (CLI / tests) wins; else the folders saved in settings.json; else default_root().
    pub fn new(root: Option<PathBuf>) -> Self {
        let dir = home().join("Library/Application Support/server-hub");
        let settings_file = dir.join("settings.json");
        let saved: Vec<PathBuf> = std::fs::read_to_string(&settings_file)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .map(|v| {
                let mut l: Vec<PathBuf> = v["roots"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(PathBuf::from)).collect()).unwrap_or_default();
                if let Some(old) = v["root"].as_str() {
                    l.push(PathBuf::from(old)); // settings from 0.1.0 held a single folder
                }
                l
            })
            .unwrap_or_default();
        let roots = match root {
            Some(r) => vec![r],
            None if saved.is_empty() => vec![default_root()],
            None => saved,
        };
        App { roots: Mutex::new(roots), custom_file: dir.join("custom.json"), settings_file, entries: Mutex::new((None, vec![])), runs: Arc::new(Mutex::new(HashMap::new())), shares: Shares::default() }
    }

    pub fn roots(&self) -> Vec<PathBuf> {
        self.roots.lock().unwrap().clone()
    }

    fn save_roots(&self, roots: Vec<PathBuf>) -> Result<(), String> {
        if let Some(d) = self.settings_file.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let list: Vec<String> = roots.iter().map(|p| p.to_string_lossy().to_string()).collect();
        std::fs::write(&self.settings_file, json!({ "roots": list }).to_string()).map_err(|e| e.to_string())?;
        *self.roots.lock().unwrap() = roots;
        self.entries.lock().unwrap().0 = None;
        Ok(())
    }

    /// Add a projects folder with the macOS folder dialog (or take `path`), remember it, rescan.
    fn add_root(&self, path: Option<&str>) -> Result<Value, String> {
        let chosen = match path {
            Some(p) => p.replacen('~', &home().to_string_lossy(), 1),
            None => {
                let script = format!(
                    "POSIX path of (choose folder with prompt \"Add a folder that holds your projects\" default location POSIX file \"{}\")",
                    home().display()
                );
                let o = Command::new("osascript").args(["-e", &script]).output().map_err(|e| e.to_string())?;
                if !o.status.success() {
                    return Ok(json!({ "ok": false })); // cancelled
                }
                String::from_utf8_lossy(&o.stdout).trim().trim_end_matches('/').to_string()
            }
        };
        let p = PathBuf::from(&chosen);
        if !p.is_dir() {
            return Err(format!("folder not found: {chosen}"));
        }
        // a missing default folder (nobody chose anything yet) gives way to the first real choice
        let mut roots: Vec<PathBuf> = self.roots().into_iter().filter(|r| r.is_dir()).collect();
        if roots.contains(&p) {
            return Err(format!("{} is already in the list", scan::short(&chosen)));
        }
        roots.push(p);
        self.save_roots(roots)?;
        Ok(json!({ "ok": true, "root": scan::short(&chosen) }))
    }

    fn remove_root(&self, path: &str) -> Result<Value, String> {
        let full = PathBuf::from(path.replacen('~', &home().to_string_lossy(), 1));
        let roots: Vec<PathBuf> = self.roots().into_iter().filter(|r| *r != full).collect();
        self.save_roots(roots)?;
        Ok(json!({ "ok": true }))
    }

    pub fn call(&self, cmd: &str, args: &Value) -> Result<Value, String> {
        match cmd {
            "snapshot" => Ok(self.snapshot(args["rescan"].as_bool().unwrap_or(false))),
            "start" => self.start(arg(args, "id")?),
            "stop" => self.stop(arg(args, "id")?),
            "kill" => self.kill(args["pid"].as_i64().ok_or("missing `pid`")? as i32),
            "logs" => Ok(self.logs(arg(args, "id")?)),
            "open" => open(arg(args, "target")?, args["app"].as_str()),
            "add" => self.add(args),
            "remove" => self.remove(arg(args, "name")?),
            "addRoot" | "setRoot" => self.add_root(args["path"].as_str().filter(|s| !s.is_empty())),
            "removeRoot" => self.remove_root(arg(args, "path")?),
            "share" => self.share(port_arg(args)?),
            "unshare" => Ok(json!({ "ok": self.shares.stop(port_arg(args)?) })),
            _ => Err(format!("unknown command `{cmd}`")),
        }
    }

    fn customs(&self) -> Vec<Custom> {
        std::fs::read_to_string(&self.custom_file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    fn save_customs(&self, list: &[Custom]) -> Result<(), String> {
        if let Some(d) = self.custom_file.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        std::fs::write(&self.custom_file, serde_json::to_string_pretty(list).unwrap()).map_err(|e| e.to_string())?;
        self.entries.lock().unwrap().0 = None;
        Ok(())
    }

    fn add(&self, a: &Value) -> Result<Value, String> {
        let name = arg(a, "name")?.trim().to_string();
        let cwd = arg(a, "cwd")?.trim().replacen('~', &std::env::var("HOME").unwrap_or_default(), 1);
        let command = arg(a, "command")?.trim().to_string();
        if !std::path::Path::new(&cwd).is_dir() {
            return Err(format!("folder not found: {cwd}"));
        }
        let port = a["port"].as_u64().map(|p| p as u16).or_else(|| a["port"].as_str().and_then(|s| s.trim().parse().ok()));
        let mut list = self.customs();
        if list.iter().any(|c| c.name == name) {
            return Err(format!("`{name}` already exists"));
        }
        list.push(Custom { name, cwd, command, port });
        self.save_customs(&list)?;
        Ok(json!({ "ok": true }))
    }

    fn remove(&self, name: &str) -> Result<Value, String> {
        let mut list = self.customs();
        list.retain(|c| c.name != name);
        self.save_customs(&list)?;
        Ok(json!({ "ok": true }))
    }

    fn entries(&self, rescan: bool) -> Vec<Entry> {
        let mut g = self.entries.lock().unwrap();
        if rescan || g.0.is_none_or(|t| t.elapsed() > RESCAN) {
            let custom = scan::custom_entries(&self.customs(), &self.custom_file);
            g.1 = scan::scan_all(&self.roots(), &custom);
            g.0 = Some(Instant::now());
        }
        g.1.clone()
    }

    fn entry(&self, id: &str) -> Result<Entry, String> {
        self.entries(false).into_iter().find(|e| e.id == id).ok_or_else(|| "this item is gone — rescan".to_string())
    }

    fn start(&self, id: &str) -> Result<Value, String> {
        let e = self.entry(id)?;
        let Some(program) = e.program.clone() else { return Err("attach-only item: nothing to start, open its URL".into()) };
        {
            let runs = self.runs.lock().unwrap();
            if runs.get(id).is_some_and(|r| r.exit.is_none()) {
                return Err("already running".into());
            }
        }
        if let Some(p) = e.port {
            if let Some(l) = procs::listeners().into_iter().find(|l| l.ports.contains(&p)) {
                return Err(format!("port {p} is already in use by pid {} — stop that first", l.pid));
            }
        }
        let mut child = Command::new(&program)
            .args(&e.args)
            .current_dir(&e.cwd)
            .env("PATH", login_path())
            .env("FORCE_COLOR", "0")
            .env("BROWSER", "none")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .map_err(|err| format!("could not start `{}`: {err}", e.command()))?;
        let pid = child.id() as i32;
        let mut logs = VecDeque::new();
        logs.push_back(format!("$ {}   (in {})", e.command(), scan::short(&e.cwd)));
        self.runs.lock().unwrap().insert(id.to_string(), Run { pgid: pid, started: Instant::now(), logs, exit: None, stopping: false });

        for out in [child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>), child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>)].into_iter().flatten() {
            let runs = self.runs.clone();
            let id = id.to_string();
            std::thread::spawn(move || {
                for line in BufReader::new(out).lines().map_while(Result::ok) {
                    let mut g = runs.lock().unwrap();
                    if let Some(r) = g.get_mut(&id).filter(|r| r.pgid == pid) {
                        r.logs.push_back(strip_ansi(&line));
                        while r.logs.len() > LOG_LINES {
                            r.logs.pop_front();
                        }
                    }
                }
            });
        }
        let runs = self.runs.clone();
        let id2 = id.to_string();
        std::thread::spawn(move || {
            let code = child.wait().map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);
            std::thread::sleep(Duration::from_millis(200)); // let the readers flush the last lines
            if let Some(r) = runs.lock().unwrap().get_mut(&id2).filter(|r| r.pgid == pid) {
                r.exit = Some(code);
                r.logs.push_back(format!("— exited with code {code}"));
            }
        });
        Ok(json!({ "ok": true, "pid": pid }))
    }

    fn stop(&self, id: &str) -> Result<Value, String> {
        {
            let mut runs = self.runs.lock().unwrap();
            if let Some(r) = runs.get_mut(id).filter(|r| r.exit.is_none()) {
                r.stopping = true;
                r.logs.push_back("— stopping…".into());
                procs::terminate(r.pgid, true);
                return Ok(json!({ "ok": true }));
            }
        }
        // started somewhere else (Terminal, Claude preview…): stop the process that holds its port
        let snap = self.snapshot(false);
        let row = snap["entries"].as_array().and_then(|a| a.iter().find(|r| r["id"] == id)).cloned().unwrap_or_default();
        match row["pid"].as_i64() {
            Some(pid) => self.kill(pid as i32),
            None => Err("not running".into()),
        }
    }

    fn kill(&self, pid: i32) -> Result<Value, String> {
        let table = procs::ps();
        let p = table.get(&pid).ok_or("process already gone")?;
        let cwd = procs::listeners().into_iter().find(|l| l.pid == pid).map(|l| l.cwd).unwrap_or_default();
        if pid == std::process::id() as i32 || procs::is_system(&p.exe, if cwd.is_empty() { "/" } else { &cwd }) {
            return Err("refusing to stop a system process".into());
        }
        procs::terminate(pid, false);
        Ok(json!({ "ok": true }))
    }

    /// Make localhost:`port` reachable from other devices on the LAN → `{ url }` to paste on a phone / another laptop.
    fn share(&self, port: u16) -> Result<Value, String> {
        let ip = share::lan_ip().ok_or("this Mac is not on a local network (Wi-Fi / Ethernet)")?;
        let l = procs::listeners().into_iter().find(|l| l.ports.contains(&port)).ok_or(format!("nothing listens on :{port}"))?;
        // already listening on every address: the port itself is the LAN link, no relay needed
        let public = if l.public.contains(&port) { port } else { self.shares.start(port)? };
        Ok(json!({ "ok": true, "url": share::url(Some(ip), public), "relay": public != port }))
    }

    fn logs(&self, id: &str) -> Value {
        match self.runs.lock().unwrap().get(id) {
            Some(r) => json!({ "lines": r.logs.iter().collect::<Vec<_>>(), "own": true }),
            None => json!({ "lines": [], "own": false }),
        }
    }

    pub fn snapshot(&self, rescan: bool) -> Value {
        let entries = self.entries(rescan);
        let table = procs::ps();
        let me = std::process::id() as i32;
        let relays = self.shares.relay_ports();
        let mut listeners = procs::listeners();
        for l in listeners.iter_mut().filter(|l| l.pid == me) {
            l.ports.retain(|p| !relays.contains(p)); // our own LAN relays are not servers
        }
        listeners.retain(|l| !l.ports.is_empty());
        let open: Vec<u16> = listeners.iter().flat_map(|l| l.ports.iter().copied()).collect();
        self.shares.prune(&open);
        let ip = share::lan_ip();
        let runs = self.runs.lock().unwrap();
        let mut rows = match_rows(&entries, &listeners, &table, &runs);
        let row_of: HashMap<i32, usize> = rows.iter().enumerate().filter_map(|(i, r)| r["pid"].as_i64().map(|p| (p as i32, i))).collect();
        // every port someone could open in a browser: dev servers, project rows, forgotten ones — not macOS or normal apps
        let shown: Vec<&Listener> = listeners
            .iter()
            .filter(|l| l.pid != me && (row_of.contains_key(&l.pid) || table.get(&l.pid).is_some_and(|p| !procs::is_system(&p.exe, &l.cwd))))
            .collect();
        let probe: Vec<u16> = shown.iter().flat_map(|l| l.ports.iter().copied()).collect();
        let health = health::check(&probe);
        let mut ports: Vec<Value> = vec![];
        for l in &shown {
            let p = table.get(&l.pid);
            let exe = p.map(|p| p.exe.as_str()).unwrap_or("");
            let row = row_of.get(&l.pid).map(|&i| &rows[i]);
            let folder = std::path::Path::new(&l.cwd).file_name().map(|n| n.to_string_lossy().to_string()).filter(|_| l.cwd != "/" && PathBuf::from(&l.cwd) != home());
            let name = row.and_then(|r| r["name"].as_str().map(|n| n.rsplit('/').next().unwrap_or(n).to_string())).or(folder).unwrap_or_else(|| exe.rsplit('/').next().unwrap_or(exe).to_string());
            for port in &l.ports {
                let h = health.get(port);
                ports.push(json!({
                    "port": port,
                    "pid": l.pid,
                    "name": name,
                    "tool": health::tool(exe, &l.args),
                    "cwd": scan::short(&l.cwd),
                    "args": scan::short(&l.args),
                    "mb": p.map(|p| p.rss_kb / 1024).unwrap_or(0),
                    "secs": p.map(|p| p.secs).unwrap_or(0),
                    "source": if row.is_some_and(|r| r["own"] == true) { "Server Hub".to_string() } else { procs::source(l.pid, &table) },
                    "health": h.map(|h| h.state).unwrap_or("unhealthy"),
                    "code": h.and_then(|h| h.code),
                    "ms": h.map(|h| h.ms),
                    "rowId": row.map(|r| r["id"].clone()),
                    // LAN: "open" = already reachable from other devices, "shared" = through our relay, null = this Mac only
                    "lan": if l.public.contains(port) { Some("open") } else if self.shares.public_port(*port).is_some() { Some("shared") } else { None },
                    "lanUrl": if l.public.contains(port) { share::url(ip, *port) } else { self.shares.public_port(*port).and_then(|p| share::url(ip, p)) },
                }));
            }
        }
        // SERVER_HUB_ONLY_PORTS=3000,5173 — show just these (clean store screenshots on a busy machine)
        if let Ok(only) = std::env::var("SERVER_HUB_ONLY_PORTS") {
            let keep: HashSet<u64> = only.split(',').filter_map(|p| p.trim().parse().ok()).collect();
            ports.retain(|p| p["port"].as_u64().is_some_and(|x| keep.contains(&x)));
        }
        ports.sort_by_key(|p| p["port"].as_u64());
        for r in rows.iter_mut() {
            let first = r["ports"].as_array().and_then(|a| a.first()).and_then(|p| p.as_u64()).map(|p| p as u16);
            r["health"] = json!(first.and_then(|p| health.get(&p)).map(|h| h.state));
        }
        let claimed: HashSet<i32> = rows.iter().filter_map(|r| r["pid"].as_i64().map(|p| p as i32)).collect();
        let own_groups: HashSet<i32> = runs.values().filter(|r| r.exit.is_none()).map(|r| r.pgid).collect();
        let outside: Vec<Value> = listeners
            .iter()
            .filter(|l| !claimed.contains(&l.pid) && l.pid != me)
            .filter_map(|l| {
                let p = table.get(&l.pid)?;
                if own_groups.contains(&p.pgid) || procs::is_system(&p.exe, &l.cwd) {
                    return None;
                }
                Some(json!({
                    "pid": l.pid,
                    "name": p.exe.rsplit('/').next().unwrap_or(&p.exe),
                    "cwd": scan::short(&l.cwd),
                    "args": scan::short(&l.args),
                    "ports": l.ports,
                    "mb": p.rss_kb / 1024,
                    "secs": p.secs,
                    "source": procs::source(l.pid, &table),
                }))
            })
            .collect();
        let running = rows.iter().filter(|r| r["status"] == "running" || r["status"] == "starting").count();
        let unhealthy = ports.iter().filter(|p| p["health"] == "unhealthy").count();
        let roots = self.roots();
        json!({
            "root": roots.first().map(|r| scan::short(&r.to_string_lossy())).unwrap_or_default(),
            "roots": roots.iter().map(|r| json!({ "path": scan::short(&r.to_string_lossy()), "exists": r.is_dir() })).collect::<Vec<_>>(),
            "rootExists": roots.iter().any(|r| r.is_dir()),
            "entries": rows,
            "outside": outside,
            "ports": ports,
            "unhealthy": unhealthy,
            "lanIp": ip.map(|i| i.to_string()),
            "running": running,
        })
    }
}

/// Decide, for every row, whether it runs and which process is it. One listener belongs to at most one row.
fn match_rows(entries: &[Entry], listeners: &[Listener], table: &HashMap<i32, Proc>, runs: &HashMap<String, Run>) -> Vec<Value> {
    let mut owner: HashMap<i32, usize> = HashMap::new(); // listener pid → row index
    // 1) rows started by this app: listeners inside its process group
    for (i, e) in entries.iter().enumerate() {
        if let Some(r) = runs.get(&e.id).filter(|r| r.exit.is_none()) {
            for l in listeners {
                if table.get(&l.pid).is_some_and(|p| p.pgid == r.pgid) {
                    owner.insert(l.pid, i);
                }
            }
        }
    }
    // 2) declared port, or same folder for rows without a port; best scoring row wins a shared port
    for l in listeners {
        if owner.contains_key(&l.pid) {
            continue;
        }
        let mut best: Option<(i32, usize)> = None;
        for (i, e) in entries.iter().enumerate() {
            if owner.values().any(|&o| o == i) && runs.get(&e.id).is_some_and(|r| r.exit.is_none()) {
                continue;
            }
            let by_port = e.port.is_some_and(|p| l.ports.contains(&p));
            let by_dir = e.port.is_none() && !l.cwd.is_empty() && l.cwd == e.cwd && e.kind != "launch";
            if !by_port && !by_dir {
                continue;
            }
            let mut score = 1;
            if !l.cwd.is_empty() && (l.cwd == e.cwd || e.args.iter().any(|a| a == &l.cwd)) {
                score += 2;
            }
            score += e.args.iter().filter(|a| a.len() > 6 && l.args.contains(a.as_str())).count() as i32;
            if best.is_none_or(|(s, _)| score > s) {
                best = Some((score, i));
            }
        }
        if let Some((_, i)) = best {
            owner.insert(l.pid, i);
        }
    }

    let mut port_users: HashMap<u16, Vec<&str>> = HashMap::new();
    for e in entries {
        if let Some(p) = e.port {
            port_users.entry(p).or_default().push(&e.name);
        }
    }
    let by_port: HashMap<u16, i32> = listeners.iter().flat_map(|l| l.ports.iter().map(move |p| (*p, l.pid))).collect();

    entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let mine: Vec<&Listener> = listeners.iter().filter(|l| owner.get(&l.pid) == Some(&i)).collect();
            let run = runs.get(&e.id);
            let own_alive = run.is_some_and(|r| r.exit.is_none());
            let main = mine.first().map(|l| l.pid);
            let ports: Vec<u16> = mine.iter().flat_map(|l| l.ports.iter().copied()).collect();
            let (status, note) = if own_alive && (main.is_some() || e.port.is_none()) {
                ("running", String::new())
            } else if own_alive {
                ("starting", String::new())
            } else if main.is_some() {
                ("running", String::new())
            } else if let Some(r) = run.filter(|r| r.exit.is_some_and(|c| c != 0) && !r.stopping) {
                ("error", r.logs.iter().rev().find(|l| !l.starts_with('—') && !l.trim().is_empty()).cloned().unwrap_or_default())
            } else if let Some(pid) = e.port.and_then(|p| by_port.get(&p)) {
                let who = owner.get(pid).map(|&o| entries[o].name.clone()).unwrap_or_else(|| {
                    table.get(pid).map(|p| p.exe.rsplit('/').next().unwrap_or("").to_string()).unwrap_or_default()
                });
                ("blocked", format!("port {} in use by {who}", e.port.unwrap()))
            } else {
                ("stopped", String::new())
            };
            let pid = main.or(if own_alive { run.map(|r| r.pgid) } else { None });
            let proc_ = pid.and_then(|p| table.get(&p));
            let shared: Vec<&str> = e.port.and_then(|p| port_users.get(&p)).map(|v| v.iter().copied().filter(|n| *n != e.name).collect()).unwrap_or_default();
            let mut url = e.url.clone();
            if url.is_none() {
                url = ports.first().or(e.port.as_ref()).filter(|_| e.port.is_some() || !ports.is_empty()).map(|p| format!("http://localhost:{p}"));
            }
            json!({
                "id": e.id,
                "repo": e.repo,
                "group": e.group,
                "project": e.project,
                "name": e.name,
                "kind": e.kind,
                "cwd": scan::short(&e.cwd),
                "cwdFull": e.cwd,
                "command": e.command(),
                "canStart": e.program.is_some(),
                "port": e.port,
                "ports": ports,
                "url": url,
                "status": status,
                "note": note,
                "own": own_alive,
                "pid": pid,
                "mb": proc_.map(|p| p.rss_kb / 1024),
                "secs": if own_alive { run.map(|r| r.started.elapsed().as_secs()) } else { proc_.map(|p| p.secs) },
                "source": if own_alive { "Server Hub".to_string() } else { pid.map(|p| procs::source(p, table)).unwrap_or_default() },
                "sharedPort": shared,
                "file": scan::short(&e.file),
            })
        })
        .collect()
}

/// Open a URL in the browser, or a folder in Finder / an editor.
pub fn open(target: &str, app: Option<&str>) -> Result<Value, String> {
    let target = target.replacen('~', &std::env::var("HOME").unwrap_or_default(), 1);
    let mut c = Command::new("open");
    if let Some(a) = app.filter(|a| !a.is_empty()) {
        c.args(["-a", a]);
    }
    let ok = c.arg(&target).status().map(|s| s.success()).unwrap_or(false);
    if ok { Ok(json!({ "ok": true })) } else { Err(format!("could not open {target}")) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn entry(name: &str, kind: &str, cwd: &str, args: &[&str], port: Option<u16>) -> Entry {
        Entry {
            id: name.into(),
            repo: "R".into(),
            name: name.into(),
            kind: kind.into(),
            cwd: cwd.into(),
            program: Some("x".into()),
            args: args.iter().map(|s| s.to_string()).collect(),
            port,
            url: None,
            file: String::new(),
            group: String::new(),
            project: String::new(),
        }
    }

    #[test]
    fn shared_port_goes_to_the_row_whose_args_match() {
        let es = vec![
            entry("relay-mockup", "launch", "/a", &["-m", "http.server", "8765", "--directory", "/x/master/mockup"], Some(8765)),
            entry("shotmatic-mockup", "launch", "/b", &["-m", "http.server", "8765", "--directory", "/x/shotmatic/mockup"], Some(8765)),
        ];
        let ls = vec![Listener { pid: 50, ports: BTreeSet::from([8765]), public: BTreeSet::new(), cwd: "/b".into(), args: "python3 -m http.server 8765 --directory /x/shotmatic/mockup".into() }];
        let mut t = HashMap::new();
        t.insert(50, Proc { pid: 50, ppid: 1, pgid: 50, rss_kb: 2048, secs: 9, exe: "python3".into() });
        let rows = match_rows(&es, &ls, &t, &HashMap::new());
        assert_eq!(rows[1]["status"], "running");
        assert_eq!(rows[0]["status"], "blocked");
        assert_eq!(rows[0]["note"], "port 8765 in use by shotmatic-mockup");
        assert_eq!(rows[0]["sharedPort"], json!(["shotmatic-mockup"]));
    }

    #[test]
    fn start_logs_and_stop_a_real_process() {
        let t = tempfile::tempdir().unwrap();
        let app = App::new(Some(t.path().to_path_buf()));
        app.entries.lock().unwrap().0 = Some(Instant::now());
        let mut e = entry("sleeper", "custom", &t.path().to_string_lossy(), &["-c", "echo hello; exec sleep 30"], None);
        e.program = Some("/bin/sh".into());
        app.entries.lock().unwrap().1 = vec![e];
        app.call("start", &json!({ "id": "sleeper" })).unwrap();
        std::thread::sleep(Duration::from_millis(400));
        let snap = app.snapshot(false);
        assert_eq!(snap["entries"][0]["status"], "running");
        assert!(app.logs("sleeper")["lines"].as_array().unwrap().iter().any(|l| l == "hello"));
        app.call("stop", &json!({ "id": "sleeper" })).unwrap();
        std::thread::sleep(Duration::from_millis(600));
        assert_eq!(app.snapshot(false)["entries"][0]["status"], "stopped");
    }
}

//! What is running right now: listening TCP ports (`lsof`), the process table (`ps`), each listener's folder,
//! and who started it (walk up the parent chain: Terminal, VS Code, Claude Code, Codex…).
use std::collections::{BTreeSet, HashMap};
use std::process::Command;

#[derive(Clone, Debug, Default)]
pub struct Proc {
    pub pid: i32,
    pub ppid: i32,
    pub pgid: i32,
    pub rss_kb: u64,
    pub secs: u64,
    pub exe: String,
}

#[derive(Clone, Debug, Default)]
pub struct Listener {
    pub pid: i32,
    pub ports: BTreeSet<u16>,
    /// ports bound to every address (`*:5173`) — other devices on the LAN can already open them
    pub public: BTreeSet<u16>,
    pub cwd: String,
    pub args: String,
}

fn run(cmd: &str, args: &[&str]) -> String {
    Command::new(cmd).args(args).output().map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default()
}

/// `[[dd-]hh:]mm:ss` → seconds
pub fn etime(s: &str) -> u64 {
    let (days, rest) = match s.split_once('-') {
        Some((d, r)) => (d.parse::<u64>().unwrap_or(0), r),
        None => (0, s),
    };
    let parts: Vec<u64> = rest.split(':').map(|x| x.parse().unwrap_or(0)).collect();
    let hms = match parts.as_slice() {
        [h, m, s] => h * 3600 + m * 60 + s,
        [m, s] => m * 60 + s,
        [s] => *s,
        _ => 0,
    };
    days * 86400 + hms
}

pub fn parse_ps(text: &str) -> HashMap<i32, Proc> {
    let mut out = HashMap::new();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(pid), Some(ppid), Some(pgid), Some(rss), Some(et)) = (it.next(), it.next(), it.next(), it.next(), it.next()) else { continue };
        let Ok(pid) = pid.parse::<i32>() else { continue };
        let exe = it.collect::<Vec<_>>().join(" ");
        out.insert(pid, Proc { pid, ppid: ppid.parse().unwrap_or(0), pgid: pgid.parse().unwrap_or(0), rss_kb: rss.parse().unwrap_or(0), secs: etime(et), exe });
    }
    out
}

pub fn ps() -> HashMap<i32, Proc> {
    parse_ps(&run("ps", &["-axo", "pid=,ppid=,pgid=,rss=,etime=,comm="]))
}

/// `lsof -F pn` output → pid → ports
pub fn parse_lsof_listen(text: &str) -> HashMap<i32, BTreeSet<u16>> {
    let mut out: HashMap<i32, BTreeSet<u16>> = HashMap::new();
    let mut pid = 0;
    for line in text.lines() {
        if let Some(p) = line.strip_prefix('p') {
            pid = p.parse().unwrap_or(0);
        } else if let Some(n) = line.strip_prefix('n') {
            if let Some((_, port)) = n.rsplit_once(':') {
                if let Ok(port) = port.parse::<u16>() {
                    out.entry(pid).or_default().insert(port);
                }
            }
        }
    }
    out
}

/// `lsof -F pn` output → pid → ports listening on a non-loopback address
pub fn parse_lsof_public(text: &str) -> HashMap<i32, BTreeSet<u16>> {
    let mut out: HashMap<i32, BTreeSet<u16>> = HashMap::new();
    let mut pid = 0;
    for line in text.lines() {
        if let Some(p) = line.strip_prefix('p') {
            pid = p.parse().unwrap_or(0);
        } else if let Some(n) = line.strip_prefix('n') {
            if let Some((_, port)) = n.rsplit_once(':') {
                if let (Ok(port), true) = (port.parse::<u16>(), crate::share::is_public_addr(n)) {
                    out.entry(pid).or_default().insert(port);
                }
            }
        }
    }
    out
}

pub fn listeners() -> Vec<Listener> {
    let raw = run("lsof", &["-nP", "-iTCP", "-sTCP:LISTEN", "-Fpn"]);
    let ports = parse_lsof_listen(&raw);
    let public = parse_lsof_public(&raw);
    if ports.is_empty() {
        return vec![];
    }
    let pids: Vec<String> = ports.keys().map(|p| p.to_string()).collect();
    let list = pids.join(",");
    let mut cwds = HashMap::new();
    let mut pid = 0;
    for line in run("lsof", &["-a", "-d", "cwd", "-Fpn", "-p", &list]).lines() {
        if let Some(p) = line.strip_prefix('p') {
            pid = p.parse().unwrap_or(0);
        } else if let Some(n) = line.strip_prefix('n') {
            cwds.insert(pid, n.to_string());
        }
    }
    let mut args = HashMap::new();
    for line in run("ps", &["-o", "pid=,args=", "-p", &list]).lines() {
        let t = line.trim_start();
        if let Some((p, a)) = t.split_once(' ') {
            if let Ok(p) = p.parse::<i32>() {
                args.insert(p, a.trim().to_string());
            }
        }
    }
    let mut out: Vec<Listener> = ports
        .into_iter()
        .map(|(pid, ports)| Listener { pid, ports, public: public.get(&pid).cloned().unwrap_or_default(), cwd: cwds.get(&pid).cloned().unwrap_or_default(), args: args.get(&pid).cloned().unwrap_or_default() })
        .collect();
    out.sort_by_key(|l| l.ports.iter().next().copied().unwrap_or(0));
    out
}

/// Name a process tree by its nearest known starter.
pub fn source(pid: i32, table: &HashMap<i32, Proc>) -> String {
    let mut cur = pid;
    for _ in 0..40 {
        let Some(p) = table.get(&cur) else { break };
        if cur != pid {
            if let Some(s) = starter(&p.exe) {
                return s.into();
            }
        }
        if p.ppid <= 1 {
            return if cur == pid { "launchd".into() } else { starter(&p.exe).unwrap_or("launchd").into() };
        }
        cur = p.ppid;
    }
    "?".into()
}

pub fn starter(exe: &str) -> Option<&'static str> {
    let l = exe.to_lowercase();
    let base = l.rsplit('/').next().unwrap_or(&l);
    Some(if l.contains("server-hub") || l.contains("server hub") {
        "Server Hub"
    } else if base == "claude" || l.contains("/claude-code") || l.contains("claude.app/") {
        "Claude Code"
    } else if base == "codex" || l.contains("codex") {
        "Codex"
    } else if l.contains("visual studio code") || base.starts_with("code helper") || base == "code" {
        "VS Code"
    } else if l.contains("cursor.app") {
        "Cursor"
    } else if l.contains("iterm") {
        "iTerm"
    } else if l.contains("terminal.app") || base == "terminal" {
        "Terminal"
    } else if l.contains("warp") {
        "Warp"
    } else if l.contains("ghostty") {
        "Ghostty"
    } else if l.contains("wezterm") {
        "WezTerm"
    } else if base == "tmux" {
        "tmux"
    } else if l.contains("xcode.app") {
        "Xcode"
    } else if l.contains("android studio") {
        "Android Studio"
    } else {
        return None;
    })
}

/// Things that are part of macOS or a normal app, not a dev server someone forgot.
pub fn is_system(exe: &str, cwd: &str) -> bool {
    let sys = ["/System/", "/usr/libexec/", "/usr/sbin/", "/Library/Apple/", "/sbin/"];
    if sys.iter().any(|p| exe.starts_with(p)) {
        return true;
    }
    let base = exe.rsplit('/').next().unwrap_or(exe).to_lowercase();
    let runtime = ["node", "python", "ruby", "java", "php"].iter().any(|r| base.starts_with(r))
        || ["bun", "deno", "dotnet", "godot", "go", "cargo", "uvicorn", "gunicorn", "flask", "rails"].contains(&base.as_str());
    // apps launched from the Dock run in `/` (or are .app bundles); dev servers run inside their project folder
    !runtime && (cwd == "/" || cwd.is_empty() || exe.starts_with("/Applications/"))
}

pub fn alive(pid: i32) -> bool {
    pid > 0 && unsafe { libc::kill(pid, 0) } == 0
}

/// SIGTERM, then SIGKILL after 3 s if it is still there. `group` = whole process group (npm + vite + esbuild…).
pub fn terminate(pid: i32, group: bool) {
    if pid <= 1 {
        return;
    }
    let target = if group { -pid } else { pid };
    unsafe { libc::kill(target, libc::SIGTERM) };
    std::thread::spawn(move || {
        for _ in 0..30 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            if !alive(pid) {
                return;
            }
        }
        unsafe { libc::kill(target, libc::SIGKILL) };
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn etime_formats() {
        assert_eq!(etime("05"), 5);
        assert_eq!(etime("01:05"), 65);
        assert_eq!(etime("02:01:05"), 7265);
        assert_eq!(etime("3-00:00:01"), 259201);
    }

    #[test]
    fn lsof_and_ps_parse() {
        let l = parse_lsof_listen("p853\nn*:57289\nn*:57289\np1122\nn127.0.0.1:42950\np17464\nn[::1]:8600\n");
        assert_eq!(l[&853].iter().copied().collect::<Vec<_>>(), vec![57289]);
        assert!(l[&17464].contains(&8600));
        let p = parse_lsof_public("p853\nn*:57289\np1122\nn127.0.0.1:42950\np17464\nn[::1]:8600\n");
        assert!(p[&853].contains(&57289));
        assert!(!p.contains_key(&1122) && !p.contains_key(&17464));
        let t = parse_ps("  10     1    10  2048    01:00 /System/Applications/Utilities/Terminal.app/Contents/MacOS/Terminal\n  20    10    20  1000    00:30 -zsh\n  30    20    30 51200    00:10 node\n");
        assert_eq!(t[&30].rss_kb, 51200);
        assert_eq!(source(30, &t), "Terminal");
        assert_eq!(source(10, &t), "launchd");
    }

    #[test]
    fn system_filter() {
        assert!(is_system("/usr/libexec/rapportd", "/"));
        assert!(is_system("/Applications/Apidog.app/Contents/MacOS/ApidogApp", "/"));
        assert!(!is_system("node", "/"));
        assert!(is_system("/Applications/Google Drive.app/Contents/MacOS/Google Drive", "/Users/me"));
        assert!(!is_system("/opt/homebrew/bin/python3.12", "/Users/me/x"));
    }
}

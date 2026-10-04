//! Find every way to run something under the projects folder: `.claude/launch.json` (what Claude's preview uses),
//! `run.sh` (games), `package.json` with a `dev`/`start` script, plus entries the user added by hand.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Entry {
    pub id: String,
    pub repo: String,
    pub name: String,
    /// launch | run.sh | npm | custom
    pub kind: String,
    pub cwd: String,
    /// None = attach-only launch config (just a URL, nothing to start)
    pub program: Option<String>,
    pub args: Vec<String>,
    pub port: Option<u16>,
    pub url: Option<String>,
    pub file: String,
    /// top folder under the scanned root (OTHERS, COMPANY-DATA…) — sidebar groups
    #[serde(default)]
    pub group: String,
    /// the product inside the repo (apps/ACTIVE/<slug>, games/ACTIVE/<slug>, a --prefix folder…)
    #[serde(default)]
    pub project: String,
}

impl Entry {
    pub fn command(&self) -> String {
        match &self.program {
            None => "(attach only)".into(),
            Some(p) => {
                let mut s = short(p);
                for a in &self.args {
                    s.push(' ');
                    let a = short(a);
                    if a.contains(' ') { s.push_str(&format!("'{a}'")) } else { s.push_str(&a) }
                }
                s
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Custom {
    pub name: String,
    pub cwd: String,
    pub command: String,
    #[serde(default)]
    pub port: Option<u16>,
}

const SKIP: &[&str] = &[
    "node_modules", ".git", "target", "Pods", "build", "dist", "dist-mas", ".dart_tool", ".godot", ".next", ".venv", "venv",
    "DerivedData", ".build", "vendor", ".gradle", ".idea", ".svelte-kit", ".astro", "coverage", "Library", "gen",
];

/// Home folder → `~` so commands and paths stay short on screen.
pub fn short(s: &str) -> String {
    match std::env::var("HOME") {
        Ok(h) if !h.is_empty() && s.starts_with(&h) => format!("~{}", &s[h.len()..]),
        _ => s.to_string(),
    }
}

fn fnv(s: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth == 0 {
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            if SKIP.contains(&name.as_str()) || (name.starts_with('.') && name != ".claude") {
                continue;
            }
            walk(&e.path(), depth - 1, out);
        } else if ft.is_file() {
            let p = e.path();
            let launch = name == "launch.json" && dir.file_name().is_some_and(|n| n == ".claude");
            if launch || name == "run.sh" || name == "package.json" {
                out.push(p);
            }
        }
    }
}

/// The repo a file belongs to: nearest folder with `.git`, else the project folder itself.
fn repo_of(dir: &Path, root: &Path) -> String {
    let mut d = Some(dir);
    while let Some(x) = d {
        if x.join(".git").exists() {
            return x.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        }
        if x == root {
            break;
        }
        d = x.parent();
    }
    dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
}

fn mk(repo: &str, name: &str, kind: &str, cwd: &Path, program: Option<String>, args: Vec<String>, port: Option<u16>, url: Option<String>, file: &Path) -> Entry {
    let cwd = cwd.to_string_lossy().to_string();
    let id = fnv(&format!("{kind}|{cwd}|{}|{}", program.clone().unwrap_or_default(), args.join(" ")));
    Entry { id, repo: repo.into(), name: name.into(), kind: kind.into(), cwd, program, args, port, url, file: file.to_string_lossy().to_string(), group: String::new(), project: String::new() }
}

fn from_launch(file: &Path, root: &Path) -> Vec<Entry> {
    let Some(repo_dir) = file.parent().and_then(|p| p.parent()) else { return vec![] };
    let Ok(text) = fs::read_to_string(file) else { return vec![] };
    let Ok(v) = serde_json::from_str::<Value>(&text) else { return vec![] };
    let repo = repo_of(repo_dir, root);
    let mut out = vec![];
    for c in v["configurations"].as_array().cloned().unwrap_or_default() {
        let name = c["name"].as_str().unwrap_or("?");
        let program = c["runtimeExecutable"].as_str().map(String::from);
        let args: Vec<String> = c["runtimeArgs"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
        let port = c["port"].as_u64().map(|p| p as u16);
        let url = c["url"].as_str().map(String::from);
        let cwd = c["cwd"].as_str().map(PathBuf::from).unwrap_or_else(|| repo_dir.to_path_buf());
        out.push(mk(&repo, name, "launch", &cwd, program, args, port, url, file));
    }
    out
}

fn from_runsh(file: &Path, root: &Path) -> Vec<Entry> {
    let Some(dir) = file.parent() else { return vec![] };
    // games/ACTIVE/<slug>/production/run.sh → <slug>
    let slug_dir = if dir.file_name().is_some_and(|n| n == "production") { dir.parent().unwrap_or(dir) } else { dir };
    let slug = slug_dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let repo = repo_of(dir, root);
    let text = fs::read_to_string(file).unwrap_or_default();
    let prog = Some(file.to_string_lossy().to_string());
    let mut out = vec![];
    if text.contains("\"play\"") {
        out.push(mk(&repo, &format!("{slug} · play"), "run.sh", dir, prog.clone(), vec!["play".into()], None, None, file));
    }
    out.push(mk(&repo, &slug, "run.sh", dir, prog, vec![], None, None, file));
    out
}

/// `--port 3000`, `--port=3000`, `-p 3000`, `PORT=3000`, `port: 3000` → 3000.
pub fn find_port(text: &str) -> Option<u16> {
    for key in ["--port=", "--port ", "-p ", "PORT=", "port: ", "port:"] {
        let mut rest = text;
        while let Some(i) = rest.find(key) {
            let after = &rest[i + key.len()..];
            let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(p) = digits.parse::<u16>() {
                if p >= 80 {
                    return Some(p);
                }
            }
            rest = after;
        }
    }
    None
}

fn from_package(file: &Path, root: &Path) -> Vec<Entry> {
    let Some(dir) = file.parent() else { return vec![] };
    let Ok(text) = fs::read_to_string(file) else { return vec![] };
    let Ok(v) = serde_json::from_str::<Value>(&text) else { return vec![] };
    let scripts = &v["scripts"];
    let script = ["dev", "start"].into_iter().find(|s| scripts[*s].is_string());
    let Some(script) = script else { return vec![] };
    let body = scripts[script].as_str().unwrap_or("");
    // libraries/plugins: `start` that only builds or watches is not a server; keep only ones that look like one
    let looks_server = ["vite", "next", "astro", "nuxt", "serve", "node ", "nodemon", "ts-node", "tsx ", "ionic", "ng serve", "react-scripts", "webpack", "http", "express", "remix", "svelte", "parcel", "tauri", "expo", "wrangler"]
        .iter()
        .any(|k| body.contains(k));
    if !looks_server {
        return vec![];
    }
    let mut port = find_port(body);
    if port.is_none() {
        for f in ["vite.config.ts", "vite.config.js", "vite.config.mjs", "astro.config.mjs"] {
            if let Ok(t) = fs::read_to_string(dir.join(f)) {
                port = find_port(&t);
                if port.is_some() {
                    break;
                }
            }
        }
    }
    if port.is_none() && (body.contains("next ") || body.ends_with("next") || body.contains("react-scripts")) {
        port = Some(3000);
    }
    if port.is_none() && body.contains("vite") {
        port = Some(5173);
    }
    if port.is_none() && body.contains("astro") {
        port = Some(4321);
    }
    let folder = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let pkg = v["name"].as_str().unwrap_or("");
    // folders named desktop/web/app say little; show the product above them
    let generic = ["desktop", "web", "app", "site", "frontend", "client", "ui"];
    let name = if generic.contains(&folder.as_str()) {
        let parent = dir.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        format!("{parent}/{folder}")
    } else if !pkg.is_empty() && !pkg.contains('/') {
        pkg.to_string()
    } else {
        folder
    };
    let repo = repo_of(dir, root);
    vec![mk(&repo, &name, "npm", dir, Some("npm".into()), vec!["run".into(), script.into()], port, None, file)]
}

/// `npm --prefix X run dev …` and a package.json in X with `dev` are the same thing; launch.json wins (it has the port).
fn npm_key(e: &Entry) -> Option<String> {
    if e.program.as_deref() != Some("npm") {
        return None;
    }
    let mut cwd = e.cwd.clone();
    let mut script = None;
    let mut i = 0;
    while i < e.args.len() {
        match e.args[i].as_str() {
            "--prefix" if i + 1 < e.args.len() => {
                cwd = e.args[i + 1].clone();
                i += 1;
            }
            "run" if i + 1 < e.args.len() && script.is_none() => {
                script = Some(e.args[i + 1].clone());
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    Some(format!("{}|{}", cwd.trim_end_matches('/'), script.unwrap_or_default()))
}

pub fn custom_entries(list: &[Custom], file: &Path) -> Vec<Entry> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    list.iter()
        .map(|c| {
            let cwd = PathBuf::from(&c.cwd);
            let mut e = mk("Added by hand", &c.name, "custom", &cwd, Some(shell.clone()), vec!["-c".into(), c.command.clone()], c.port, None, file);
            e.group = "Added by hand".into();
            e.project = c.name.clone();
            e
        })
        .collect()
}

const GENERIC: &[&str] = &["desktop", "web", "app", "site", "frontend", "client", "ui", "mockup", "landing", "production", "src", "dist", "public", "example-app", "example", "demo"];

/// Which product a row belongs to: `<kind>/ACTIVE|BACKLOG/<slug>` anywhere in its folder or args, else the folder it runs in
/// (`--prefix X`, `--directory X`, or its own cwd when that is not the repo root), skipping generic names like `web`.
pub fn project_of(e: &Entry, repo_root: Option<&Path>) -> String {
    let paths: Vec<&str> = std::iter::once(e.cwd.as_str()).chain(e.program.as_deref()).chain(e.args.iter().map(String::as_str)).collect();
    for p in &paths {
        let parts: Vec<&str> = p.split('/').collect();
        for w in parts.windows(3) {
            if ["apps", "games", "extensions", "videos", "books"].contains(&w[0]) && ["ACTIVE", "BACKLOG"].contains(&w[1]) && !w[2].is_empty() {
                return w[2].to_string();
            }
        }
    }
    let named = |p: &str| -> Option<String> {
        Path::new(p).ancestors().filter_map(|a| a.file_name()).map(|n| n.to_string_lossy().to_string()).find(|n| !GENERIC.contains(&n.as_str()))
    };
    for (i, a) in e.args.iter().enumerate() {
        if (a == "--prefix" || a == "--directory") && i + 1 < e.args.len() && e.args[i + 1].starts_with('/') {
            if let Some(n) = named(&e.args[i + 1]) {
                return n;
            }
        }
    }
    if repo_root.is_some_and(|r| Path::new(&e.cwd) != r) {
        if let Some(n) = named(&e.cwd) {
            return n;
        }
    }
    e.repo.clone()
}

fn git_root(dir: &Path, root: &Path) -> Option<PathBuf> {
    dir.ancestors().take_while(|a| a.starts_with(root)).find(|a| a.join(".git").exists()).map(Path::to_path_buf)
}

fn label(e: &mut Entry, root: &Path) {
    let file = Path::new(&e.file);
    // first folder under the root; a project sitting right in the root is grouped under the root's own name
    let dir = file.parent().unwrap_or(file);
    e.group = dir
        .strip_prefix(root)
        .ok()
        .and_then(|r| r.components().next())
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .or_else(|| root.file_name().map(|n| n.to_string_lossy().to_string()))
        .unwrap_or_else(|| "Other".into());
    let rr = file.parent().and_then(|d| git_root(d, root));
    e.project = project_of(e, rr.as_deref());
}

pub fn scan(root: &Path, custom: &[Entry]) -> Vec<Entry> {
    let mut files = vec![];
    walk(root, 10, &mut files);
    files.sort();
    let mut launch = vec![];
    let mut rest = vec![];
    for f in &files {
        let name = f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        match name.as_str() {
            "launch.json" => launch.extend(from_launch(f, root)),
            "run.sh" => rest.extend(from_runsh(f, root)),
            _ => rest.extend(from_package(f, root)),
        }
    }
    let mut seen_ids = HashSet::new();
    let mut seen_npm = HashSet::new();
    let mut out = vec![];
    for e in launch.into_iter().chain(custom.iter().cloned()).chain(rest) {
        if !seen_ids.insert(e.id.clone()) {
            continue;
        }
        if let Some(k) = npm_key(&e) {
            if !seen_npm.insert(k) {
                continue;
            }
        }
        let mut e = e;
        if e.kind != "custom" {
            label(&mut e, root);
        }
        out.push(e);
    }
    out
}

/// Several project folders: scan each, custom rows once, drop rows found twice (one folder inside another).
pub fn scan_all(roots: &[PathBuf], custom: &[Entry]) -> Vec<Entry> {
    let mut seen = HashSet::new();
    let mut out = vec![];
    for (i, r) in roots.iter().enumerate() {
        for e in scan(r, if i == 0 { custom } else { &[] }) {
            if seen.insert(e.id.clone()) {
                out.push(e);
            }
        }
    }
    if roots.is_empty() {
        out.extend(custom.iter().cloned());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_from_scripts_and_configs() {
        assert_eq!(find_port("vite --port 1440"), Some(1440));
        assert_eq!(find_port("next dev -p 3100"), Some(3100));
        assert_eq!(find_port("server: { port: 1450, strictPort: true }"), Some(1450));
        assert_eq!(find_port("PORT=8080 node x.js"), Some(8080));
        assert_eq!(find_port("vite build"), None);
    }

    #[test]
    fn scans_and_dedups_launch_against_package() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path().join("Repo");
        let web = repo.join("apps/ACTIVE/foo/web");
        let game = repo.join("games/ACTIVE/dino/production");
        fs::create_dir_all(repo.join(".git")).unwrap();
        fs::create_dir_all(repo.join(".claude")).unwrap();
        fs::create_dir_all(&web).unwrap();
        fs::create_dir_all(&game).unwrap();
        fs::create_dir_all(web.join("node_modules/x")).unwrap();
        fs::write(web.join("node_modules/x/package.json"), r#"{"scripts":{"dev":"vite"}}"#).unwrap();
        fs::write(web.join("package.json"), r#"{"name":"foo-web","scripts":{"dev":"vite"}}"#).unwrap();
        fs::write(game.join("run.sh"), "if [ \"${1:-}\" = \"play\" ]; then exit; fi").unwrap();
        let launch = format!(
            r#"{{"configurations":[{{"name":"foo-web","runtimeExecutable":"npm","runtimeArgs":["--prefix","{}","run","dev","--","--port","1999"],"port":1999}},{{"name":"bridge","url":"http://127.0.0.1:4011","port":4011}}]}}"#,
            web.display()
        );
        fs::write(repo.join(".claude/launch.json"), launch).unwrap();

        let es = scan(t.path(), &[]);
        let names: Vec<_> = es.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["foo-web", "bridge", "dino · play", "dino"], "package.json folded into launch, node_modules skipped");
        assert!(es.iter().all(|e| e.repo == "Repo"));
        assert_eq!(es[0].port, Some(1999));
        assert_eq!(es[1].program, None);
        assert_eq!(es[2].args, vec!["play"]);
        assert!(es.iter().all(|e| e.group == "Repo"));
        assert_eq!(es[0].project, "foo", "apps/ACTIVE/<slug> in --prefix");
        assert_eq!(es[2].project, "dino");
        assert_eq!(es[1].project, "Repo", "attach-only row falls back to the repo");
    }

    #[test]
    fn several_roots_and_a_project_right_in_the_root() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        fs::write(b.path().join("package.json"), r#"{"name":"solo","scripts":{"dev":"vite --port 5173"}}"#).unwrap();
        let inner = a.path().join("Work/shop");
        fs::create_dir_all(&inner).unwrap();
        fs::write(inner.join("package.json"), r#"{"name":"shop","scripts":{"dev":"next dev -p 3000"}}"#).unwrap();
        let es = scan_all(&[a.path().to_path_buf(), b.path().to_path_buf(), inner.clone()], &[]);
        assert_eq!(es.len(), 2, "nested root does not double the shop row");
        assert_eq!(es[0].group, "Work");
        assert_eq!(es[1].group, b.path().file_name().unwrap().to_string_lossy());
    }
}

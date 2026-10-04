//! `server-hub bridge [--port 4410] [--root DIR]` — the app's API over HTTP so the UI runs in a browser.
//! `server-hub list [--root DIR]` — print every item and its status.
use hub_core::api::App;
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let opt = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let app = App::new(opt("--root").map(PathBuf::from));
    match args.first().map(String::as_str) {
        Some("list") => {
            let s = app.snapshot(true);
            for r in s["entries"].as_array().unwrap() {
                println!("{:<9} {:<28} {:<24} {:>6}  {}", r["status"].as_str().unwrap(), r["repo"].as_str().unwrap(), r["name"].as_str().unwrap(), r["port"].as_u64().map(|p| format!(":{p}")).unwrap_or_default(), r["note"].as_str().unwrap());
            }
            for p in s["ports"].as_array().unwrap() {
                println!("port      :{:<6} {:<10} {:<24} {} {}", p["port"], p["health"].as_str().unwrap(), p["name"].as_str().unwrap(), p["tool"].as_str().unwrap(), p["code"]);
            }
            for o in s["outside"].as_array().unwrap() {
                println!("outside   pid {:<6} {:<16} {} from {}", o["pid"], o["name"].as_str().unwrap(), o["ports"], o["source"].as_str().unwrap());
            }
        }
        Some("bridge") => {
            let port: u16 = opt("--port").and_then(|p| p.parse().ok()).unwrap_or(4410);
            let server = tiny_http::Server::http(("127.0.0.1", port)).unwrap_or_else(|e| panic!("port {port} busy: {e}"));
            println!("Server Hub bridge on http://127.0.0.1:{port}/api/<cmd>  (root {})", app.roots().iter().map(|r| r.display().to_string()).collect::<Vec<_>>().join(", "));
            for mut req in server.incoming_requests() {
                let mut body = String::new();
                let _ = std::io::Read::read_to_string(req.as_reader(), &mut body);
                let cmd = req.url().trim_start_matches("/api/").split('?').next().unwrap_or("").to_string();
                let a: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::json!({}));
                let (code, out) = if req.method() == &tiny_http::Method::Options {
                    (204, String::new())
                } else {
                    match app.call(&cmd, &a) {
                        Ok(v) => (200, v.to_string()),
                        Err(e) => (400, serde_json::json!({ "error": e }).to_string()),
                    }
                };
                let mut resp = tiny_http::Response::from_string(out).with_status_code(code);
                for h in ["Content-Type: application/json", "Access-Control-Allow-Origin: *", "Access-Control-Allow-Headers: content-type", "Access-Control-Allow-Methods: POST, OPTIONS"] {
                    resp.add_header(h.parse::<tiny_http::Header>().unwrap());
                }
                let _ = req.respond(resp);
            }
        }
        _ => eprintln!("usage: server-hub list | bridge [--port 4410]   [--root ~/DATA/PROJECTS]"),
    }
}

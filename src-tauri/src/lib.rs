//! The desktop shell: one `api` command that forwards to hub_core::api::App (same calls as `server-hub bridge`),
//! plus a menu bar icon showing how many servers run, with ■ for each. Closing the window keeps it in the menu bar.
use hub_core::api::App;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, RunEvent, WindowEvent};

#[tauri::command]
async fn api(app: tauri::State<'_, Arc<App>>, cmd: String, args: Value) -> Result<Value, String> {
    // runs on Tauri's async pool; lsof/ps take a few ms
    app.call(&cmd, &args)
}

fn show(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// Menu bar title: how many localhost ports are open, with a warning when one of them does not answer.
fn tray_title(snap: &Value) -> String {
    let n = snap["ports"].as_array().map(|a| a.len()).unwrap_or(0);
    match snap["unhealthy"].as_u64().unwrap_or(0) {
        0 => n.to_string(),
        bad => format!("{n} · {bad}!"),
    }
}

/// Menu bar list: every open localhost port (click = open in browser), running rows with ■, open, quit.
fn tray_menu(app: &AppHandle, snap: &Value) -> tauri::Result<Menu<tauri::Wry>> {
    let menu = Menu::new(app)?;
    let ports = snap["ports"].as_array().cloned().unwrap_or_default();
    let head = if ports.is_empty() { "No localhost port open".to_string() } else { format!("Localhost · {} open", ports.len()) };
    menu.append(&MenuItem::with_id(app, "lhead", head, false, None::<&str>)?)?;
    for p in &ports {
        let mark = match p["health"].as_str() { Some("healthy") => "🟢", Some("tcp") => "⚪", _ => "🔴" };
        let label = format!("{mark}  :{}  {} · {}", p["port"], p["name"].as_str().unwrap_or("?"), p["tool"].as_str().unwrap_or(""));
        menu.append(&MenuItem::with_id(app, format!("url:http://localhost:{}", p["port"]), label, true, None::<&str>)?)?;
    }
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    let rows: Vec<&Value> = snap["entries"].as_array().map(|a| a.iter().filter(|r| r["status"] == "running" || r["status"] == "starting").collect()).unwrap_or_default();
    if !rows.is_empty() {
        menu.append(&MenuItem::with_id(app, "head", format!("Projects · {} running", rows.len()), false, None::<&str>)?)?;
    }
    for r in &rows {
        let port = r["ports"].as_array().and_then(|p| p.first()).and_then(|p| p.as_u64()).map(|p| format!("  :{p}")).unwrap_or_default();
        let label = format!("■  {}{port}", r["name"].as_str().unwrap_or("?"));
        menu.append(&MenuItem::with_id(app, format!("stop:{}", r["id"].as_str().unwrap_or("")), label, true, None::<&str>)?)?;
    }
    if !rows.is_empty() {
        menu.append(&PredefinedMenuItem::separator(app)?)?;
    }
    menu.append(&MenuItem::with_id(app, "open", "Open Server Hub", true, None::<&str>)?)?;
    menu.append(&MenuItem::with_id(app, "quit", "Quit Server Hub", true, None::<&str>)?)?;
    Ok(menu)
}

pub fn run() {
    let core = Arc::new(App::new(None));
    let app = tauri::Builder::default()
        .manage(core.clone())
        .invoke_handler(tauri::generate_handler![api])
        .setup(move |app| {
            let handle = app.handle().clone();
            let snap = core.snapshot(true);
            let tray = TrayIconBuilder::with_id("hub")
                .icon(app.default_window_icon().cloned().expect("app icon"))
                .title(tray_title(&snap))
                .tooltip("Server Hub")
                .menu(&tray_menu(&handle, &snap)?)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, ev| {
                    let id = ev.id().as_ref().to_string();
                    match id.as_str() {
                        "open" => show(app),
                        "quit" => app.exit(0),
                        _ => {
                            if let Some(url) = id.strip_prefix("url:") {
                                let _ = hub_core::api::open(url, None);
                            } else if let Some(row) = id.strip_prefix("stop:") {
                                let core = app.state::<Arc<App>>();
                                let _ = core.call("stop", &json!({ "id": row }));
                            }
                        }
                    }
                })
                .build(app)?;
            // keep the menu bar in step; rebuild the menu only when what runs changes (rebuilding closes an open menu)
            let core = core.clone();
            std::thread::spawn(move || {
                let mut last = String::new();
                loop {
                    std::thread::sleep(Duration::from_secs(3));
                    let snap = core.snapshot(false);
                    let sig = format!(
                        "{:?}{:?}",
                        snap["entries"].as_array().map(|a| a.iter().filter(|r| r["status"] == "running").map(|r| format!("{}{}", r["id"], r["ports"])).collect::<Vec<_>>()),
                        snap["ports"].as_array().map(|a| a.iter().map(|p| format!("{}{}", p["port"], p["health"])).collect::<Vec<_>>())
                    );
                    if sig != last {
                        last = sig;
                        let _ = tray.set_title(Some(tray_title(&snap)));
                        if let Ok(m) = tray_menu(&handle, &snap) {
                            let _ = tray.set_menu(Some(m));
                        }
                    }
                }
            });
            Ok(())
        })
        .on_window_event(|w, ev| {
            if let WindowEvent::CloseRequested { api, .. } = ev {
                // closing the window keeps Server Hub in the menu bar; Quit lives in the tray menu and ⌘Q
                api.prevent_close();
                let _ = w.hide();
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building Server Hub");
    app.run(|app, ev| {
        if let RunEvent::Reopen { .. } = ev {
            show(app);
        }
    });
}

//! Minimal child entrypoint: no database, provider services, tray, single-instance
//! plugin, filesystem bridge, or main-app command registration.
use super::*;
use std::{
    io::{Read, Write},
    sync::atomic::{AtomicU64, Ordering},
};
use tauri::Manager;
use tokio::sync::oneshot;

struct Bridge {
    output: Mutex<std::io::Stdout>,
    pending: Mutex<Option<(u64, oneshot::Sender<Result<Value>>)>>,
    sequence: AtomicU64,
}

fn read_sync(reader: &mut impl Read, limit: usize) -> Result<Vec<u8>> {
    let mut size = [0; 4];
    reader.read_exact(&mut size).map_err(|e| e.to_string())?;
    let size = u32::from_be_bytes(size) as usize;
    if size == 0 || size > limit {
        return Err("PLUGIN_MESSAGE_TOO_LARGE".into());
    }
    let mut bytes = vec![0; size];
    reader.read_exact(&mut bytes).map_err(|e| e.to_string())?;
    Ok(bytes)
}

impl Bridge {
    fn send(&self, message: &FromChild) -> Result<()> {
        let bytes = serde_json::to_vec(message).map_err(|e| e.to_string())?;
        if bytes.len() > MAX_MESSAGE {
            return Err("PLUGIN_MESSAGE_TOO_LARGE".into());
        }
        let mut output = self.output.lock().map_err(|_| "PLUGIN_PIPE_CLOSED")?;
        output
            .write_all(&(bytes.len() as u32).to_be_bytes())
            .and_then(|()| output.write_all(&bytes))
            .and_then(|()| output.flush())
            .map_err(|e| e.to_string())
    }
}

#[tauri::command]
async fn plugin_runtime_request(
    state: tauri::State<'_, Arc<Bridge>>,
    method: String,
    value: Value,
) -> Result<Value> {
    if method.len() > 32 {
        return Err("Unsupported plugin method".into());
    }
    let bridge = state.inner().clone();
    let seq = bridge.sequence.fetch_add(1, Ordering::Relaxed);
    let (sender, receiver) = oneshot::channel();
    {
        let mut pending = bridge.pending.lock().map_err(|_| "PLUGIN_PIPE_CLOSED")?;
        if pending.is_some() {
            return Err("Plugin host is busy; await the previous request".into());
        }
        *pending = Some((seq, sender));
    }
    let result = tauri::async_runtime::spawn_blocking({
        let bridge = bridge.clone();
        move || bridge.send(&FromChild::Request { seq, method, value })
    })
    .await
    .map_err(|_| "PLUGIN_PIPE_CLOSED".to_string())
    .and_then(|r| r);
    if let Err(error) = result {
        bridge
            .pending
            .lock()
            .map_err(|_| "PLUGIN_PIPE_CLOSED")?
            .take();
        return Err(error);
    }
    receiver
        .await
        .map_err(|_| "PLUGIN_PIPE_CLOSED".to_string())?
}

#[tauri::command]
async fn plugin_runtime_heartbeat(state: tauri::State<'_, Arc<Bridge>>) -> Result<()> {
    let bridge = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || bridge.send(&FromChild::Heartbeat))
        .await
        .map_err(|_| "PLUGIN_PIPE_CLOSED".to_string())?
}

pub(super) fn response(path: &str, boot: &Boot) -> http::Response<Vec<u8>> {
    let (status, csp, body) = match path {
        "/shell" => (200, "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src ipc: http://ipc.localhost; frame-src cc-plugin: http://cc-plugin.localhost https://cc-plugin.localhost; object-src 'none'; base-uri 'none'; form-action 'none'", include_str!("shell.html").to_string()),
        "/page" => (200, PAGE_CSP, format!("<!doctype html><meta charset=\"utf-8\"><script>{}</script>{}", include_str!("../../../../sdk/ui-plugin.js"), boot.html)),
        _ => (404, PAGE_CSP, String::new()),
    };
    http::Response::builder()
        .status(status)
        .header("Content-Type", "text/html; charset=utf-8")
        .header("Content-Security-Policy", csp)
        .header("Cache-Control", "no-store")
        .header("X-Content-Type-Options", "nosniff")
        .header("Referrer-Policy", "no-referrer")
        .body(body.into_bytes())
        .expect("static plugin response")
}

pub fn run() -> Result<()> {
    let boot: Boot = serde_json::from_slice(&read_sync(&mut std::io::stdin(), MAX_BOOT)?)
        .map_err(|e| e.to_string())?;
    let boot = Arc::new(boot);
    let bridge = Arc::new(Bridge {
        output: Mutex::new(std::io::stdout()),
        pending: Mutex::new(None),
        sequence: AtomicU64::new(1),
    });
    let profile = PathBuf::from(
        std::env::var_os("CC_SWITCH_PLUGIN_PROFILE").ok_or("Missing runtime profile")?,
    );
    let mut context = crate::app_context();
    let config = context.config_mut();
    config.identifier = "com.ccswitch.plugin-runtime".into();
    config.app.windows.clear();
    config.build.dev_url = None;
    config.app.security.csp = None;
    config.app.security.dev_csp = None;
    config.app.security.asset_protocol.enable = false;
    let protocol_boot = boot.clone();
    tauri::Builder::default()
        .manage(bridge.clone())
        .register_uri_scheme_protocol("cc-plugin", move |_ctx, request| {
            if request.method() != http::Method::GET {
                return response("", &protocol_boot);
            }
            response(request.uri().path(), &protocol_boot)
        })
        .invoke_handler(tauri::generate_handler![
            plugin_runtime_request,
            plugin_runtime_heartbeat
        ])
        .setup(move |app| {
            let url = "cc-plugin://localhost/shell".parse().expect("static URL");
            let window = tauri::WebviewWindowBuilder::new(
                app,
                "plugin-runtime",
                tauri::WebviewUrl::CustomProtocol(url),
            )
            .title(&boot.title)
            .inner_size(960.0, 650.0)
            .min_inner_size(400.0, 300.0)
            .data_directory(profile)
            .incognito(true)
            .disable_drag_drop_handler()
            .devtools(false)
            .on_navigation(|url| {
                (url.scheme() == "cc-plugin" && url.host_str() == Some("localhost")
                    || matches!(url.scheme(), "http" | "https")
                        && url.host_str() == Some("cc-plugin.localhost"))
                    && matches!(url.path(), "/shell" | "/page")
            })
            .initialization_script(format!(
                "window.__PLUGIN_CONTEXT__ = {};",
                serde_json::to_string(&boot.context)?
            ))
            .build()?;
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                loop {
                    let message = read_sync(&mut std::io::stdin(), MAX_REPLY).and_then(|bytes| {
                        serde_json::from_slice::<ToChild>(&bytes).map_err(|e| e.to_string())
                    });
                    match message {
                        Ok(ToChild::Reply { seq, result }) => {
                            if let Ok(mut pending) = bridge.pending.lock() {
                                if pending
                                    .as_ref()
                                    .is_some_and(|(expected, _)| *expected == seq)
                                {
                                    if let Some((_, sender)) = pending.take() {
                                        let _ = sender.send(result);
                                    }
                                }
                            }
                        }
                        Ok(ToChild::Appearance { appearance }) => {
                            let ui = handle.clone();
                            let _ = handle.run_on_main_thread(move || {
                                if let Some(window) = ui.get_webview_window("plugin-runtime") {
                                    if let Ok(json) = serde_json::to_string(&appearance) {
                                        let _ = window
                                            .eval(format!("window.__pluginAppearance?.({json})"));
                                    }
                                }
                            });
                        }
                        Ok(ToChild::Focus) => {
                            let ui = handle.clone();
                            let _ = handle.run_on_main_thread(move || {
                                if let Some(window) = ui.get_webview_window("plugin-runtime") {
                                    let _ = window.unminimize();
                                    let _ = window.show();
                                    let _ = window.set_focus();
                                }
                            });
                        }
                        // Parent exit/crash closes the pipe. This is independent of the UI loop.
                        Err(_) => {
                            #[cfg(unix)]
                            unsafe {
                                if libc::getpgrp() == libc::getpid() {
                                    libc::kill(0, libc::SIGKILL);
                                }
                            }
                            std::process::exit(0);
                        }
                    }
                }
            });
            // Native-driven challenges avoid relying on background-window JS timers.
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_secs(1));
                if window.eval("window.__pluginPing?.()").is_err() {
                    break;
                }
            });
            Ok(())
        })
        .run(context)
        .map_err(|e| e.to_string())
}

//! A disposable, separately launched Tauri process owns all third-party JavaScript.
//! The main process only accepts bounded, allow-listed messages over private pipes.
use super::*;
use crate::{database::Database, store::AppState};
use std::{collections::HashMap, process::Stdio, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    process::{Child, Command},
    sync::{mpsc, watch},
    time::Instant,
};

pub(crate) mod child;
mod process;
use process::ProcessTree;
#[cfg(test)]
mod tests;

pub const CHILD_FLAG: &str = "--cc-switch-plugin-runtime";
const MAX_MESSAGE: usize = 128 * 1024;
const MAX_REPLY: usize = 1024 * 1024;
const MAX_BOOT: usize = MAX_HTML * 6 + MAX_MESSAGE;
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(10);
const START_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Appearance {
    language: String,
    theme: String,
}

impl Appearance {
    fn validate(&self) -> Result<()> {
        if self.language.len() > 32 || !matches!(self.theme.as_str(), "light" | "dark") {
            return Err("PLUGIN_INVALID_CONTEXT".into());
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct Boot {
    title: String,
    html: String,
    context: Value,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum FromChild {
    Heartbeat,
    Request {
        seq: u64,
        method: String,
        value: Value,
    },
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum ToChild {
    Reply { seq: u64, result: Result<Value> },
    Appearance { appearance: Appearance },
    Focus,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatus {
    pub session_id: String,
    pub state: &'static str,
    pub pid: Option<u32>,
}

struct Session {
    plugin_id: String,
    status: Arc<Mutex<RuntimeStatus>>,
    stop: watch::Sender<bool>,
    control: mpsc::Sender<ToChild>,
}

// The UI normally owns one session. A small bound allows mount/unmount overlap;
// closed sessions are retained until their owner disposes them, never accumulated.
static SESSIONS: std::sync::LazyLock<Mutex<HashMap<String, Session>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));
static REQUEST_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);

fn sessions() -> Result<std::sync::MutexGuard<'static, HashMap<String, Session>>> {
    SESSIONS
        .lock()
        .map_err(|_| "PLUGIN_RUNTIME_UNAVAILABLE".into())
}

pub(super) fn stop_plugin(id: &str) {
    if let Ok(sessions) = SESSIONS.lock() {
        for session in sessions.values().filter(|s| s.plugin_id == id) {
            let _ = session.stop.send(true);
        }
    }
}

pub(crate) fn stop_all() {
    if let Ok(mut sessions) = SESSIONS.lock() {
        for (_, session) in sessions.drain() {
            let _ = session.stop.send(true);
        }
    }
}

fn update(status: &Mutex<RuntimeStatus>, state: &'static str) {
    if let Ok(mut status) = status.lock() {
        status.state = state;
    }
}

async fn read_frame(reader: &mut (impl AsyncRead + Unpin), limit: usize) -> Result<Vec<u8>> {
    let size = reader.read_u32().await.map_err(|e| e.to_string())? as usize;
    if size == 0 || size > limit {
        return Err("PLUGIN_MESSAGE_TOO_LARGE".into());
    }
    let mut bytes = vec![0; size];
    reader
        .read_exact(&mut bytes)
        .await
        .map_err(|e| e.to_string())?;
    Ok(bytes)
}

async fn write_frame(
    writer: &mut (impl AsyncWrite + Unpin),
    value: &impl Serialize,
    limit: usize,
) -> Result<()> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if bytes.len() > limit {
        return Err("PLUGIN_MESSAGE_TOO_LARGE".into());
    }
    // A non-reading child must never stall the supervisor.
    tokio::time::timeout(Duration::from_secs(2), async {
        writer.write_u32(bytes.len() as u32).await?;
        writer.write_all(&bytes).await?;
        writer.flush().await
    })
    .await
    .map_err(|_| "PLUGIN_PIPE_TIMEOUT".to_string())?
    .map_err(|e| e.to_string())
}

fn command() -> Result<Command> {
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut command = Command::new(executable);
    command
        .arg(CHILD_FLAG)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .env_clear();
    // Pass OS display/runtime essentials, not provider tokens or developer secrets.
    for key in [
        "PATH",
        "HOME",
        "TMPDIR",
        "TMP",
        "TEMP",
        "LANG",
        "LC_ALL",
        "DISPLAY",
        "WAYLAND_DISPLAY",
        "XDG_RUNTIME_DIR",
        "DBUS_SESSION_BUS_ADDRESS",
        "XAUTHORITY",
        "SystemRoot",
        "WINDIR",
        "USERPROFILE",
        "LOCALAPPDATA",
        "APPDATA",
        "WEBKIT_DISABLE_DMABUF_RENDERER",
        "WEBKIT_DISABLE_COMPOSITING_MODE",
        "GDK_BACKEND",
        "APPDIR",
        "LD_LIBRARY_PATH",
        "GSETTINGS_SCHEMA_DIR",
        "GTK_PATH",
        "GDK_PIXBUF_MODULE_FILE",
        "GI_TYPELIB_PATH",
        "XDG_DATA_DIRS",
        "GIO_EXTRA_MODULES",
        "WEBVIEW2_BROWSER_EXECUTABLE_FOLDER",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    #[cfg(unix)]
    {
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        command.creation_flags(0x08000000);
    } // CREATE_NO_WINDOW (native windows still work)
    Ok(command)
}

async fn terminate(child: &mut Child, tree: &ProcessTree) {
    tree.kill();
    let _ = child.start_kill();
    let _ = child.wait().await;
}

#[tauri::command]
pub async fn start_ui_plugin_runtime(
    state: tauri::State<'_, AppState>,
    id: String,
    page: String,
    appearance: Appearance,
) -> Result<RuntimeStatus> {
    appearance.validate()?;
    let plugin_id = id.clone();
    let (boot, revision) = with_registry_async(move |root| {
        let record = read_record(root, &id)?;
        let html = page_html(root, &format!("/{id}/{page}"))?;
        let title = record.manifest.pages.iter().find(|p| p.id == page).ok_or("Unknown plugin page")?;
        Ok((Boot {
            title: format!("{} — {}", record.manifest.name, title.title), html,
            context: serde_json::json!({"apiVersion":1,"pluginId":id,"pageId":page,
                "permissions":record.granted_permissions,"language":appearance.language,"theme":appearance.theme}),
        }, record.access_revision))
    }).await?;
    let session_id = uuid::Uuid::new_v4().to_string();
    let status = RuntimeStatus {
        session_id: session_id.clone(),
        state: "starting",
        pid: None,
    };
    let status_ref = Arc::new(Mutex::new(status.clone()));
    let (stop, stop_rx) = watch::channel(false);
    let (control, control_rx) = mpsc::channel(4);
    {
        let mut sessions = sessions()?;
        if sessions.len() >= 4 {
            return Err("PLUGIN_RUNTIME_LIMIT".into());
        }
        sessions.insert(
            session_id,
            Session {
                plugin_id: plugin_id.clone(),
                status: status_ref.clone(),
                stop,
                control,
            },
        );
    }
    let db = state.db.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = supervise(
            boot,
            plugin_id,
            revision,
            db,
            status_ref.clone(),
            stop_rx,
            control_rx,
        )
        .await
        {
            log::warn!("Plugin runtime stopped: {error}");
            update(&status_ref, "failed");
        }
    });
    Ok(status)
}

#[tauri::command]
pub async fn get_ui_plugin_runtime(session_id: String) -> Result<RuntimeStatus> {
    sessions()?
        .get(&session_id)
        .ok_or("PLUGIN_RUNTIME_CLOSED")?
        .status
        .lock()
        .map(|s| s.clone())
        .map_err(|_| "PLUGIN_RUNTIME_UNAVAILABLE".into())
}

#[tauri::command]
pub async fn stop_ui_plugin_runtime(session_id: String) -> Result<()> {
    if let Some(session) = sessions()?.remove(&session_id) {
        let _ = session.stop.send(true);
    }
    Ok(())
}

#[tauri::command]
pub async fn update_ui_plugin_runtime(
    session_id: String,
    appearance: Option<Appearance>,
) -> Result<()> {
    if let Some(value) = &appearance {
        value.validate()?;
    }
    sessions()?
        .get(&session_id)
        .ok_or("PLUGIN_RUNTIME_CLOSED")?
        .control
        .try_send(match appearance {
            Some(appearance) => ToChild::Appearance { appearance },
            None => ToChild::Focus,
        })
        .map_err(|_| "PLUGIN_RUNTIME_BUSY".into())
}

async fn supervise(
    boot: Boot,
    id: String,
    revision: String,
    db: Arc<Database>,
    status: Arc<Mutex<RuntimeStatus>>,
    mut stop: watch::Receiver<bool>,
    mut control: mpsc::Receiver<ToChild>,
) -> Result<()> {
    let (checked_id, checked_revision) = (id.clone(), revision.clone());
    with_registry_async(move |root| {
        let record = read_record(root, &checked_id)?;
        if !record.enabled || record.access_revision != checked_revision {
            return Err("PLUGIN_ACCESS_CHANGED".into());
        }
        Ok(())
    })
    .await?;
    if *stop.borrow() {
        update(&status, "closed");
        return Ok(());
    }
    let profile = tempfile::tempdir().map_err(|e| e.to_string())?;
    let mut child = command()?
        .env("CC_SWITCH_PLUGIN_PROFILE", profile.path())
        .spawn()
        .map_err(|e| e.to_string())?;
    let tree = ProcessTree::attach(&child)?;
    if let Ok(mut state) = status.lock() {
        state.pid = child.id();
    }
    let mut input = child.stdin.take().ok_or("Missing child input")?;
    let mut output = child.stdout.take().ok_or("Missing child output")?;
    if let Err(error) = write_frame(&mut input, &boot, MAX_BOOT).await {
        terminate(&mut child, &tree).await;
        return Err(error);
    }
    let (sender, mut messages) = mpsc::channel(4);
    let reader = tauri::async_runtime::spawn(async move {
        loop {
            let message = read_frame(&mut output, MAX_MESSAGE)
                .await
                .and_then(|bytes| {
                    serde_json::from_slice::<FromChild>(&bytes).map_err(|e| e.to_string())
                });
            let failed = message.is_err();
            if sender.send(message).await.is_err() || failed {
                break;
            }
        }
    });
    let mut deadline = Instant::now() + START_TIMEOUT;
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    let mut previous_tick = Instant::now();
    let mut rate_start = Instant::now();
    let mut received = 0;
    let (reply_tx, mut replies) = mpsc::channel(1);
    let mut busy = false;
    let mut context = boot.context;
    let outcome: Result<&'static str> = loop {
        tokio::select! {
            _ = stop.changed() => break Ok("closed"),
            _ = tick.tick() => {
                let now = Instant::now();
                // System suspend is not evidence that a plugin hung.
                if now.duration_since(previous_tick) > HEARTBEAT_TIMEOUT { deadline = now + HEARTBEAT_TIMEOUT; }
                previous_tick = now;
                if now >= deadline { break Ok("unresponsive"); }
                match child.try_wait() {
                    Ok(Some(exit)) => break Ok(if exit.success() { "closed" } else { "failed" }),
                    Err(error) => break Err(error.to_string()),
                    _ => {}
                }
            }
            Some(command) = control.recv() => {
                if let ToChild::Appearance { appearance } = &command {
                    context["language"] = Value::String(appearance.language.clone());
                    context["theme"] = Value::String(appearance.theme.clone());
                }
                if let Err(error) = write_frame(&mut input, &command, MAX_REPLY).await { break Err(error); }
            }
            Some((seq, result)) = replies.recv() => {
                busy = false;
                if let Err(error) = write_frame(&mut input, &ToChild::Reply { seq, result }, MAX_REPLY).await { break Err(error); }
            }
            message = messages.recv() => {
                if rate_start.elapsed() >= Duration::from_secs(1) { rate_start = Instant::now(); received = 0; }
                received += 1;
                if received > 32 { break Err("PLUGIN_RATE_LIMIT".into()); }
                match message {
                    Some(Ok(FromChild::Heartbeat)) => { deadline = Instant::now() + HEARTBEAT_TIMEOUT; update(&status, "running"); }
                    Some(Ok(FromChild::Request { seq, method, value })) => {
                        if busy { break Err("PLUGIN_CONCURRENT_REQUEST".into()); }
                        busy = true;
                        let (id, revision, db, context, reply_tx) = (id.clone(), revision.clone(), db.clone(), context.clone(), reply_tx.clone());
                        tauri::async_runtime::spawn(async move {
                            let result = dispatch(db, id, revision, context, method, value).await;
                            let _ = reply_tx.send((seq, result)).await;
                        });
                    }
                    _ => {
                        // EOF after a normal native-window close is a regular closed state.
                        let exit = tokio::time::timeout(Duration::from_millis(100), child.wait()).await;
                        break Ok(if matches!(exit, Ok(Ok(status)) if status.success()) { "closed" } else { "failed" });
                    }
                }
            }
        }
    };
    reader.abort();
    terminate(&mut child, &tree).await;
    update(&status, outcome?);
    Ok(())
}

async fn dispatch(
    db: Arc<Database>,
    id: String,
    revision: String,
    context: Value,
    method: String,
    value: Value,
) -> Result<Value> {
    let permit = REQUEST_SLOTS.try_acquire().map_err(|_| "PLUGIN_BUSY")?;
    tauri::async_runtime::spawn_blocking(move || {
        let _permit = permit;
        let check = |root: &Path| {
            let record = read_record(root, &id)?;
            if !record.enabled || record.access_revision != revision {
                return Err("PLUGIN_ACCESS_CHANGED".into());
            }
            Ok(())
        };
        with_registry(check)?;
        let result = match method.as_str() {
            "ready" | "getContext" => Ok(context),
            "getStorage" => with_registry(|root| {
                check(root)?;
                storage(root, &id, None)
            }),
            "setStorage" => with_registry(|root| {
                check(root)?;
                storage(root, &id, Some(value)).map(|_| Value::Null)
            }),
            "providers.list" => {
                with_registry(|root| authorize(root, &id, PluginPermission::ProvidersRead))?;
                let query = serde_json::from_value(value).map_err(|_| "PLUGIN_INVALID_QUERY")?;
                serde_json::to_value(read_api::providers(
                    &read_api::isolated_reader(&db)?,
                    query,
                )?)
                .map_err(|e| e.to_string())
            }
            "usage.getStats" => {
                with_registry(|root| authorize(root, &id, PluginPermission::UsageRead))?;
                let query = serde_json::from_value(value).map_err(|_| "PLUGIN_INVALID_QUERY")?;
                serde_json::to_value(read_api::usage(&read_api::isolated_reader(&db)?, query)?)
                    .map_err(|e| e.to_string())
            }
            _ => Err("Unsupported plugin method".into()),
        }?;
        with_registry(check)?;
        Ok(result)
    })
    .await
    .map_err(|_| "PLUGIN_TASK_FAILED".to_string())?
}

//! Opt-in native acceptance runner. Never compiled into normal application builds.
//! Exercises the real child WebView, pipe bridge, registry and process supervisor.
use super::*;
use serde_json::json;

const ID: &str = "test.native-compat";
const REPORT_FLAG: &str = "--plugin-compat-check";
const PARENT_FLAG: &str = "--plugin-compat-parent";

pub(crate) fn entry() -> bool {
    let args: Vec<_> = std::env::args_os().collect();
    let Some(flag) = args.get(1) else {
        return false;
    };
    if flag != REPORT_FLAG && flag != PARENT_FLAG {
        return false;
    }
    let Some(path) = args.get(2).map(PathBuf::from) else {
        std::process::exit(2);
    };
    if flag == PARENT_FLAG {
        let result = tauri::async_runtime::block_on(parent_fixture(&path));
        if let Err(error) = result {
            let _ = fs::write(path.join("error.txt"), error);
        }
        std::process::exit(1);
    }
    let diagnostics = path
        .parent()
        .unwrap_or(Path::new("."))
        .join("plugin-compat-logs");
    if fs::create_dir_all(&diagnostics).is_err() {
        std::process::exit(2);
    }
    std::env::set_var("CC_SWITCH_PLUGIN_DIAGNOSTICS", diagnostics);
    let mut report = json!({"platform":std::env::consts::OS,"passed":[],"success":false});
    let result = tauri::async_runtime::block_on(run(&mut report));
    report["success"] = json!(result.is_ok());
    if let Err(error) = &result {
        report["error"] = json!(error);
    }
    if fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).is_err() {
        std::process::exit(2);
    }
    std::process::exit(if result.is_ok() { 0 } else { 1 });
}

fn setup(home: &Path) -> Result<Arc<Database>> {
    std::env::set_var("CC_SWITCH_TEST_HOME", home);
    let db = Arc::new(Database::init().map_err(|e| e.to_string())?);
    {
        let conn = db.conn.lock().map_err(|e| e.to_string())?;
        conn.execute("INSERT INTO providers (id,app_type,name,settings_config) VALUES ('fixture','claude','Native fixture','{\"apiKey\":\"PRIVATE_COMPAT_TOKEN\"}')", []).map_err(|e|e.to_string())?;
        conn.execute("INSERT INTO proxy_request_logs (request_id,provider_id,app_type,model,input_tokens,output_tokens,total_cost_usd,latency_ms,status_code,created_at) VALUES ('fixture','fixture','claude','test',100,20,'0.001',10,200,?1)", [chrono::Utc::now().timestamp()-3600]).map_err(|e|e.to_string())?;
    }
    let root = crate::config::get_app_config_dir().join("ui-plugins");
    let record = PluginRecord {
        manifest: PluginManifest {
            api_version: 1,
            id: ID.into(),
            name: "Native compatibility acceptance".into(),
            version: "1".into(),
            description: String::new(),
            pages: vec![PluginPage {
                id: "main".into(),
                title: "Acceptance".into(),
                entry: "index.html".into(),
            }],
            permissions: vec![PluginPermission::ProvidersRead, PluginPermission::UsageRead],
        },
        enabled: true,
        documents: BTreeMap::from([("main".into(), String::new())]),
        storage: json!({"note":"preserved"}),
        granted_permissions: vec![PluginPermission::ProvidersRead, PluginPermission::UsageRead],
        access_revision: uuid::Uuid::new_v4().to_string(),
    };
    write_record(&root, &record)?;
    Ok(db)
}

async fn launch(db: &Arc<Database>, scenario: &str, hang: bool, denied: bool) -> Result<String> {
    let script = format!(
        r#"
<!doctype html><meta charset="utf-8"><h1>Native plugin acceptance</h1><script>
CCSwitch.ready.then(async context => {{
  const saved = await CCSwitch.getStorage();
  const providers = await CCSwitch.providers.list({{limit:1}});
  if(providers.total !== 1 || providers.items[0].id !== 'fixture' || JSON.stringify(providers).includes('PRIVATE_COMPAT_TOKEN')) throw new Error('Invalid provider projection');
  const endDate = Math.floor(Date.now()/1000);
  let denied = false;
  try {{ const usage = await CCSwitch.usage.getStats({{startDate:endDate-86400,endDate}}); if(usage.summary.totalRequests !== 1) throw new Error('Invalid usage summary'); }}
  catch(error) {{ if(error.message !== 'PLUGIN_PERMISSION_DENIED') throw error; denied = true; }}
  await CCSwitch.setStorage({{note:saved.note,scenario:{scenario},pluginId:context.pluginId,providers:providers.total,denied}});
  if ({hang}) setTimeout(() => {{ while(true) {{}} }}, 2000);
}}).catch(error => CCSwitch.setStorage({{scenario:{scenario},error:String(error)}}));
</script>"#,
        scenario = serde_json::to_string(scenario).unwrap()
    );
    with_registry_async(move |root| {
        let mut record = read_record(root, ID)?;
        record.documents.insert("main".into(), script);
        record.granted_permissions = if denied {
            vec![PluginPermission::ProvidersRead]
        } else {
            record.manifest.permissions.clone()
        };
        record.access_revision = uuid::Uuid::new_v4().to_string();
        write_record(root, &record)
    })
    .await?;
    let status = start(
        db.clone(),
        ID.into(),
        "main".into(),
        Appearance {
            language: "en".into(),
            theme: "light".into(),
        },
    )
    .await?;
    Ok(status.session_id)
}

async fn wait_ready(id: &str, scenario: &str, denied: bool) -> Result<u32> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let status = get_ui_plugin_runtime(id.into()).await?;
        let saved = with_registry_async(|root| storage(root, ID, None)).await?;
        if saved["scenario"] == scenario && !saved["error"].is_null() {
            return Err(saved.to_string());
        }
        if status.state == "running" && saved["scenario"] == scenario {
            if saved["note"] != "preserved" || saved["pluginId"] != ID || saved["denied"] != denied
            {
                return Err(format!("Unexpected native API result: {saved}"));
            }
            return status.pid.ok_or_else(|| "Missing native child PID".into());
        }
        if !matches!(status.state, "starting" | "running") || Instant::now() >= deadline {
            return Err(format!(
                "Native page did not become ready: {status:?}; storage={saved}"
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn wait_state(id: &str, expected: &str, pid: u32) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut ticks = 0;
    loop {
        let status = get_ui_plugin_runtime(id.into()).await?;
        if status.state == expected && !process_alive(pid) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "Expected {expected}, got {status:?}; host progress ticks={ticks}"
            ));
        }
        ticks += 1;
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn stop(id: &str, pid: u32) -> Result<()> {
    stop_ui_plugin_runtime(id.into()).await?;
    wait_exit(pid).await
}

async fn wait_exit(pid: u32) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while process_alive(pid) {
        if Instant::now() >= deadline {
            return Err(format!("Process {pid} survived termination"));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(())
}

fn process_alive(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    if let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) {
        if stat
            .rsplit_once(')')
            .is_some_and(|(_, fields)| fields.trim_start().starts_with('Z'))
        {
            return false;
        }
    }
    #[cfg(unix)]
    unsafe {
        libc::kill(pid as i32, 0) == 0
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::{Foundation::CloseHandle, System::Threading::*};
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut code = 0;
        let alive = GetExitCodeProcess(handle, &mut code) != 0 && code == 259;
        CloseHandle(handle);
        alive
    }
}

fn descendants(root: u32) -> Result<Vec<u32>> {
    #[cfg(any(target_os = "linux", windows))]
    let mut parents: Vec<(u32, u32)> = Vec::new();
    // macOS WebKit XPC services are owned by launchd, not this process tree.
    #[cfg(not(any(target_os = "linux", windows)))]
    let parents: Vec<(u32, u32)> = Vec::new();
    #[cfg(target_os = "linux")]
    for entry in fs::read_dir("/proc").map_err(|e| e.to_string())?.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        if let Ok(stat) = fs::read_to_string(entry.path().join("stat")) {
            if let Some(parent) = stat
                .rsplit_once(')')
                .and_then(|(_, fields)| fields.split_whitespace().nth(1))
                .and_then(|parent| parent.parse().ok())
            {
                parents.push((pid, parent));
            }
        }
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::{
            Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
            System::Diagnostics::ToolHelp::*,
        };
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of_val(&entry) as u32;
        let mut present = Process32FirstW(snapshot, &mut entry);
        while present != 0 {
            parents.push((entry.th32ProcessID, entry.th32ParentProcessID));
            present = Process32NextW(snapshot, &mut entry);
        }
        CloseHandle(snapshot);
    }
    let mut found = std::collections::BTreeSet::from([root]);
    loop {
        let before = found.len();
        for (pid, parent) in &parents {
            if found.contains(parent) {
                found.insert(*pid);
            }
        }
        if found.len() == before {
            break;
        }
    }
    found.remove(&root);
    #[cfg(any(target_os = "linux", windows))]
    if found.is_empty() {
        return Err("No native WebView descendant was discovered".into());
    }
    Ok(found.into_iter().collect())
}

fn crash(pid: u32) -> Result<()> {
    #[cfg(unix)]
    unsafe {
        if libc::kill(pid as i32, libc::SIGKILL) != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::{Foundation::CloseHandle, System::Threading::*};
        let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if handle.is_null() {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let result = TerminateProcess(handle, 17);
        CloseHandle(handle);
        if result == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
    }
    Ok(())
}

fn passed(report: &mut Value, name: &str) {
    report["passed"].as_array_mut().unwrap().push(json!(name));
}

async fn run(report: &mut Value) -> Result<()> {
    let home = tempfile::tempdir().map_err(|e| e.to_string())?;
    let db = setup(home.path())?;
    let normal = launch(&db, "normal", false, false).await?;
    let pid = wait_ready(&normal, "normal", false).await?;
    stop(&normal, pid).await?;
    passed(report, "native_webview_sdk_storage_providers_usage");
    let hung = launch(&db, "hang", true, false).await?;
    let pid = wait_ready(&hung, "hang", false).await?;
    let renderers = descendants(pid)?;
    report["watchdogDescendants"] = json!(renderers);
    wait_state(&hung, "unresponsive", pid).await?;
    for child in renderers {
        wait_exit(child).await?;
    }
    stop(&hung, pid).await?;
    passed(report, "infinite_loop_watchdog_host_remains_responsive");
    let restart = launch(&db, "restart", false, false).await?;
    let pid = wait_ready(&restart, "restart", false).await?;
    let renderers = descendants(pid)?;
    crash(pid)?;
    wait_state(&restart, "failed", pid).await?;
    for child in renderers {
        wait_exit(child).await?;
    }
    stop(&restart, pid).await?;
    passed(
        report,
        "restart_preserves_storage_and_child_crash_is_contained",
    );
    let limited = launch(&db, "limited", false, true).await?;
    let pid = wait_ready(&limited, "limited", true).await?;
    super::super::set_ui_plugin_enabled(ID.into(), false, None).await?;
    wait_state(&limited, "closed", pid).await?;
    stop(&limited, pid).await?;
    passed(report, "permission_denial_and_disable_terminate_runtime");
    parent_crash(report).await?;
    passed(report, "parent_crash_terminates_child");
    Ok(())
}

async fn parent_fixture(root: &Path) -> Result<()> {
    let db = setup(root)?;
    let id = launch(&db, "orphan", false, false).await?;
    let pid = wait_ready(&id, "orphan", false).await?;
    fs::write(
        root.join("ready.json"),
        json!({"pid":pid,"descendants":descendants(pid)?}).to_string(),
    )
    .map_err(|e| e.to_string())?;
    std::future::pending::<()>().await;
    Ok(())
}

async fn parent_crash(report: &mut Value) -> Result<()> {
    let root = tempfile::tempdir().map_err(|e| e.to_string())?;
    let mut parent = Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
        .arg(PARENT_FLAG)
        .arg(root.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(40);
    let ready = loop {
        if let Ok(bytes) = fs::read(root.path().join("ready.json")) {
            let value: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            break value;
        }
        if parent.try_wait().map_err(|e| e.to_string())?.is_some() || Instant::now() >= deadline {
            return Err(format!(
                "Parent fixture failed: {}",
                fs::read_to_string(root.path().join("error.txt")).unwrap_or_default()
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let pid = ready["pid"].as_u64().ok_or("Invalid child PID")? as u32;
    report["parentCrashDescendants"] = ready["descendants"].clone();
    parent.kill().await.map_err(|e| e.to_string())?;
    wait_exit(pid).await?;
    for child in ready["descendants"]
        .as_array()
        .ok_or("Missing descendants")?
    {
        wait_exit(child.as_u64().ok_or("Invalid descendant PID")? as u32).await?;
    }
    Ok(())
}

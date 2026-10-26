//! User-installed UI extensions. Packages are snapshots of self-contained HTML pages;
//! no plugin code runs in Rust or in the application's JavaScript context.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::Mutex,
};

const MAX_HTML: usize = 2 * 1024 * 1024;
const MAX_PACKAGE: usize = 8 * 1024 * 1024;
const MAX_RECORD: usize = MAX_PACKAGE * 6 + 128 * 1024;
const MAX_STORAGE: usize = 64 * 1024;
// ponytail: serialize small local registry writes; split by plugin if contention is measured.
static REGISTRY_LOCK: Mutex<()> = Mutex::new(());

pub(crate) mod read_api;
pub(crate) mod runtime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginPermission {
    #[serde(rename = "providers.read")]
    ProvidersRead,
    #[serde(rename = "usage.read")]
    UsageRead,
}

pub const PAGE_CSP: &str = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data:; font-src data:; connect-src 'none'; frame-src 'none'; worker-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'; sandbox allow-scripts";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginPage {
    pub id: String,
    pub title: String,
    pub entry: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginManifest {
    pub api_version: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    pub pages: Vec<PluginPage>,
    #[serde(default)]
    pub permissions: Vec<PluginPermission>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledPlugin {
    pub manifest: PluginManifest,
    pub enabled: bool,
    pub granted_permissions: Vec<PluginPermission>,
}

#[derive(Debug, Serialize)]
pub struct PluginProblem {
    pub id: String,
    pub error: String,
}

#[derive(Default, Debug, Serialize)]
pub struct PluginList {
    pub plugins: Vec<InstalledPlugin>,
    pub problems: Vec<PluginProblem>,
}

#[derive(Serialize, Deserialize)]
struct PluginRecord {
    manifest: PluginManifest,
    enabled: bool,
    documents: BTreeMap<String, String>,
    storage: Value,
    #[serde(default)]
    granted_permissions: Vec<PluginPermission>,
    #[serde(default)]
    access_revision: String,
}

type Result<T> = std::result::Result<T, String>;

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.as_bytes()[0].is_ascii_lowercase()
        && id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-' || c == b'.')
        && !id.contains("..")
        && !id.ends_with('.')
}

fn validate_manifest(manifest: &PluginManifest) -> Result<()> {
    validate_grants(&manifest.permissions, &manifest.permissions)?;
    if manifest.api_version != 1 {
        return Err("Unsupported plugin API version (expected 1)".into());
    }
    if !valid_id(&manifest.id) {
        return Err("Invalid plugin ID".into());
    }
    if manifest.name.trim().is_empty()
        || manifest.name.len() > 128
        || manifest.version.trim().is_empty()
        || manifest.version.len() > 64
        || manifest.description.len() > 1024
    {
        return Err("Invalid plugin name, version or description".into());
    }
    if manifest.pages.is_empty() || manifest.pages.len() > 10 {
        return Err("A plugin must declare 1–10 pages".into());
    }
    let mut ids = std::collections::HashSet::new();
    for page in &manifest.pages {
        // v1 deliberately accepts single HTML files, not arbitrary directory trees.
        let stem = page.entry.strip_suffix(".html").unwrap_or("");
        if !valid_id(&page.id)
            || !ids.insert(&page.id)
            || page.title.trim().is_empty()
            || page.title.len() > 128
            || stem.is_empty()
            || page.entry.len() > 128
            || !stem
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        {
            return Err("Invalid page: use a unique ID, title and a plain .html filename".into());
        }
    }
    Ok(())
}

fn read_bounded(path: &Path, limit: usize) -> Result<String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.file_type().is_file() || metadata.len() > limit as u64 {
        return Err("Plugin files must be regular files within the size limit".into());
    }
    let mut bytes = Vec::new();
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000); // FILE_FLAG_OPEN_REPARSE_POINT
    }
    let file = options.open(path).map_err(|e| e.to_string())?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Plugin files must be regular files".into());
    }
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > limit {
        return Err("Plugin file exceeds size limit".into());
    }
    String::from_utf8(bytes).map_err(|_| "Plugin files must be UTF-8".into())
}

fn record_path(root: &Path, id: &str) -> Result<PathBuf> {
    if !valid_id(id) {
        return Err("Invalid plugin ID".into());
    }
    // Prefix avoids Windows reserved device filenames, including con/prn/nul.
    Ok(root.join(format!("plugin-{id}.json")))
}

fn read_record(root: &Path, id: &str) -> Result<PluginRecord> {
    let record: PluginRecord =
        serde_json::from_str(&read_bounded(&record_path(root, id)?, MAX_RECORD)?)
            .map_err(|e| e.to_string())?;
    validate_manifest(&record.manifest)?;
    if record.manifest.id != id
        || record.documents.len() != record.manifest.pages.len()
        || record
            .manifest
            .pages
            .iter()
            .any(|p| !record.documents.contains_key(&p.id))
        || record.documents.values().any(|s| s.len() > MAX_HTML)
        || record.documents.values().map(String::len).sum::<usize>() > MAX_PACKAGE
    {
        return Err("Invalid installed plugin snapshot".into());
    }
    validate_storage(&record.storage)?;
    validate_grants(&record.manifest.permissions, &record.granted_permissions)?;
    Ok(record)
}

fn write_record(root: &Path, record: &PluginRecord) -> Result<()> {
    let bytes = serde_json::to_vec(record).map_err(|e| e.to_string())?;
    crate::config::atomic_write_private(&record_path(root, &record.manifest.id)?, &bytes)
        .map_err(|e| e.to_string())
}

fn list(root: &Path) -> Result<PluginList> {
    let mut result = PluginList::default();
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(result),
        Err(e) => return Err(e.to_string()),
    };
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        let Some(id) = name
            .to_str()
            .and_then(|s| s.strip_prefix("plugin-"))
            .and_then(|s| s.strip_suffix(".json"))
        else {
            continue;
        };
        if !valid_id(id) {
            continue;
        }
        match read_record(root, id) {
            Ok(record) => result.plugins.push(InstalledPlugin {
                manifest: record.manifest,
                enabled: record.enabled,
                granted_permissions: record.granted_permissions,
            }),
            Err(error) => result.problems.push(PluginProblem {
                id: id.into(),
                error,
            }),
        }
    }
    result
        .plugins
        .sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
    result.problems.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(result)
}

fn install(root: &Path, directory: &Path) -> Result<InstalledPlugin> {
    let manifest: PluginManifest = serde_json::from_str(&read_bounded(
        &directory.join("cc-switch-plugin.json"),
        32 * 1024,
    )?)
    .map_err(|e| e.to_string())?;
    validate_manifest(&manifest)?;
    if record_path(root, &manifest.id)?
        .try_exists()
        .map_err(|e| e.to_string())?
    {
        return Err(
            "Plugin already installed; uninstall it before installing another version".into(),
        );
    }
    let installed = list(root)?;
    if installed.plugins.len() + installed.problems.len() >= 64 {
        return Err("At most 64 plugins may be installed".into());
    }
    let mut documents = BTreeMap::new();
    let mut size = 0;
    for page in &manifest.pages {
        let html = read_bounded(&directory.join(&page.entry), MAX_HTML)?;
        if html.trim().is_empty() {
            return Err("Plugin page is empty".into());
        }
        size += html.len();
        if size > MAX_PACKAGE {
            return Err("Plugin pages exceed the 8 MiB package limit".into());
        }
        documents.insert(page.id.clone(), html);
    }
    let record = PluginRecord {
        manifest,
        enabled: false,
        documents,
        storage: Value::Null,
        granted_permissions: Vec::new(),
        access_revision: uuid::Uuid::new_v4().to_string(),
    };
    write_record(root, &record)?;
    Ok(InstalledPlugin {
        manifest: record.manifest,
        enabled: false,
        granted_permissions: Vec::new(),
    })
}

fn validate_grants(requested: &[PluginPermission], granted: &[PluginPermission]) -> Result<()> {
    for (index, permission) in granted.iter().enumerate() {
        if !requested.contains(permission) || granted[..index].contains(permission) {
            return Err("Invalid or undeclared plugin permission".into());
        }
    }
    Ok(())
}

fn set_enabled(
    root: &Path,
    id: &str,
    enabled: bool,
    grants: Option<Vec<PluginPermission>>,
) -> Result<()> {
    let mut record = read_record(root, id)?;
    let grants = if enabled {
        if !record.manifest.permissions.is_empty() && grants.is_none() {
            return Err("Plugin permissions require explicit approval".into());
        }
        grants.unwrap_or_default()
    } else {
        Vec::new()
    };
    validate_grants(&record.manifest.permissions, &grants)?;
    record.granted_permissions = grants;
    record.access_revision = uuid::Uuid::new_v4().to_string();
    record.enabled = enabled;
    write_record(root, &record)
}

fn set_permissions(root: &Path, id: &str, grants: Vec<PluginPermission>) -> Result<()> {
    let mut record = read_record(root, id)?;
    if !record.enabled {
        return Err("Plugin is disabled".into());
    }
    validate_grants(&record.manifest.permissions, &grants)?;
    record.granted_permissions = grants;
    record.access_revision = uuid::Uuid::new_v4().to_string();
    write_record(root, &record)
}

fn authorize(root: &Path, id: &str, permission: PluginPermission) -> Result<String> {
    let record = read_record(root, id).map_err(|_| "PLUGIN_UNAVAILABLE")?;
    if !record.enabled {
        return Err("PLUGIN_DISABLED".into());
    }
    if !record.manifest.permissions.contains(&permission)
        || !record.granted_permissions.contains(&permission)
    {
        return Err("PLUGIN_PERMISSION_DENIED".into());
    }
    Ok(record.access_revision)
}

fn check_revision(
    root: &Path,
    id: &str,
    permission: PluginPermission,
    revision: &str,
) -> Result<()> {
    if authorize(root, id, permission)? != revision {
        return Err("PLUGIN_ACCESS_CHANGED".into());
    }
    Ok(())
}

fn validate_storage(value: &Value) -> Result<()> {
    if serde_json::to_vec(value).map_err(|e| e.to_string())?.len() > MAX_STORAGE {
        return Err("Plugin storage exceeds 64 KiB".into());
    }
    Ok(())
}

fn storage(root: &Path, id: &str, value: Option<Value>) -> Result<Value> {
    let mut record = read_record(root, id)?;
    if !record.enabled {
        return Err("Plugin is disabled".into());
    }
    if let Some(value) = value {
        validate_storage(&value)?;
        record.storage = value;
        write_record(root, &record)?;
    }
    Ok(record.storage)
}

fn with_registry<T>(f: impl FnOnce(&Path) -> Result<T>) -> Result<T> {
    let _guard = REGISTRY_LOCK
        .lock()
        .map_err(|_| "Plugin registry lock unavailable")?;
    f(&crate::config::get_app_config_dir().join("ui-plugins"))
}

// Even lock acquisition and config-path lookup can block. Keep the entire registry
// operation off both the UI thread and async executor threads.
async fn with_registry_async<T: Send + 'static>(
    f: impl FnOnce(&Path) -> Result<T> + Send + 'static,
) -> Result<T> {
    tauri::async_runtime::spawn_blocking(move || with_registry(f))
        .await
        .map_err(|error| {
            log::warn!("Plugin registry task failed: {error}");
            "PLUGIN_TASK_FAILED".to_string()
        })?
}

#[tauri::command]
pub async fn list_ui_plugins() -> Result<PluginList> {
    with_registry_async(list).await
}

#[tauri::command]
pub async fn install_ui_plugin(directory: String) -> Result<InstalledPlugin> {
    with_registry_async(move |root| install(root, Path::new(&directory))).await
}

#[tauri::command]
pub async fn set_ui_plugin_enabled(
    id: String,
    enabled: bool,
    granted_permissions: Option<Vec<PluginPermission>>,
) -> Result<()> {
    let plugin_id = id.clone();
    with_registry_async(move |root| set_enabled(root, &id, enabled, granted_permissions)).await?;
    runtime::stop_plugin(&plugin_id);
    Ok(())
}

#[tauri::command]
pub async fn set_ui_plugin_permissions(
    id: String,
    granted_permissions: Vec<PluginPermission>,
) -> Result<()> {
    let plugin_id = id.clone();
    with_registry_async(move |root| set_permissions(root, &id, granted_permissions)).await?;
    runtime::stop_plugin(&plugin_id);
    Ok(())
}

#[tauri::command]
pub async fn uninstall_ui_plugin(id: String) -> Result<()> {
    let plugin_id = id.clone();
    with_registry_async(move |root| {
        fs::remove_file(record_path(root, &id)?).map_err(|e| e.to_string())
    })
    .await?;
    runtime::stop_plugin(&plugin_id);
    Ok(())
}

#[tauri::command]
pub async fn get_ui_plugin_storage(id: String) -> Result<Value> {
    with_registry_async(move |root| storage(root, &id, None)).await
}

#[tauri::command]
pub async fn set_ui_plugin_storage(id: String, value: Value) -> Result<()> {
    with_registry_async(move |root| storage(root, &id, Some(value)).map(|_| ())).await
}

fn page_html(root: &Path, path: &str) -> Result<String> {
    let parts: Vec<_> = path.trim_start_matches('/').split('/').collect();
    if parts.len() != 2 || !valid_id(parts[1]) {
        return Err("Invalid plugin page".into());
    }
    let record = read_record(root, parts[0])?;
    if !record.enabled {
        return Err("Plugin is disabled".into());
    }
    record
        .documents
        .get(parts[1])
        .cloned()
        .ok_or_else(|| "Unknown plugin page".into())
}

#[cfg(test)]
mod tests;

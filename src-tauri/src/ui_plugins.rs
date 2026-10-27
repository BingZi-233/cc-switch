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
    #[serde(default, skip_serializing)]
    documents: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    documents_version: Option<String>,
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

fn documents_path(root: &Path, id: &str, version: &str) -> Result<PathBuf> {
    if !valid_id(id) || uuid::Uuid::parse_str(version).is_err() {
        return Err("Invalid plugin document snapshot".into());
    }
    Ok(root.join(format!("plugin-{id}.documents")).join(version))
}

fn ensure_documents_directory(root: &Path, id: &str) -> Result<PathBuf> {
    if !valid_id(id) {
        return Err("Invalid plugin ID".into());
    }
    let directory = root.join(format!("plugin-{id}.documents"));
    if !fs::symlink_metadata(&directory)
        .map_err(|e| e.to_string())?
        .file_type()
        .is_dir()
    {
        return Err("Invalid plugin document directory".into());
    }
    Ok(directory)
}

fn clean_old_documents(root: &Path, id: &str, current: &str) {
    let Ok(directory) = ensure_documents_directory(root, id) else {
        return;
    };
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name == current || uuid::Uuid::parse_str(name).is_err() {
            continue;
        }
        if let Err(error) = fs::remove_dir_all(entry.path()) {
            log::warn!("Failed to remove old plugin documents for {id}: {error}");
        }
    }
}

fn read_record(root: &Path, id: &str) -> Result<PluginRecord> {
    let mut record: PluginRecord =
        serde_json::from_str(&read_bounded(&record_path(root, id)?, MAX_RECORD)?)
            .map_err(|e| e.to_string())?;
    validate_manifest(&record.manifest)?;
    if let Some(version) = &record.documents_version {
        ensure_documents_directory(root, id)?;
        let directory = documents_path(root, id, version)?;
        if !fs::symlink_metadata(&directory)
            .map_err(|e| e.to_string())?
            .file_type()
            .is_dir()
        {
            return Err("Invalid plugin document snapshot".into());
        }
        record.documents = record
            .manifest
            .pages
            .iter()
            .map(|page| {
                read_bounded(&directory.join(format!("{}.html", page.id)), MAX_HTML)
                    .map(|html| (page.id.clone(), html))
            })
            .collect::<Result<_>>()?;
    }
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
    let path = record_path(root, &record.manifest.id)?;
    let new_version = record
        .documents_version
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let new_snapshot = record.documents_version.is_none();
    let directory = documents_path(root, &record.manifest.id, &new_version)?;
    let mut created_snapshot = false;
    let result = (|| {
        if new_snapshot {
            fs::create_dir_all(root).map_err(|e| e.to_string())?;
            let parent = root.join(format!("plugin-{}.documents", record.manifest.id));
            match fs::create_dir(&parent) {
                Ok(()) => (),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
                Err(error) => return Err(error.to_string()),
            }
            ensure_documents_directory(root, &record.manifest.id)?;
            fs::create_dir(&directory).map_err(|e| e.to_string())?;
            created_snapshot = true;
            for (id, html) in &record.documents {
                crate::config::atomic_write_private(
                    &directory.join(format!("{id}.html")),
                    html.as_bytes(),
                )
                .map_err(|e| e.to_string())?;
            }
        }
        let mut value = serde_json::to_value(record).map_err(|e| e.to_string())?;
        value["documents_version"] = Value::String(new_version.clone());
        let bytes = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
        crate::config::atomic_write_private(&path, &bytes).map_err(|e| e.to_string())
    })();
    if result.is_err() && created_snapshot {
        let _ = fs::remove_dir_all(&directory);
    } else if result.is_ok() {
        clean_old_documents(root, &record.manifest.id, &new_version);
    }
    result
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
            Ok(record) => {
                if let Some(version) = &record.documents_version {
                    clean_old_documents(root, id, version);
                }
                result.plugins.push(InstalledPlugin {
                    manifest: record.manifest,
                    enabled: record.enabled,
                    granted_permissions: record.granted_permissions,
                });
            }
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
    let previous = if record_path(root, &manifest.id)?
        .try_exists()
        .map_err(|e| e.to_string())?
    {
        Some(read_record(root, &manifest.id)?)
    } else {
        let installed = list(root)?;
        if installed.plugins.len() + installed.problems.len() >= 64 {
            return Err("At most 64 plugins may be installed".into());
        }
        None
    };
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
    let (enabled, storage, granted_permissions) = previous
        .map(|record| {
            let grants = record
                .granted_permissions
                .into_iter()
                .filter(|permission| manifest.permissions.contains(permission))
                .collect();
            (record.enabled, record.storage, grants)
        })
        .unwrap_or((false, Value::Null, Vec::new()));
    let record = PluginRecord {
        manifest,
        enabled,
        documents,
        documents_version: None,
        storage,
        granted_permissions,
        access_revision: uuid::Uuid::new_v4().to_string(),
    };
    write_record(root, &record)?;
    Ok(InstalledPlugin {
        manifest: record.manifest,
        enabled: record.enabled,
        granted_permissions: record.granted_permissions,
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

#[cfg(test)]
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
    let installed = with_registry_async(move |root| install(root, Path::new(&directory))).await?;
    runtime::stop_plugin(&installed.manifest.id);
    Ok(installed)
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
    let result = with_registry_async(move |root| {
        fs::remove_file(record_path(root, &id)?).map_err(|e| e.to_string())?;
        let documents = root.join(format!("plugin-{id}.documents"));
        if let Err(error) = fs::remove_dir_all(documents) {
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(format!("Plugin was uninstalled, but its document directory could not be removed: {error}"));
            }
        }
        Ok(())
    })
    .await;
    runtime::stop_plugin(&plugin_id);
    result
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

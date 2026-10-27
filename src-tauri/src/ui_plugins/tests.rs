use super::*;
use serde_json::json;

fn manifest() -> Value {
    json!({
        "apiVersion": 1, "id": "test.fixture", "name": "Test fixture", "version": "1.0.0",
        "pages": [
            { "id": "notes", "title": "Notes", "entry": "index.html" },
            { "id": "about", "title": "About", "entry": "about.html" }
        ]
    })
}

pub(super) fn package(directory: &Path) {
    fs::create_dir_all(directory).unwrap();
    fs::write(
        directory.join("cc-switch-plugin.json"),
        manifest().to_string(),
    )
    .unwrap();
    fs::write(
        directory.join("index.html"),
        "<!doctype html><title>Test fixture</title><h1>Notes</h1>",
    )
    .unwrap();
    fs::write(
        directory.join("about.html"),
        "<!doctype html><h1>About</h1>",
    )
    .unwrap();
}

#[test]
fn lifecycle_persists_snapshots_and_scoped_storage() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let root = temp.path().join("installed");
    package(&source);
    let item = install(&root, &source).unwrap();
    let id = &item.manifest.id;
    assert!(!item.enabled);
    assert!(page_html(&root, &format!("/{id}/notes")).is_err());
    assert!(storage(&root, id, Some(json!("no"))).is_err());
    set_enabled(&root, id, true, None).unwrap();
    let original = page_html(&root, &format!("/{id}/notes")).unwrap();
    fs::write(source.join("index.html"), "changed after installation").unwrap();
    assert_eq!(page_html(&root, &format!("/{id}/notes")).unwrap(), original);
    storage(&root, id, Some(json!({"note":"你好"}))).unwrap();
    assert_eq!(storage(&root, id, None).unwrap(), json!({"note":"你好"}));
    assert!(storage(&root, "missing", None).is_err());
    assert!(storage(&root, id, Some(json!("x".repeat(MAX_STORAGE)))).is_err());
    assert_eq!(storage(&root, id, None).unwrap(), json!({"note":"你好"}));
    set_enabled(&root, id, false, None).unwrap();
    assert!(page_html(&root, &format!("/{id}/notes")).is_err());
    set_enabled(&root, id, true, None).unwrap();
    assert_eq!(list(&root).unwrap().plugins.len(), 1);
    fs::remove_file(record_path(&root, id).unwrap()).unwrap();
    assert!(list(&root).unwrap().plugins.is_empty());
    assert!(source.join("index.html").exists());
}

#[test]
fn pages_are_separate_and_legacy_records_migrate_without_losing_data() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let root = temp.path().join("installed");
    package(&source);
    let id = install(&root, &source).unwrap().manifest.id;
    let path = record_path(&root, &id).unwrap();
    let mut record: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert!(record.get("documents").is_none());
    let version = record["documents_version"].as_str().unwrap();
    let snapshot = documents_path(&root, &id, version).unwrap();
    assert!(snapshot.join("notes.html").exists());
    let orphan = documents_path(&root, &id, &uuid::Uuid::new_v4().to_string()).unwrap();
    fs::create_dir_all(&orphan).unwrap();
    list(&root).unwrap();
    assert!(!orphan.exists());
    assert!(snapshot.join("notes.html").exists());

    // Simulate the old single-file format and migrate on the next state change.
    record["documents"] = json!({
        "notes": fs::read_to_string(snapshot.join("notes.html")).unwrap(),
        "about": fs::read_to_string(snapshot.join("about.html")).unwrap(),
    });
    record["storage"] = json!({"note": "preserved"});
    record.as_object_mut().unwrap().remove("documents_version");
    fs::write(&path, record.to_string()).unwrap();
    set_enabled(&root, &id, true, None).unwrap();
    assert_eq!(
        storage(&root, &id, None).unwrap(),
        json!({"note": "preserved"})
    );
    assert!(page_html(&root, &format!("/{id}/notes"))
        .unwrap()
        .contains("Notes"));
    let migrated: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    assert!(migrated.get("documents").is_none());
    assert!(migrated["documents_version"].is_string());
}

#[test]
fn upgrading_preserves_data_and_existing_grants_without_granting_new_permissions() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let root = temp.path().join("installed");
    package(&source);
    let mut definition = manifest();
    definition["permissions"] = json!(["providers.read"]);
    fs::write(source.join("cc-switch-plugin.json"), definition.to_string()).unwrap();
    let id = install(&root, &source).unwrap().manifest.id;
    set_enabled(
        &root,
        &id,
        true,
        Some(vec![PluginPermission::ProvidersRead]),
    )
    .unwrap();
    storage(&root, &id, Some(json!({"note": "keep"}))).unwrap();
    let revision = read_record(&root, &id).unwrap().access_revision;

    definition["version"] = json!("2.0.0");
    definition["permissions"] = json!(["providers.read", "usage.read"]);
    fs::write(source.join("cc-switch-plugin.json"), definition.to_string()).unwrap();
    fs::write(source.join("index.html"), "updated page").unwrap();
    let upgraded = install(&root, &source).unwrap();
    assert!(upgraded.enabled);
    assert_eq!(upgraded.manifest.version, "2.0.0");
    assert_eq!(
        upgraded.granted_permissions,
        vec![PluginPermission::ProvidersRead]
    );
    assert_eq!(storage(&root, &id, None).unwrap(), json!({"note": "keep"}));
    assert_eq!(
        page_html(&root, &format!("/{id}/notes")).unwrap(),
        "updated page"
    );
    assert_ne!(read_record(&root, &id).unwrap().access_revision, revision);

    fs::remove_file(source.join("about.html")).unwrap();
    assert!(install(&root, &source).is_err());
    assert_eq!(storage(&root, &id, None).unwrap(), json!({"note": "keep"}));
    assert_eq!(
        page_html(&root, &format!("/{id}/notes")).unwrap(),
        "updated page"
    );

    fs::write(source.join("about.html"), "updated about").unwrap();
    definition["permissions"] = json!(["usage.read"]);
    fs::write(source.join("cc-switch-plugin.json"), definition.to_string()).unwrap();
    assert!(install(&root, &source)
        .unwrap()
        .granted_permissions
        .is_empty());
}

#[test]
fn manifest_rejects_traversal_duplicates_and_unknown_capabilities() {
    let base = manifest();
    for id in [
        "../outside",
        "/tmp/a",
        "C:\\a",
        "a/../b",
        "a..b",
        "",
        "a%2fb",
        "a.",
    ] {
        let mut value = base.clone();
        value["id"] = json!(id);
        assert!(validate_manifest(&serde_json::from_value(value).unwrap()).is_err());
        assert!(record_path(Path::new("/tmp/plugins"), id).is_err());
    }
    for entry in [
        "../index.html",
        "/index.html",
        "a\\index.html",
        "a%2findex.html",
        "a.js",
    ] {
        let mut value = base.clone();
        value["pages"][0]["entry"] = json!(entry);
        assert!(validate_manifest(&serde_json::from_value(value).unwrap()).is_err());
    }
    let mut value = base.clone();
    value["apiVersion"] = json!(2);
    assert!(validate_manifest(&serde_json::from_value(value).unwrap()).is_err());
    let mut value = base.clone();
    value["pages"][1]["id"] = value["pages"][0]["id"].clone();
    assert!(validate_manifest(&serde_json::from_value(value).unwrap()).is_err());
    let mut value = base;
    value["permissions"] = json!(["shell"]);
    assert!(serde_json::from_value::<PluginManifest>(value).is_err());
}

#[test]
fn invalid_package_never_leaves_a_partial_install_and_corruption_is_isolated() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let root = temp.path().join("installed");
    package(&source);
    fs::remove_file(source.join("about.html")).unwrap();
    assert!(install(&root, &source).is_err());
    assert!(list(&root).unwrap().plugins.is_empty());
    package(&source);
    let item = install(&root, &source).unwrap();
    fs::write(root.join("plugin-broken.json"), "not json").unwrap();
    let result = list(&root).unwrap();
    assert_eq!(result.plugins.len(), 1);
    assert_eq!(result.problems[0].id, "broken");
    fs::write(source.join("index.html"), "x".repeat(MAX_HTML + 1)).unwrap();
    fs::remove_file(record_path(&root, &item.manifest.id).unwrap()).unwrap();
    assert!(install(&root, &source).is_err());
}

#[cfg(unix)]
#[test]
fn symbolic_links_cannot_import_external_content() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    package(&source);
    fs::write(temp.path().join("secret"), "secret").unwrap();
    fs::remove_file(source.join("index.html")).unwrap();
    std::os::unix::fs::symlink(temp.path().join("secret"), source.join("index.html")).unwrap();
    assert!(install(&temp.path().join("installed"), &source).is_err());
}

#[cfg(unix)]
#[test]
fn document_directory_cannot_be_a_symlink() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let root = temp.path().join("installed");
    let external = temp.path().join("external");
    package(&source);
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&external).unwrap();
    std::os::unix::fs::symlink(&external, root.join("plugin-test.fixture.documents")).unwrap();
    assert!(install(&root, &source).is_err());
    assert_eq!(fs::read_dir(external).unwrap().count(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn slow_registry_work_does_not_block_the_callers_executor() {
    let caller = std::thread::current().id();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let operation = with_registry_async(move |_| {
        assert_ne!(std::thread::current().id(), caller);
        started_tx.send(()).unwrap();
        // Model slow disk I/O while holding the registry lock. A timeout bounds
        // the test if the caller stops making progress.
        release_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .map_err(|error| error.to_string())?;
        Ok(42)
    });
    let heartbeat = async {
        started_rx.await.unwrap();
        tokio::task::yield_now().await;
        release_tx.send(()).unwrap();
    };
    let (result, ()) = tokio::join!(operation, heartbeat);
    assert_eq!(result.unwrap(), 42);
}

use super::*;
use crate::ui_plugins::{self, PluginPermission::*};
use serde_json::json;

#[test]
fn approval_is_explicit_scoped_revocable_and_invalidates_inflight_reads() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let root = temp.path().join("registry");
    ui_plugins::tests::package(&source);
    let path = source.join("cc-switch-plugin.json");
    let mut manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    manifest["permissions"] = json!(["providers.read", "usage.read"]);
    std::fs::write(path, manifest.to_string()).unwrap();
    let installed = ui_plugins::install(&root, &source).unwrap();
    let id = &installed.manifest.id;
    assert!(installed.granted_permissions.is_empty());
    assert!(ui_plugins::set_enabled(&root, id, true, None).is_err());
    ui_plugins::set_enabled(&root, id, true, Some(vec![ProvidersRead])).unwrap();
    let revision = authorize(&root, id, ProvidersRead).unwrap();
    assert_eq!(
        authorize(&root, id, UsageRead).unwrap_err(),
        "PLUGIN_PERMISSION_DENIED"
    );
    assert!(authorize(&root, "another.plugin", ProvidersRead).is_err());
    ui_plugins::set_permissions(&root, id, vec![]).unwrap();
    assert!(check_revision(&root, id, ProvidersRead, &revision).is_err());
    ui_plugins::set_permissions(&root, id, vec![ProvidersRead, UsageRead]).unwrap();
    assert_eq!(
        check_revision(&root, id, ProvidersRead, &revision).unwrap_err(),
        "PLUGIN_ACCESS_CHANGED"
    );
    ui_plugins::set_enabled(&root, id, false, None).unwrap();
    assert!(ui_plugins::read_record(&root, id)
        .unwrap()
        .granted_permissions
        .is_empty());
    assert_eq!(
        authorize(&root, id, UsageRead).unwrap_err(),
        "PLUGIN_DISABLED"
    );
    assert!(ui_plugins::set_permissions(&root, id, vec![UsageRead]).is_err());
    assert!(ui_plugins::validate_grants(&[UsageRead], &[ProvidersRead]).is_err());
    assert!(ui_plugins::validate_grants(&[UsageRead], &[UsageRead, UsageRead]).is_err());
}

#[test]
fn old_installs_remain_compatible_without_implicitly_acquiring_permissions() {
    let temp = tempfile::tempdir().unwrap();
    ui_plugins::tests::package(&temp.path().join("source"));
    let installed =
        ui_plugins::install(&temp.path().join("registry"), &temp.path().join("source")).unwrap();
    let path =
        ui_plugins::record_path(&temp.path().join("registry"), &installed.manifest.id).unwrap();
    let mut record: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    record
        .as_object_mut()
        .unwrap()
        .remove("granted_permissions");
    record.as_object_mut().unwrap().remove("access_revision");
    record["manifest"]
        .as_object_mut()
        .unwrap()
        .remove("permissions");
    record["enabled"] = json!(true);
    std::fs::write(&path, record.to_string()).unwrap();
    assert!(ui_plugins::list(&temp.path().join("registry"))
        .unwrap()
        .problems
        .is_empty());
    assert_eq!(
        authorize(
            &temp.path().join("registry"),
            &installed.manifest.id,
            UsageRead
        )
        .unwrap_err(),
        "PLUGIN_PERMISSION_DENIED"
    );
}

fn fixture() -> Database {
    let db = Database::memory().unwrap();
    let conn = db.conn.lock().unwrap();
    for (id, app, name) in [
        ("p1", "claude", "Claude vendor"),
        ("p2", "codex", "Codex vendor"),
        ("p3", "codex", "Other vendor"),
    ] {
        conn.execute("INSERT INTO providers (id,app_type,name,settings_config,website_url,notes,meta) VALUES (?1,?2,?3,?4,?5,?5,?4)", rusqlite::params![id,app,name,r#"{"apiKey":"SECRET-CREDENTIAL","headers":{"Authorization":"SECRET-CREDENTIAL"}}"#,"SECRET-CREDENTIAL"]).unwrap();
    }
    let now = chrono::Utc::now().timestamp();
    for (id, app, input, output, cost, status) in [
        ("p1", "claude", 100, 20, "0.001", 200),
        ("p2", "codex", 200, 50, "0.002", 500),
    ] {
        conn.execute("INSERT INTO proxy_request_logs (request_id,provider_id,app_type,model,input_tokens,output_tokens,total_cost_usd,latency_ms,status_code,error_message,session_id,created_at) VALUES (?1,?1,?2,'private-model',?3,?4,?5,100,?6,'SECRET-CREDENTIAL','private-session',?7)", rusqlite::params![id,app,input,output,cost,status,now-3600]).unwrap();
    }
    conn.execute_batch("UPDATE proxy_request_logs SET cache_read_tokens = 50, cache_creation_tokens = 10, input_token_semantics = 2; PRAGMA query_only = ON").unwrap();
    drop(conn);
    db
}

#[test]
fn provider_projection_is_paginated_readonly_and_excludes_secrets() {
    let db = fixture();
    let result = providers(
        &db,
        ProviderQuery {
            app_type: Some(AppType::Codex),
            offset: Some(1),
            limit: Some(1),
        },
    )
    .unwrap();
    assert_eq!(result.total, 2);
    assert_eq!(result.items.len(), 1);
    assert_eq!(result.items[0].id, "p3");
    let result = serde_json::to_value(providers(&db, ProviderQuery::default()).unwrap()).unwrap();
    assert!(!result.to_string().contains("SECRET-CREDENTIAL"));
    let mut keys: Vec<_> = result["items"][0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort();
    assert_eq!(keys, ["appType", "category", "id", "name"]);
    assert!(providers(
        &db,
        ProviderQuery {
            limit: Some(201),
            ..Default::default()
        }
    )
    .is_err());
    assert!(serde_json::from_value::<ProviderQuery>(json!({"appType":"unknown"})).is_err());
    assert!(serde_json::from_value::<ProviderQuery>(json!({"includeCredentials":true})).is_err());
}

#[test]
fn usage_reuses_main_screen_totals_and_filters_without_returning_raw_logs() {
    let db = fixture();
    let now = chrono::Utc::now().timestamp();
    let query = || UsageQuery {
        start_date: now - 7 * 86400,
        end_date: now,
        app_type: None,
        provider_name: None,
    };
    let result = usage(&db, query()).unwrap();
    assert_eq!(result.summary.total_requests, 2);
    assert_eq!(result.summary.total_tokens, 490);
    assert_eq!(result.summary.total_cost_usd, "0.003000");
    assert_eq!(result.trends.iter().map(|p| p.requests).sum::<u64>(), 2);
    assert_eq!(result.trends.iter().map(|p| p.tokens).sum::<u64>(), 490);
    let json = serde_json::to_string(&result).unwrap();
    for secret in [
        "SECRET-CREDENTIAL",
        "private-session",
        "private-model",
        "errorMessage",
        "requestId",
    ] {
        assert!(!json.contains(secret));
    }
    let filtered = usage(
        &db,
        UsageQuery {
            app_type: Some(AppType::Claude),
            provider_name: Some("Claude vendor".into()),
            ..query()
        },
    )
    .unwrap();
    assert_eq!(filtered.summary.total_requests, 1);
    assert_eq!(filtered.summary.total_tokens, 180);
    for (start, end) in [
        (-1, now),
        (now, now),
        (now, now - 1),
        (0, now),
        (now, i64::MAX),
    ] {
        assert_eq!(
            usage(
                &db,
                UsageQuery {
                    start_date: start,
                    end_date: end,
                    ..query()
                }
            )
            .unwrap_err(),
            "PLUGIN_INVALID_QUERY"
        );
    }
}

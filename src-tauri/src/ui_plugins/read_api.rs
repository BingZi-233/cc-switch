//! Explicit read-only projections. Never serialize Provider, settingsConfig or raw request logs.
use super::{authorize, check_revision, with_registry, PluginPermission, Result};
use crate::{app_config::AppType, database::Database, store::AppState};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderQuery {
    app_type: Option<AppType>,
    offset: Option<u32>,
    limit: Option<u32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInfo {
    id: String,
    app_type: String,
    name: String,
    category: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderList {
    items: Vec<ProviderInfo>,
    total: u64,
    offset: u32,
    limit: u32,
    source: &'static str,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UsageQuery {
    start_date: i64,
    end_date: i64,
    app_type: Option<AppType>,
    provider_name: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummary {
    total_requests: u64,
    total_tokens: u64,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_creation_tokens: u64,
    total_cost_usd: String,
    success_rate: f32,
    cache_hit_rate: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsagePoint {
    date: String,
    requests: u64,
    tokens: u64,
    cost_usd: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageStats {
    start_date: i64,
    end_date: i64,
    summary: UsageSummary,
    trends: Vec<UsagePoint>,
}

fn read_error(error: impl std::fmt::Display) -> String {
    log::warn!("Plugin read failed: {error}");
    "PLUGIN_READ_FAILED".into()
}

fn providers(db: &Database, query: ProviderQuery) -> Result<ProviderList> {
    let limit = query.limit.unwrap_or(100);
    let offset = query.offset.unwrap_or(0);
    if !(1..=200).contains(&limit) || offset > 100_000 {
        return Err("PLUGIN_INVALID_QUERY".into());
    }
    let app = query.app_type.as_ref().map(AppType::as_str);
    let conn = db.conn.lock().map_err(read_error)?;
    let total = conn
        .query_row(
            "SELECT COUNT(*) FROM providers WHERE (?1 IS NULL OR app_type = ?1)",
            [app],
            |row| row.get(0),
        )
        .map_err(read_error)?;
    // Projection is in SQL: credentials, URLs and free-form notes are never even loaded.
    let mut statement = conn.prepare("SELECT id, app_type, name, category FROM providers WHERE (?1 IS NULL OR app_type = ?1) ORDER BY app_type, COALESCE(sort_index, 999999), id LIMIT ?2 OFFSET ?3").map_err(read_error)?;
    let items = statement
        .query_map(rusqlite::params![app, limit, offset], |row| {
            Ok(ProviderInfo {
                id: row.get(0)?,
                app_type: row.get(1)?,
                name: row.get(2)?,
                category: row.get(3)?,
            })
        })
        .map_err(read_error)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(read_error)?;
    Ok(ProviderList {
        items,
        total,
        offset,
        limit,
        source: "registry",
    })
}

fn usage(db: &Database, query: UsageQuery) -> Result<UsageStats> {
    let now = chrono::Utc::now().timestamp();
    if query.start_date < 0
        || query.end_date <= query.start_date
        || query.end_date - query.start_date > 366 * 86_400
        || query.end_date > now + 86_400
        || query
            .provider_name
            .as_ref()
            .is_some_and(|s| s.is_empty() || s.len() > 256)
    {
        return Err("PLUGIN_INVALID_QUERY".into());
    }
    let app = query.app_type.as_ref().map(AppType::as_str);
    let provider = query.provider_name.as_deref();
    let summary = db
        .get_usage_summary(
            Some(query.start_date),
            Some(query.end_date),
            app,
            provider,
            None,
        )
        .map_err(read_error)?;
    let trends = db
        .get_daily_trends(
            Some(query.start_date),
            Some(query.end_date),
            app,
            provider,
            None,
        )
        .map_err(read_error)?;
    Ok(UsageStats {
        start_date: query.start_date,
        end_date: query.end_date,
        summary: UsageSummary {
            total_requests: summary.total_requests,
            total_tokens: summary.real_total_tokens,
            input_tokens: summary.total_input_tokens,
            output_tokens: summary.total_output_tokens,
            cache_read_tokens: summary.total_cache_read_tokens,
            cache_creation_tokens: summary.total_cache_creation_tokens,
            total_cost_usd: summary.total_cost,
            success_rate: summary.success_rate,
            cache_hit_rate: summary.cache_hit_rate,
        },
        trends: trends
            .into_iter()
            .map(|point| UsagePoint {
                date: point.date,
                requests: point.request_count,
                // DailyStats.total_tokens excludes caches; this API reports processed tokens.
                tokens: point.total_input_tokens
                    + point.total_output_tokens
                    + point.total_cache_creation_tokens
                    + point.total_cache_read_tokens,
                cost_usd: point.total_cost,
            })
            .collect(),
    })
}

async fn read_authorized<T: Send + 'static>(
    state: tauri::State<'_, AppState>,
    id: String,
    permission: PluginPermission,
    query: impl FnOnce(&Database) -> Result<T> + Send + 'static,
) -> Result<T> {
    let db = state.db.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let revision = with_registry(|root| authorize(root, &id, permission))?;
        let result = query(&db)?;
        // Revoking, disabling, or reinstalling also discards responses already in flight.
        with_registry(|root| check_revision(root, &id, permission, &revision))?;
        Ok(result)
    })
    .await
    .map_err(read_error)?
}

#[tauri::command]
pub async fn get_ui_plugin_providers(
    state: tauri::State<'_, AppState>,
    id: String,
    query: ProviderQuery,
) -> Result<ProviderList> {
    read_authorized(state, id, PluginPermission::ProvidersRead, move |db| {
        providers(db, query)
    })
    .await
}

#[tauri::command]
pub async fn get_ui_plugin_usage(
    state: tauri::State<'_, AppState>,
    id: String,
    query: UsageQuery,
) -> Result<UsageStats> {
    read_authorized(state, id, PluginPermission::UsageRead, move |db| {
        usage(db, query)
    })
    .await
}

#[cfg(test)]
mod tests;

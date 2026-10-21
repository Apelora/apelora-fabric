// SPDX-License-Identifier: AGPL-3.0-only
//! Metrics for the installation's own collector (`docs/architecture/MONITORING.md`).
//!
//! - `GET /metrics` on the API address: the text exposition format (OpenMetrics when the
//!   collector asks for it), for callers holding `fabric.metrics.read` — by default the service
//!   identity of the collector with the built-in Metrics Reader role. The safer default: the
//!   same TLS and authentication as the rest of the API, and every refusal is audited.
//! - Optionally the same exposition without authentication on a separate address
//!   (`APELORA_METRICS_ADDR`, off by default), for a collector on loopback or a management
//!   network that cannot hold a token.
//! - `GET /api/v1/monitoring`: how monitoring is set up, for the Console.
//!
//! State read for metrics (pool, job queue, inventory) is refreshed when a collector scrapes,
//! at most every 15 s (database) and 30 s (inventory) per process, never per API request.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use apelora_domain::ProviderMode;
use apelora_metrics as m;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use serde::Serialize;
use utoipa::ToSchema;

use super::authz::{allow_any, allow_platform};
use super::{ApiError, AppState, Auth, ErrorBody, Principal};

/// The permission that opens `GET /metrics`.
pub const PERMISSION: &str = "fabric.metrics.read";
const DB_EVERY: Duration = Duration::from_secs(15);
const INVENTORY_EVERY: Duration = Duration::from_secs(30);
const INVENTORY_BUDGET: Duration = Duration::from_secs(5);
const ACQUIRE_LIMIT: Duration = Duration::from_secs(2);

/// How metrics are exposed by this process.
#[derive(Debug, Clone, Default)]
pub struct MonitoringConfig {
    /// Constant `region`, `site` and `sector` labels.
    pub place: m::Place,
    /// The separate listener without authentication (`APELORA_METRICS_ADDR`); `None`: off.
    pub listener: Option<SocketAddr>,
    /// The worker's listener as this process knows it (`APELORA_WORKER_METRICS_ADDR` when set
    /// in the same environment, otherwise the worker's default).
    pub worker: String,
}

impl MonitoringConfig {
    pub fn from_env(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let place = m::Place::from_env(&get)?;
        let listener = match get("APELORA_METRICS_ADDR")
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty() && v != "off")
        {
            None => None,
            Some(v) => Some(
                v.parse::<SocketAddr>()
                    .map_err(|e| format!("APELORA_METRICS_ADDR={v:?}: {e}"))?,
            ),
        };
        let worker = get("APELORA_WORKER_METRICS_ADDR")
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| m::WORKER_METRICS_DEFAULT_ADDR.to_owned());
        Ok(Self {
            place,
            listener,
            worker,
        })
    }
}

/// The configuration and the scrape-time cache, one per API state.
#[derive(Debug, Default)]
pub struct Monitoring {
    pub config: MonitoringConfig,
    refreshed: tokio::sync::Mutex<Refreshed>,
}

#[derive(Debug, Default)]
struct Refreshed {
    db: Option<Instant>,
    inventory: Option<Instant>,
}

impl Monitoring {
    pub fn new(config: MonitoringConfig) -> Self {
        Self {
            config,
            refreshed: tokio::sync::Mutex::default(),
        }
    }
}

fn label<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".into())
}

/// Counts of a state read in full, as gauge rows.
fn tally(keys: impl Iterator<Item = Vec<String>>) -> BTreeMap<Vec<String>, f64> {
    let mut out: BTreeMap<Vec<String>, f64> = BTreeMap::new();
    for k in keys {
        *out.entry(k).or_insert(0.0) += 1.0;
    }
    out
}

fn rows(t: &BTreeMap<Vec<String>, f64>) -> Vec<(Vec<&str>, f64)> {
    t.iter()
        .map(|(k, v)| (k.iter().map(String::as_str).collect(), *v))
        .collect()
}

async fn refresh_db(pool: &apelora_persistence::Pool) {
    use apelora_persistence::monitoring::{last_backups, pool_sample, queue_summary};
    let s = pool_sample(pool, ACQUIRE_LIMIT).await;
    m::DB_CONNECTIONS.replace([
        (vec!["in_use"], f64::from(s.in_use)),
        (vec!["idle"], f64::from(s.idle)),
        (vec!["max"], f64::from(s.max)),
    ]);
    let Some(secs) = s.acquire_secs else {
        m::DB_UP.set(&[], 0.0);
        tracing::warn!(
            "metrics: no database connection within 2 s; queue figures are from the last good read"
        );
        return;
    };
    m::DB_ACQUIRE.observe(&[], secs);
    m::DB_UP.set(&[], 1.0);
    match queue_summary(pool).await {
        Ok(q) => {
            m::JOBS.replace(q.by_kind.iter().map(|(kind, status, n)| {
                (vec![m::job_kind_label(kind), status.as_str()], *n as f64)
            }));
            m::JOBS_OLDEST_RUNNABLE_AGE.set(&[], q.oldest_runnable_secs);
            m::JOBS_EXPIRED_LEASES.set(&[], q.expired_leases as f64);
        }
        Err(e) => tracing::warn!(error = %e, "metrics: the job queue could not be read"),
    }
    match last_backups(pool).await {
        Ok(b) => m::LAST_BACKUP.replace(b.iter().map(|(p, at)| (vec![p.as_str()], *at))),
        Err(e) => tracing::warn!(error = %e, "metrics: backups could not be read"),
    }
}

async fn refresh_inventory(state: &AppState) {
    let members = state.provider.cluster_members().await;
    let instances = state.provider.instances().await;
    if let Ok(list) = &members {
        let t = tally(list.iter().map(|x| vec![x.status.clone()]));
        m::MEMBERS.replace(rows(&t));
    }
    if let Ok(list) = &instances {
        let t = tally(list.iter().map(|i| vec![i.status.clone(), label(&i.kind)]));
        m::INSTANCES.replace(rows(&t));
    }
    let mut ok = members.is_ok() && instances.is_ok();
    if let Auth::Tokens(pool) = &state.auth {
        match super::manage::status_counts(pool, instances.as_deref().ok()).await {
            Ok(c) => m::APPLICATIONS.replace(c.iter().map(|(s, n)| (vec![s.as_str()], *n as f64))),
            Err(e) => {
                ok = false;
                tracing::warn!(error = %e.message(), "metrics: application status could not be derived");
            }
        }
    }
    if ok {
        m::INVENTORY_LAST_READ.set(&[], m::now_secs());
    }
}

fn certificates(state: &AppState) {
    let mut rows: Vec<(Vec<&str>, f64)> = Vec::new();
    if let Some(e) = state.provider.certificate_expiry() {
        rows.push((vec!["deploy"], e.client as f64));
        rows.push((vec!["server"], e.server as f64));
    }
    if let Some(e) = state.admin.as_deref().and_then(|a| a.certificate_expiry()) {
        rows.push((vec!["admin"], e.client as f64));
    }
    m::CERTIFICATE_EXPIRY.replace(rows);
}

/// Brings the scrape-time families up to date (bounded by the intervals above; concurrent
/// scrapes wait for one refresh instead of each reading the database and Incus).
async fn refresh(state: &AppState) {
    let mut r = state.monitoring.refreshed.lock().await;
    let now = Instant::now();
    if r.db.is_none_or(|t| now.duration_since(t) >= DB_EVERY) {
        r.db = Some(now);
        if let Auth::Tokens(pool) = &state.auth {
            refresh_db(pool).await;
        }
    }
    if r.inventory
        .is_none_or(|t| now.duration_since(t) >= INVENTORY_EVERY)
    {
        r.inventory = Some(now);
        if tokio::time::timeout(INVENTORY_BUDGET, refresh_inventory(state))
            .await
            .is_err()
        {
            tracing::warn!(
                "metrics: the inventory read took longer than 5 s; figures are from the last good read"
            );
        }
    }
    certificates(state);
}

/// The exposition, refreshed, in the format the collector asked for.
async fn exposition(state: &AppState, headers: &HeaderMap) -> Response {
    refresh(state).await;
    let om = m::wants_openmetrics(headers.get(header::ACCEPT).and_then(|v| v.to_str().ok()));
    (
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                if om {
                    m::OPENMETRICS_CONTENT_TYPE
                } else {
                    m::TEXT_CONTENT_TYPE
                },
            ),
            (header::CACHE_CONTROL, "no-store"),
        ],
        m::render(om),
    )
        .into_response()
}

/// Metrics of this control plane in the text exposition format (OpenMetrics when the `Accept`
/// header asks for `application/openmetrics-text`). Needs `fabric.metrics.read` (built-in role
/// Metrics Reader, for the collector's service identity). Labels never carry names, ids, paths
/// with ids, tokens or error texts; see docs/architecture/MONITORING.md.
#[utoipa::path(get, path = "/metrics", tag = "system", operation_id = "metrics",
    responses(
        (status = 200, description = "Metrics in the text exposition format", content_type = "text/plain", body = String),
        (status = 401, body = ErrorBody), (status = 403, body = ErrorBody)))]
pub(super) async fn metrics(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    allow_platform(&p, PERMISSION)?;
    Ok(exposition(&state, &headers).await)
}

/// The separate listener (`APELORA_METRICS_ADDR`): `GET /metrics` without authentication and
/// nothing else. Keep it on loopback or a management network.
pub fn listener_router(state: AppState) -> Router {
    Router::new()
        .route(
            "/metrics",
            get(|State(s): State<AppState>, headers: HeaderMap| async move {
                exposition(&s, &headers).await
            }),
        )
        .with_state(state)
        .layer(middleware::from_fn(super::requests::trace))
}

/// One place a collector scrapes.
#[derive(Serialize, ToSchema, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MetricsEndpoint {
    /// `control-plane`, `control-plane-listener` (separate, unauthenticated) or `worker`.
    pub component: String,
    /// `host:port` for listeners; absent for the API's own address (the one the caller used).
    pub address: Option<String>,
    pub path: String,
    /// `bearer` (a token with `fabric.metrics.read`) or `none` (keep it off public networks).
    pub authentication: String,
}

/// How monitoring is set up, for the Console's Monitoring panel.
#[derive(Serialize, ToSchema, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MonitoringInfo {
    pub region: String,
    pub site: String,
    pub sector: String,
    pub provider: ProviderMode,
    /// The caller holds `fabric.metrics.read` and could scrape `GET /metrics` with its credential.
    pub may_scrape: bool,
    /// The permission a collector needs (`fabric.metrics.read`).
    pub permission: String,
    /// The built-in role that holds only that permission.
    pub role: String,
    pub endpoints: Vec<MetricsEndpoint>,
    /// Media types `GET /metrics` can answer with.
    pub formats: Vec<String>,
    /// Incus's own metrics path on every member, scraped directly with a metrics certificate.
    pub incus_metrics_path: String,
}

/// Where each metrics endpoint is, whether the caller may scrape, and the region, site and
/// sector labels. Needs `fabric.metrics.read` or `cluster.view`.
#[utoipa::path(get, path = "/api/v1/monitoring", tag = "system", operation_id = "monitoring",
    responses((status = 200, body = MonitoringInfo), (status = 401, body = ErrorBody), (status = 403, body = ErrorBody)))]
pub(super) async fn monitoring(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<MonitoringInfo>, ApiError> {
    allow_any(
        &p,
        &[PERMISSION, "cluster.view"],
        &apelora_auth::Resource::platform(),
    )?;
    let c = &state.monitoring.config;
    let mut endpoints = vec![MetricsEndpoint {
        component: "control-plane".into(),
        address: None,
        path: "/metrics".into(),
        authentication: "bearer".into(),
    }];
    if let Some(a) = c.listener {
        endpoints.push(MetricsEndpoint {
            component: "control-plane-listener".into(),
            address: Some(a.to_string()),
            path: "/metrics".into(),
            authentication: "none".into(),
        });
    }
    endpoints.push(MetricsEndpoint {
        component: "worker".into(),
        address: Some(c.worker.clone()),
        path: "/metrics".into(),
        authentication: "none".into(),
    });
    Ok(Json(MonitoringInfo {
        region: c.place.region.clone(),
        site: c.place.site.clone(),
        sector: c.place.sector.clone(),
        provider: state.provider.mode(),
        may_scrape: allow_platform(&p, PERMISSION).is_ok(),
        permission: PERMISSION.into(),
        role: apelora_auth::Builtin::MetricsReader.key().into(),
        endpoints,
        formats: vec![
            m::TEXT_CONTENT_TYPE.into(),
            m::OPENMETRICS_CONTENT_TYPE.into(),
        ],
        incus_metrics_path: "/1.0/metrics".into(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_defaults_and_refusals() {
        let none = |_: &str| None;
        let c = MonitoringConfig::from_env(none).expect("defaults");
        assert_eq!(c.listener, None);
        assert_eq!(c.worker, m::WORKER_METRICS_DEFAULT_ADDR);
        assert_eq!(c.place, m::Place::default());
        let set = |k: &str| match k {
            "APELORA_METRICS_ADDR" => Some("127.0.0.1:9470".to_owned()),
            "APELORA_SECTOR" => Some("s01".to_owned()),
            _ => None,
        };
        let c = MonitoringConfig::from_env(set).expect("valid");
        assert_eq!(c.listener, Some("127.0.0.1:9470".parse().expect("addr")));
        assert_eq!(c.place.sector, "s01");
        let off = |k: &str| (k == "APELORA_METRICS_ADDR").then(|| "off".to_owned());
        assert_eq!(MonitoringConfig::from_env(off).expect("off").listener, None);
        let bad = |k: &str| (k == "APELORA_METRICS_ADDR").then(|| "everywhere".to_owned());
        assert!(MonitoringConfig::from_env(bad).is_err());
        let bad = |k: &str| (k == "APELORA_REGION").then(|| "Europe West".to_owned());
        assert!(MonitoringConfig::from_env(bad).is_err());
    }
}

// SPDX-License-Identifier: AGPL-3.0-only
//! Health and capacity (design: `docs/design/health-and-capacity.md`). Read-only: every
//! number comes from Incus, PostgreSQL or this process; anything that could not be observed is
//! `unknown` with the reason, never a guess. The rules that turn readings into states are pure
//! (`apelora_domain::capacity`), so the Console shows what the server decided.

use std::time::{Duration, Instant};

use apelora_domain::capacity::{
    self as cap, HealthCondition, HealthPoolUsage, HealthQueueCounts, HealthServerResources,
    HealthUsage, HealthWorkerState,
};
use apelora_domain::{ClusterMember, InstanceKind, Permission, ProviderMode, ServerInfo};
use axum::extract::State;
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde::Serialize;
use utoipa::ToSchema;

use super::authz::{allow_platform, filter_for, require};
use super::{ApiError, AppState, Auth, AuthMode, ErrorBody, Principal, Source, source};

/// How long one server's resources may take to read before it is reported as unavailable.
const SERVER_BUDGET: Duration = Duration::from_secs(5);
/// How long a database check may take.
const DATABASE_BUDGET: Duration = Duration::from_secs(3);

fn timestamp(t: time::OffsetDateTime) -> String {
    t.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

fn ms(since: Instant) -> f64 {
    (since.elapsed().as_secs_f64() * 10_000.0).round() / 10.0
}

// --- Components -------------------------------------------------------------------------------

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HealthControlPlane {
    pub state: HealthCondition,
    pub version: String,
    pub api_version: String,
    /// RFC 3339.
    pub started_at: String,
    pub uptime_secs: i64,
    pub auth: AuthMode,
    /// Where inventory comes from: a real Incus server or labelled mock data.
    pub provider: ProviderMode,
    pub reason: Option<String>,
    pub next_step: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HealthDatabase {
    pub state: HealthCondition,
    /// Round trip of `SELECT 1` in milliseconds.
    pub round_trip_ms: Option<f64>,
    /// The newest migration applied.
    pub schema_version: Option<i64>,
    /// The newest migration this build carries.
    pub expected_schema_version: i64,
    pub reason: Option<String>,
    pub next_step: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HealthWorker {
    pub id: String,
    pub host: String,
    pub version: String,
    /// RFC 3339.
    pub started_at: String,
    /// RFC 3339: the last heartbeat (written every 15 s).
    pub last_seen: String,
    pub seconds_since_seen: i64,
    pub state: HealthWorkerState,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HealthWorkers {
    pub state: HealthCondition,
    /// Workers seen in the last day, most recently seen first.
    pub items: Vec<HealthWorker>,
    pub reason: Option<String>,
    pub next_step: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HealthJobQueue {
    pub state: HealthCondition,
    /// `null` when the database could not be read.
    pub counts: Option<HealthQueueCounts>,
    pub reason: Option<String>,
    pub next_step: Option<String>,
}

/// Cluster members by Incus status.
#[derive(Serialize, ToSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct HealthMemberCounts {
    pub total: usize,
    pub online: usize,
    pub evacuated: usize,
    pub offline: usize,
    /// Any other status (for example Blocked).
    pub other: usize,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HealthIncus {
    pub state: HealthCondition,
    pub provider: ProviderMode,
    pub api_version: Option<String>,
    pub server_version: Option<String>,
    pub clustered: Option<bool>,
    /// Round trip of `GET /1.0` in milliseconds.
    pub round_trip_ms: Option<f64>,
    /// `null` when Incus could not be read or is not clustered.
    pub members: Option<HealthMemberCounts>,
    pub reason: Option<String>,
    pub next_step: Option<String>,
}

/// Networks or storage pools: how many, and which are not created everywhere they should be.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HealthInventory {
    pub state: HealthCondition,
    /// `null` when Incus could not be read.
    pub total: Option<usize>,
    /// One line per problem, e.g. `br0: Errored` or `local: missing on node-3`.
    pub problems: Vec<String>,
    pub reason: Option<String>,
    pub next_step: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HealthIdentityProvider {
    pub name: String,
    /// `environment` (set by the control plane's environment) or `console` (saved in the Console).
    pub source: String,
    pub enabled: bool,
    /// Its keys could be fetched at the last check (unreachable ones are retried every minute).
    pub reachable: bool,
    pub error: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HealthIdentityProviders {
    pub state: HealthCondition,
    pub items: Vec<HealthIdentityProvider>,
    /// Whether API tokens are accepted.
    pub api_tokens: bool,
    pub reason: Option<String>,
    pub next_step: Option<String>,
}

/// One state per component of this installation, with the reason and next step when it is not
/// healthy.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HealthReport {
    /// RFC 3339: when this report was made.
    pub checked_at: String,
    /// The worst component state (`not-set-up` counts as healthy).
    pub overall: HealthCondition,
    pub control_plane: HealthControlPlane,
    pub database: HealthDatabase,
    pub workers: HealthWorkers,
    pub job_queue: HealthJobQueue,
    pub incus: HealthIncus,
    pub networks: HealthInventory,
    pub storage_pools: HealthInventory,
    pub identity_providers: HealthIdentityProviders,
}

fn control_plane(state: &AppState) -> HealthControlPlane {
    let now = jiff::Timestamp::now();
    let uptime = now.duration_since(state.started_at).as_secs().max(0);
    let dev = matches!(state.auth, Auth::DisabledForDevelopment);
    HealthControlPlane {
        state: if dev {
            HealthCondition::Degraded
        } else {
            HealthCondition::Healthy
        },
        version: env!("CARGO_PKG_VERSION").into(),
        api_version: apelora_domain::API_VERSION.into(),
        started_at: state.started_at.to_string(),
        uptime_secs: uptime,
        auth: if dev {
            AuthMode::DisabledDevelopment
        } else {
            AuthMode::Tokens
        },
        provider: state.provider.mode(),
        reason: dev.then(|| {
            "Sign-in is off (development mode): anyone who reaches the API acts as an administrator"
                .into()
        }),
        next_step: dev
            .then(|| "Start the control plane with DATABASE_URL set to require sign-in".into()),
    }
}

fn unknown_db() -> String {
    "Needs the database, which could not be read".into()
}

async fn database(
    pool: &apelora_persistence::Pool,
) -> (
    HealthDatabase,
    Option<apelora_persistence::status::DatabaseCheck>,
) {
    let expected = apelora_persistence::status::expected_schema_version();
    let down = |reason: String, next: &str| HealthDatabase {
        state: HealthCondition::Down,
        round_trip_ms: None,
        schema_version: None,
        expected_schema_version: expected,
        reason: Some(reason),
        next_step: Some(next.into()),
    };
    let check = match tokio::time::timeout(
        DATABASE_BUDGET,
        apelora_persistence::status::database_check(pool),
    )
    .await
    {
        Err(_) => {
            tracing::warn!(
                budget_secs = DATABASE_BUDGET.as_secs(),
                "health: the database did not answer in time"
            );
            return (
                down(
                    format!(
                        "The database did not answer within {} s",
                        DATABASE_BUDGET.as_secs()
                    ),
                    "Check that PostgreSQL is running and not overloaded",
                ),
                None,
            );
        }
        Ok(Err(e)) if e.is_unavailable() => {
            tracing::warn!(error = %e, "health: the database is not reachable");
            return (
                down(
                    "The database is not reachable".into(),
                    "Check DATABASE_URL and that PostgreSQL is running",
                ),
                None,
            );
        }
        Ok(Err(e)) => {
            tracing::error!(error = %e, "health: the database refused the check");
            return (
                down(
                    "The database refused the check; the control plane's log has the details"
                        .into(),
                    "Read the control plane's log",
                ),
                None,
            );
        }
        Ok(Ok(c)) => c,
    };
    let (state, reason, next) = match check.schema_version {
        Some(v) if v < expected => (
            HealthCondition::Down,
            Some(format!(
                "The database schema is at migration {v}; this control plane needs {expected}"
            )),
            Some("Restart the control plane: it applies its migrations when it starts".into()),
        ),
        None => (
            HealthCondition::Down,
            Some(format!(
                "No migrations are recorded; this control plane needs {expected}"
            )),
            Some("Restart the control plane: it applies its migrations when it starts".into()),
        ),
        Some(v) if v > expected => (
            HealthCondition::Degraded,
            Some(format!(
                "The database schema is at migration {v}, newer than this control plane ({expected}): a newer control plane or worker uses it"
            )),
            Some("Upgrade this control plane to the same version".into()),
        ),
        Some(_) if check.round_trip_ms > cap::DATABASE_SLOW_MS => (
            HealthCondition::Degraded,
            Some(format!(
                "A trivial query took {:.0} ms",
                check.round_trip_ms
            )),
            Some("Check the database server's load and the network to it".into()),
        ),
        Some(_) => (HealthCondition::Healthy, None, None),
    };
    (
        HealthDatabase {
            state,
            round_trip_ms: Some((check.round_trip_ms * 10.0).round() / 10.0),
            schema_version: check.schema_version,
            expected_schema_version: expected,
            reason,
            next_step: next,
        },
        Some(check),
    )
}

async fn workers_and_queue(
    pool: Option<&apelora_persistence::Pool>,
    db_ok: bool,
) -> (HealthWorkers, HealthJobQueue) {
    let unknown = |reason: String| {
        (
            HealthWorkers {
                state: HealthCondition::Unknown,
                items: Vec::new(),
                reason: Some(reason.clone()),
                next_step: None,
            },
            HealthJobQueue {
                state: HealthCondition::Unknown,
                counts: None,
                reason: Some(reason),
                next_step: None,
            },
        )
    };
    let Some(pool) = pool else {
        let mut w = unknown(
            "Workers report through the database; there is none in development mode".into(),
        );
        w.0.state = HealthCondition::NotSetUp;
        w.1.state = HealthCondition::NotSetUp;
        return w;
    };
    if !db_ok {
        return unknown(unknown_db());
    }
    let (workers, counts) = tokio::join!(
        apelora_persistence::status::workers(pool),
        apelora_persistence::status::queue_counts(pool)
    );
    let (Ok(workers), Ok(c)) = (workers, counts) else {
        tracing::warn!("health: worker heartbeats or job counts could not be read");
        return unknown(unknown_db());
    };
    let items: Vec<HealthWorker> = workers
        .into_iter()
        .map(|w| HealthWorker {
            state: cap::worker_state(w.seconds_since_seen, w.stopped_at.is_some()),
            id: w.id,
            host: w.host,
            version: w.version,
            started_at: timestamp(w.started_at),
            last_seen: timestamp(w.last_seen),
            seconds_since_seen: w.seconds_since_seen,
        })
        .collect();
    let seen: Vec<(HealthWorkerState, i64)> = items
        .iter()
        .map(|w| (w.state, w.seconds_since_seen))
        .collect();
    let (ws, wreason) = cap::workers_state(&seen);
    let running = items.iter().any(|w| w.state == HealthWorkerState::Running);
    let counts = HealthQueueCounts {
        waiting: c.waiting,
        retrying: c.retrying,
        running: c.running,
        stalled: c.stalled,
        failed_last_hour: c.failed_last_hour,
        oldest_waiting_secs: c.oldest_waiting_secs,
    };
    let (qs, qreason) = cap::queue_state(&counts, running);
    (
        HealthWorkers {
            state: ws,
            next_step: (ws != HealthCondition::Healthy).then(|| {
                "Start the deployment worker (apelora-deployment-worker) with the same DATABASE_URL, and read its log".into()
            }),
            items,
            reason: wreason,
        },
        HealthJobQueue {
            state: qs,
            counts: Some(counts),
            next_step: match qs {
                // The workers' row already says how to start one.
                HealthCondition::Down => None,
                HealthCondition::Degraded => Some("Open Operations to see the jobs and their errors".into()),
                _ => None,
            },
            reason: qreason,
        },
    )
}

struct IncusReading {
    health: HealthIncus,
    networks: HealthInventory,
    pools: HealthInventory,
}

fn incus_down(provider: ProviderMode, reason: String) -> IncusReading {
    let needs = || HealthInventory {
        state: HealthCondition::Unknown,
        total: None,
        problems: Vec::new(),
        reason: Some("Needs Incus, which could not be read".into()),
        next_step: None,
    };
    IncusReading {
        health: HealthIncus {
            state: HealthCondition::Down,
            provider,
            api_version: None,
            server_version: None,
            clustered: None,
            round_trip_ms: None,
            members: None,
            reason: Some(reason),
            next_step: Some(
                "Check that Incus runs and that the control plane can reach it (docs/runbooks/troubleshooting.md)".into(),
            ),
        },
        networks: needs(),
        pools: needs(),
    }
}

/// One sentence per failed server about its fencing (HA slice B), from the worker's records.
async fn fencing_note(state: &AppState) -> Option<String> {
    let super::Auth::Tokens(pool) = &state.auth else {
        return None;
    };
    let failures = apelora_persistence::recovery::open_failures(pool)
        .await
        .map_err(|e| tracing::debug!(error = %e, "health: failed servers not readable"))
        .ok()?;
    let parts: Vec<String> = failures
        .iter()
        .map(|f| {
            if f.fenced_at.is_some() {
                format!(
                    "{} is fenced; its modules on shared storage restart on other servers",
                    f.member
                )
            } else {
                let why = f
                    .fencing
                    .as_ref()
                    .and_then(|v| v.get("reason"))
                    .and_then(|r| r.as_str())
                    .unwrap_or("the supervisor has not looked at it yet");
                format!("{} is not fenced yet: {why}", f.member)
            }
        })
        .collect();
    (!parts.is_empty()).then(|| parts.join(". "))
}

async fn incus(state: &AppState) -> IncusReading {
    let pr = &state.provider;
    let mode = pr.mode();
    let started = Instant::now();
    let info: ServerInfo = match pr.server_info().await {
        Ok(i) => i,
        Err(e) => {
            tracing::warn!(error = %e, "health: Incus is not reachable");
            return incus_down(mode, e.to_string());
        }
    };
    let round_trip = ms(started);
    let (members, networks, pools) =
        tokio::join!(pr.cluster_members(), pr.networks(), pr.storage_pools());
    let members = match members {
        Ok(m) => m,
        Err(e) => {
            return incus_down(
                mode,
                format!("The cluster's members could not be read: {e}"),
            );
        }
    };
    let mut counts = HealthMemberCounts {
        total: members.len(),
        ..Default::default()
    };
    for m in &members {
        match m.status.as_str() {
            "Online" => counts.online += 1,
            "Evacuated" => counts.evacuated += 1,
            "Offline" => counts.offline += 1,
            _ => counts.other += 1,
        }
    }
    let not_online = counts.total - counts.online;
    let (state, reason, next) = if mode == ProviderMode::Mock {
        (
            HealthCondition::NotSetUp,
            Some("Mock data: no Incus server is configured".into()),
            Some("Set APELORA_INCUS_URL and the certificate paths to read a real cluster".into()),
        )
    } else if not_online > 0 {
        let names: Vec<String> = members
            .iter()
            .filter(|m| m.status != "Online")
            .map(|m| format!("{} is {}", m.name, m.status))
            .collect();
        let fencing = fencing_note(state).await;
        (
            HealthCondition::Degraded,
            Some(format!(
                "{not_online} of {} servers not online: {}{}",
                counts.total,
                names.join(", "),
                fencing
                    .as_deref()
                    .map(|f| format!(". {f}"))
                    .unwrap_or_default()
            )),
            Some(if counts.offline > 0 {
                "Servers → Members shows whether each failed server is fenced and which modules restart on other servers; check its power and network".into()
            } else {
                "See the servers below; Servers → Members has evacuate and restore".into()
            }),
        )
    } else {
        (HealthCondition::Healthy, None, None)
    };
    let member_names: Vec<String> = members.iter().map(|m| m.name.clone()).collect();
    let inventory = |problems: Vec<String>, total: usize, what: &str| HealthInventory {
        state: cap::inventory_state(&problems),
        total: Some(total),
        reason: (!problems.is_empty())
            .then(|| format!("{} {what} not created everywhere", problems.len())),
        next_step: (!problems.is_empty()).then(|| {
            "Open Networks or Storage, then check the Incus log on the named servers".into()
        }),
        problems,
    };
    let unreadable = |e: apelora_incus_adapter::AdapterError, what: &str| HealthInventory {
        state: HealthCondition::Unknown,
        total: None,
        problems: Vec::new(),
        reason: Some(format!("{what} could not be read: {e}")),
        next_step: None,
    };
    let networks = match networks {
        Ok(list) => {
            let managed: Vec<_> = list.iter().filter(|n| n.managed).collect();
            let mut problems = Vec::new();
            for n in &managed {
                if n.status != "Created" {
                    problems.push(format!("{} ({}): {}", n.name, n.project, n.status));
                }
                let missing = cap::missing_on(&n.locations, &member_names, &n.kind);
                if !missing.is_empty() {
                    problems.push(format!(
                        "{} ({}): missing on {}",
                        n.name,
                        n.project,
                        missing.join(", ")
                    ));
                }
            }
            inventory(problems, managed.len(), "networks")
        }
        Err(e) => unreadable(e, "Networks"),
    };
    let pools = match pools {
        Ok(list) => {
            let mut problems = Vec::new();
            for p in &list {
                if p.status != "Created" {
                    problems.push(format!("{}: {}", p.name, p.status));
                }
                let missing = cap::missing_on(&p.locations, &member_names, &p.driver);
                if !missing.is_empty() {
                    problems.push(format!("{}: missing on {}", p.name, missing.join(", ")));
                }
            }
            inventory(problems, list.len(), "storage pools")
        }
        Err(e) => unreadable(e, "Storage pools"),
    };
    IncusReading {
        health: HealthIncus {
            state,
            provider: mode,
            api_version: Some(info.api_version),
            server_version: info.server_version,
            clustered: Some(info.clustered),
            round_trip_ms: Some(round_trip),
            members: info.clustered.then_some(counts),
            reason,
            next_step: next,
        },
        networks,
        pools,
    }
}

fn identity_providers(state: &AppState) -> HealthIdentityProviders {
    let api_tokens = state.api_tokens_accepted();
    if matches!(state.auth, Auth::DisabledForDevelopment) {
        return HealthIdentityProviders {
            state: HealthCondition::NotSetUp,
            items: Vec::new(),
            api_tokens,
            reason: Some("Sign-in is off in development mode".into()),
            next_step: None,
        };
    }
    let mut items = Vec::new();
    if state.oidc.is_some() {
        items.push(HealthIdentityProvider {
            name: state
                .sign_in
                .as_deref()
                .map_or_else(|| "Environment".to_owned(), |s| s.label.clone()),
            source: "environment".into(),
            enabled: true,
            reachable: true,
            error: None,
        });
    }
    let runtime = state
        .auth_runtime
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    items.extend(runtime.providers.iter().map(|p| HealthIdentityProvider {
        name: p.record.name.clone(),
        source: "console".into(),
        enabled: p.record.enabled,
        reachable: p.verifier.is_some(),
        error: p.error.clone(),
    }));
    let unreachable: Vec<&str> = items
        .iter()
        .filter(|p| p.enabled && !p.reachable)
        .map(|p| p.name.as_str())
        .collect();
    let (state, reason, next) = if !unreachable.is_empty() {
        (
            HealthCondition::Degraded,
            Some(format!(
                "Not reachable: {}. People who sign in with {} cannot sign in until it answers (retried every minute)",
                unreachable.join(", "),
                if unreachable.len() == 1 { "it" } else { "them" }
            )),
            Some("Open Identity & Access → Sign-in and check the provider".into()),
        )
    } else if !items.iter().any(|p| p.enabled) {
        (
            HealthCondition::NotSetUp,
            Some(if api_tokens {
                "No identity provider is set up; people sign in with API tokens".into()
            } else {
                "No identity provider is set up and API tokens are off".into()
            }),
            None,
        )
    } else {
        (HealthCondition::Healthy, None, None)
    };
    HealthIdentityProviders {
        state,
        items,
        api_tokens,
        reason,
        next_step: next,
    }
}

/// The state of each part of this installation: control plane, database, deployment workers,
/// job queue, Incus, networks, storage pools and identity providers. Read-only; needs
/// `cluster.view`.
#[utoipa::path(get, path = "/api/v1/health/components", tag = "system", operation_id = "health_components",
    responses((status = 200, body = HealthReport), (status = 401, body = ErrorBody),
    (status = 403, body = ErrorBody)))]
pub(super) async fn components(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<HealthReport>, ApiError> {
    require(&p, Permission::ReadInventory)?;
    let pool = match &state.auth {
        Auth::Tokens(pool) => Some(pool),
        Auth::DisabledForDevelopment => None,
    };
    let db = async {
        match pool {
            Some(pool) => {
                let (h, check) = database(pool).await;
                let ok = check.is_some();
                (h, ok)
            }
            None => (
                HealthDatabase {
                    state: HealthCondition::NotSetUp,
                    round_trip_ms: None,
                    schema_version: None,
                    expected_schema_version: apelora_persistence::status::expected_schema_version(),
                    reason: Some("No database in development mode".into()),
                    next_step: None,
                },
                false,
            ),
        }
    };
    let ((database, db_ok), reading) = tokio::join!(db, incus(&state));
    let (workers, job_queue) = workers_and_queue(pool, db_ok).await;
    let control_plane = control_plane(&state);
    let identity_providers = identity_providers(&state);
    let overall = HealthCondition::worst([
        control_plane.state,
        database.state,
        workers.state,
        job_queue.state,
        reading.health.state,
        reading.networks.state,
        reading.pools.state,
        identity_providers.state,
    ]);
    Ok(Json(HealthReport {
        checked_at: jiff::Timestamp::now().to_string(),
        overall,
        control_plane,
        database,
        workers,
        job_queue,
        incus: reading.health,
        networks: reading.networks,
        storage_pools: reading.pools,
        identity_providers,
    }))
}

// --- Servers ----------------------------------------------------------------------------------

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HealthServerInstance {
    pub name: String,
    pub project: String,
    pub kind: InstanceKind,
    pub status: String,
    /// Created and owned by Apelora.
    pub managed: bool,
}

/// One server ("minion"): an Incus cluster member, or the standalone Incus server.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HealthServer {
    pub name: String,
    /// Incus status: Online, Evacuated, Offline, Blocked, …
    pub status: String,
    /// Incus's message for the status.
    pub message: String,
    pub state: HealthCondition,
    pub roles: Vec<String>,
    pub architecture: String,
    pub failure_domain: String,
    /// Kept out of automatic placement (`scheduler.instance=manual`).
    pub held: bool,
    /// `null` when they could not be read (see `resourcesError`).
    pub resources: Option<HealthServerResources>,
    pub resources_error: Option<String>,
    pub instances: Vec<HealthServerInstance>,
    /// Warnings decided by the server (memory above 90 %, a pool above 85 %, CPU busy, …).
    pub warnings: Vec<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HealthServers {
    pub source: Source,
    pub clustered: bool,
    pub items: Vec<HealthServer>,
}

/// A server and what was read about its resources.
pub(super) struct Reading {
    pub(super) member: ClusterMember,
    pub(super) resources: Result<HealthServerResources, String>,
}

/// The servers (members, or the standalone server) with their resources, read concurrently,
/// each within [`SERVER_BUDGET`].
pub(super) async fn read_servers(state: &AppState) -> Result<(bool, Vec<Reading>), ApiError> {
    let info = state.provider.server_info().await?;
    let members = if info.clustered {
        state.provider.cluster_members().await?
    } else {
        vec![ClusterMember {
            name: "Incus server".into(),
            url: String::new(),
            roles: Vec::new(),
            status: "Online".into(),
            message: "Not clustered".into(),
            architecture: info.architectures.first().cloned().unwrap_or_default(),
            failure_domain: String::new(),
            database: false,
            groups: Vec::new(),
            scheduler: "all".into(),
        }]
    };
    let mut set = tokio::task::JoinSet::new();
    for (i, m) in members.iter().enumerate() {
        let provider = state.provider.clone();
        let target = info.clustered.then(|| m.name.clone());
        set.spawn(async move {
            let r = match tokio::time::timeout(
                SERVER_BUDGET,
                provider.server_resources(target.as_deref()),
            )
            .await
            {
                Ok(Ok(r)) => Ok(r),
                Ok(Err(e)) => Err(e.to_string()),
                Err(_) => Err(format!("no answer within {} s", SERVER_BUDGET.as_secs())),
            };
            (i, r)
        });
    }
    let mut results: Vec<Option<Result<HealthServerResources, String>>> = vec![None; members.len()];
    while let Some(done) = set.join_next().await {
        match done {
            Ok((i, r)) => results[i] = Some(r),
            Err(e) => tracing::error!(error = %e, "health: a server read panicked"),
        }
    }
    Ok((
        info.clustered,
        members
            .into_iter()
            .zip(results)
            .map(|(member, r)| {
                let resources = r.unwrap_or_else(|| Err("the read did not finish".into()));
                if let Err(e) = &resources {
                    tracing::debug!(member = %member.name, error = %e, "health: resources not readable");
                }
                Reading { member, resources }
            })
            .collect(),
    ))
}

/// Each server's status, CPU, memory, storage and the instances on it, with warnings. Read-only;
/// needs `cluster.view`.
#[utoipa::path(get, path = "/api/v1/health/servers", tag = "system", operation_id = "health_servers",
    responses((status = 200, body = HealthServers), (status = 401, body = ErrorBody),
    (status = 403, body = ErrorBody), (status = 502, body = ErrorBody)))]
pub(super) async fn servers(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<HealthServers>, ApiError> {
    require(&p, Permission::ReadInventory)?;
    let ((clustered, readings), instances) = tokio::try_join!(read_servers(&state), async {
        state.provider.instances().await.map_err(ApiError::from)
    })?;
    let items = readings
        .into_iter()
        .map(|Reading { member, resources }| {
            let (st, warnings) =
                cap::server_state(&member.status, resources.as_ref().map_err(String::as_str));
            let on_it = instances
                .iter()
                .filter(|i| !clustered || i.location == member.name)
                .map(|i| HealthServerInstance {
                    name: i.name.clone(),
                    project: i.project.clone(),
                    kind: i.kind,
                    status: i.status.clone(),
                    managed: i.managed,
                })
                .collect();
            HealthServer {
                held: member.scheduler == apelora_domain::enrolment::HELD,
                state: st,
                name: member.name,
                status: member.status,
                message: member.message,
                roles: member.roles,
                architecture: member.architecture,
                failure_domain: member.failure_domain,
                resources_error: resources.as_ref().err().cloned(),
                resources: resources.ok(),
                instances: on_it,
                warnings,
            }
        })
        .collect();
    Ok(Json(HealthServers {
        source: source(state.provider.mode()),
        clustered,
        items,
    }))
}

// --- Capacity (Studio) ------------------------------------------------------------------------

/// One server's free capacity, as the Studio compares it with a design.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CapacityServer {
    /// The server's name for callers with `cluster.view`; otherwise `Server 1`, `Server 2`, …
    pub label: String,
    /// Online, not held out of placement, and its resources could be read.
    pub accepts_modules: bool,
    /// Why it does not accept new modules.
    pub reason: Option<String>,
    pub architecture: String,
    pub cpu_threads: Option<u64>,
    /// 1-minute load average (clustered servers only).
    pub load: Option<f64>,
    pub memory: Option<HealthUsage>,
    /// Space in the pool volumes are created in (`APELORA_VOLUME_POOL`), on this server.
    pub volume_pool: Option<HealthUsage>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CapacityTotals {
    /// Servers that accept new modules; the sums below are over these.
    pub servers_accepting: usize,
    pub cpu_threads: u64,
    pub memory_free_bytes: u64,
    pub memory_total_bytes: u64,
    /// `null` when no volume pool is configured. A shared pool (one pool every server uses)
    /// is counted once.
    pub volume_pool_free_bytes: Option<u64>,
}

/// Free capacity of the cluster for the Studio: per server and in total.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CapacityReport {
    pub source: Source,
    /// Server names are shown (the caller holds `cluster.view`); otherwise servers are numbered.
    pub named: bool,
    /// The pool volumes are created in; `null`: designs with volumes cannot be deployed.
    pub volume_pool: Option<String>,
    /// The volume pool is one pool all servers share (its free space is one budget, not one
    /// per server).
    pub volume_pool_shared: bool,
    pub servers: Vec<CapacityServer>,
    pub totals: CapacityTotals,
}

fn pool_usage(pools: &[HealthPoolUsage], name: Option<&str>) -> Option<HealthUsage> {
    let name = name?;
    pools.iter().find(|p| p.name == name).map(|p| HealthUsage {
        used_bytes: p.used_bytes,
        total_bytes: p.total_bytes,
    })
}

/// How much CPU, memory and volume space the cluster has free, per server and in total, for
/// comparing with a design. Read-only. With `cluster.view` servers are named; with
/// `capacity.view` (held by designers and operators at any scope) they are numbered.
#[utoipa::path(get, path = "/api/v1/capacity", tag = "inventory", operation_id = "capacity",
    responses((status = 200, body = CapacityReport), (status = 401, body = ErrorBody),
    (status = 403, body = ErrorBody), (status = 502, body = ErrorBody)))]
pub(super) async fn capacity(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<CapacityReport>, ApiError> {
    let named = allow_platform(&p, "cluster.view").is_ok();
    if !named && filter_for(&p, "capacity.view").is_empty() {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "you may not see the cluster's capacity (capacity.view)",
        ));
    }
    let (_, readings) = read_servers(&state).await?;
    let pool = state.volume_pool.as_deref();
    let shared = match pool {
        Some(name) => match state.provider.storage_pools().await {
            Ok(pools) => pools
                .iter()
                .any(|p| p.name == name && cap::pool_is_shared(&p.driver)),
            Err(e) => {
                tracing::debug!(error = %e, "capacity: storage pools not readable; volume pool taken as per server");
                false
            }
        },
        None => false,
    };
    let mut shared_free: Option<u64> = None;
    let mut totals = CapacityTotals {
        servers_accepting: 0,
        cpu_threads: 0,
        memory_free_bytes: 0,
        memory_total_bytes: 0,
        volume_pool_free_bytes: pool.map(|_| 0),
    };
    let servers = readings
        .into_iter()
        .enumerate()
        .map(|(i, Reading { member, resources })| {
            let reason = match (&resources, member.status.as_str()) {
                (_, "Online") if member.scheduler == apelora_domain::enrolment::HELD => {
                    Some("Held out of automatic placement".to_owned())
                }
                (Err(_), "Online") => Some("Its resources could not be read".to_owned()),
                (_, "Online") => None,
                (_, other) => Some(format!("It is {other}")),
            };
            let r = resources.ok();
            let volume_pool = r.as_ref().and_then(|r| pool_usage(&r.pools, pool));
            if reason.is_none()
                && let Some(r) = &r
            {
                totals.servers_accepting += 1;
                totals.cpu_threads += r.cpu_threads.unwrap_or_default();
                if let Some(m) = r.memory {
                    totals.memory_free_bytes += m.free_bytes();
                    totals.memory_total_bytes += m.total_bytes;
                }
                if let (Some(t), Some(v)) = (totals.volume_pool_free_bytes.as_mut(), volume_pool) {
                    if shared {
                        shared_free = Some(shared_free.unwrap_or(0).max(v.free_bytes()));
                    } else {
                        *t += v.free_bytes();
                    }
                }
            }
            CapacityServer {
                label: if named {
                    member.name.clone()
                } else {
                    format!("Server {}", i + 1)
                },
                accepts_modules: reason.is_none(),
                reason,
                architecture: member.architecture,
                cpu_threads: r.as_ref().and_then(|r| r.cpu_threads),
                load: r.as_ref().and_then(|r| r.load_averages.first().copied()),
                memory: r.as_ref().and_then(|r| r.memory),
                volume_pool,
            }
        })
        .collect();
    if shared {
        totals.volume_pool_free_bytes = pool.map(|_| shared_free.unwrap_or(0));
    }
    Ok(Json(CapacityReport {
        source: source(state.provider.mode()),
        named,
        volume_pool: pool.map(str::to_owned),
        volume_pool_shared: shared,
        servers,
        totals,
    }))
}

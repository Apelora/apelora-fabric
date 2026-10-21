// SPDX-License-Identifier: AGPL-3.0-only
//! The Console's Storage area (docs/architecture/STORAGE.md, docs/development/CONSOLE-PLAN.md):
//! pools with their use, every application volume with its schedules and backups, the custom
//! volumes in Apelora's projects that no application references (orphans, read-only), and per
//! volume: its backups, a backup on request, its schedules and a restore from a backup.
//!
//! A restore is a `volume.restore` job (deployment worker, `storage.rs`): the dry run here says
//! what it replaces (the data written since the backup and the volume's own snapshots), the
//! request runs the same check again, needs the volume's name typed and `application.operate`
//! (and an approval where a policy asks), and queues the job together with its audit row.

use std::collections::{BTreeMap, BTreeSet};

use apelora_auth::Resource;
use apelora_domain::capacity::{HealthUsage, pool_is_shared};
use apelora_domain::storage::{RestorePlan, RestoreSubject, Schedule, plan_restore};
use apelora_persistence::{BackupRecord, Enqueued, JobOrigin, VolumeRecord};
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use super::admin::rfc3339;
use super::authz::{allow, allow_platform, audit_scoped, guard};
use super::deploy::{JobAccepted, bad_request, idempotency_key, store};
use super::manage::load;
use super::volumes::{VolumeUse, instances_using, member_of};
use super::{ApiError, AppState, ErrorBody, Principal, Source, source};

type Res<T> = Result<T, ApiError>;

fn not_found(what: String) -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "not_found",
        format!("not found: {what}"),
    )
}

/// The application's resource for authorisation: its scope when it has one, otherwise the
/// platform (applications recorded before tenancy).
async fn app_resource(pool: &apelora_persistence::Pool, application: &str) -> Res<Resource> {
    Ok(
        apelora_persistence::identity::application_context(pool, application)
            .await?
            .map_or_else(Resource::platform, |c| c.resource()),
    )
}

fn member(m: &str) -> Option<&str> {
    Some(m).filter(|m| !m.is_empty() && *m != "none")
}

// ---- pools ------------------------------------------------------------------------------------

/// One member's share of a pool.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StorageMemberUsage {
    pub member: String,
    pub used_bytes: u64,
    pub total_bytes: u64,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StoragePoolUsage {
    pub name: String,
    pub driver: String,
    pub status: String,
    pub description: String,
    /// One pool every member reaches (clustered LVM, Ceph): its space is counted once.
    pub shared: bool,
    /// What Apelora uses it for: `volumes` (`APELORA_VOLUME_POOL`), `backups`
    /// (`APELORA_BACKUP_POOL`), or nothing.
    pub role: Option<String>,
    /// Use per member as Incus reports it; a shared pool lists the first member that reported it.
    pub members: Vec<StorageMemberUsage>,
    /// Used of total over all members (a shared pool once); `None` when no member reported it.
    pub total: Option<HealthUsage>,
    /// Members whose use could not be read, with the reason.
    pub unreadable: Vec<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StoragePoolReport {
    pub source: Source,
    pub volume_pool: Option<String>,
    pub backup_pool: Option<String>,
    pub items: Vec<StoragePoolUsage>,
}

/// Storage pools with how full each is, per member and in total (read from each member's
/// state, like the Health page). Read-only. Needs `cluster.view`.
#[utoipa::path(get, path = "/api/v1/storage/pools", tag = "inventory", operation_id = "storage_pool_usage",
    responses((status = 200, body = StoragePoolReport), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 502, body = ErrorBody)))]
pub(super) async fn pool_usage(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Res<Json<StoragePoolReport>> {
    allow_platform(&p, "cluster.view")?;
    let pools = state.provider.storage_pools().await?;
    let (_, readings) = super::fabric_health::read_servers(&state).await?;
    let volume_pool = state.volume_pool.as_deref().map(str::to_owned);
    let backup_pool = state.backup_pool.as_deref().map(str::to_owned);
    let items = pools
        .into_iter()
        .map(|pool| {
            let shared = pool_is_shared(&pool.driver);
            let mut members = Vec::new();
            let mut unreadable = Vec::new();
            for r in &readings {
                match &r.resources {
                    Ok(res) => {
                        if let Some(u) = res.pools.iter().find(|u| u.name == pool.name)
                            && !(shared && !members.is_empty())
                        {
                            members.push(StorageMemberUsage {
                                member: r.member.name.clone(),
                                used_bytes: u.used_bytes,
                                total_bytes: u.total_bytes,
                            });
                        }
                    }
                    Err(e) => unreadable.push(format!("{}: {e}", r.member.name)),
                }
            }
            let total = (!members.is_empty()).then(|| HealthUsage {
                used_bytes: members.iter().map(|m| m.used_bytes).sum(),
                total_bytes: members.iter().map(|m| m.total_bytes).sum(),
            });
            let role = if volume_pool.as_deref() == Some(pool.name.as_str()) {
                Some("volumes".to_owned())
            } else if backup_pool.as_deref() == Some(pool.name.as_str()) {
                Some("backups".to_owned())
            } else {
                None
            };
            StoragePoolUsage {
                name: pool.name,
                driver: pool.driver,
                status: pool.status,
                description: pool.description,
                shared,
                role,
                members,
                total,
                unreadable,
            }
        })
        .collect();
    Ok(Json(StoragePoolReport {
        source: source(state.provider.mode()),
        volume_pool,
        backup_pool,
        items,
    }))
}

// ---- volumes ----------------------------------------------------------------------------------

/// A schedule as set, and how its last run went.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleState {
    pub every_minutes: i32,
    pub keep: i32,
    /// When it last ran successfully (RFC 3339).
    pub last_run_at: Option<String>,
    /// Why the last run failed; cleared by the next success.
    pub error: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct VolumeSchedules {
    /// `None`: no scheduled snapshots.
    pub snapshots: Option<ScheduleState>,
    /// `None`: no scheduled backups.
    pub backups: Option<ScheduleState>,
}

fn schedules_of(r: &VolumeRecord) -> VolumeSchedules {
    let state = |every: Option<i32>,
                 keep: Option<i32>,
                 last: Option<time::OffsetDateTime>,
                 error: &Option<String>| {
        every.map(|every_minutes| ScheduleState {
            every_minutes,
            keep: keep.unwrap_or(1),
            last_run_at: last.map(rfc3339),
            error: error.clone(),
        })
    };
    VolumeSchedules {
        snapshots: state(
            r.snapshot_every_minutes,
            r.snapshot_keep,
            r.last_snapshot_at,
            &r.snapshot_error,
        ),
        backups: state(
            r.backup_every_minutes,
            r.backup_keep,
            r.last_backup_at,
            &r.backup_error,
        ),
    }
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StorageVolume {
    pub application: String,
    pub module: String,
    pub instance: String,
    /// The design's name (`data`).
    pub name: String,
    /// The Incus volume (`<instance>-<name>`).
    pub incus_name: String,
    pub pool: String,
    pub member: String,
    pub size_mib: i32,
    pub class: String,
    /// What Incus reports now; `None` when Incus could not be read.
    pub state: Option<VolumeUse>,
    pub used_by: Vec<String>,
    pub schedules: VolumeSchedules,
    /// Backups recorded for it.
    pub backups: i64,
    pub last_backup_at: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StorageVolumeList {
    pub source: Source,
    /// Where backups go; `None`: backups cannot be taken (`APELORA_BACKUP_POOL` is unset).
    pub backup_pool: Option<String>,
    /// Why Incus could not be read (the list still shows Apelora's records).
    pub incus_error: Option<String>,
    pub items: Vec<StorageVolume>,
}

/// The custom volumes of `(project, pool)`, read once per pair, keyed by `(name, member)`.
type Observed = BTreeMap<(String, String), apelora_incus_adapter::CustomVolume>;

async fn observe(
    state: &AppState,
    pairs: BTreeSet<(String, String)>,
) -> Result<Observed, apelora_incus_adapter::AdapterError> {
    let mut out = BTreeMap::new();
    for (project, pool) in pairs {
        for v in state.provider.custom_volumes(&project, &pool).await? {
            out.insert(
                (
                    format!("{project}/{pool}/{}", v.name),
                    member(&v.location).unwrap_or_default().to_owned(),
                ),
                v,
            );
        }
    }
    Ok(out)
}

fn find<'a>(
    observed: &'a Observed,
    project: &str,
    pool: &str,
    name: &str,
    at: &str,
) -> Option<&'a apelora_incus_adapter::CustomVolume> {
    let key = format!("{project}/{pool}/{name}");
    observed
        .get(&(key.clone(), member(at).unwrap_or_default().to_owned()))
        // A standalone server, or a pool Incus lists without members.
        .or_else(|| observed.get(&(key, String::new())))
}

/// Every application volume the caller may see, with what Incus reports, its schedules and
/// how many backups it has. Read-only. Needs `application.view` on each application.
#[utoipa::path(get, path = "/api/v1/storage/volumes", tag = "applications", operation_id = "storage_volumes",
    responses((status = 200, body = StorageVolumeList), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody)))]
pub(super) async fn volumes(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Res<Json<StorageVolumeList>> {
    let pool = store(&state)?;
    let mut visible: BTreeMap<String, bool> = BTreeMap::new();
    let mut records = Vec::new();
    for r in apelora_persistence::volumes_with_applications(pool).await? {
        let ok = match visible.get(&r.application) {
            Some(ok) => *ok,
            None => {
                let res = app_resource(pool, &r.application).await?;
                let ok = allow(&p, "application.view", &res).is_ok();
                visible.insert(r.application.clone(), ok);
                ok
            }
        };
        if ok {
            records.push(r);
        }
    }
    let summaries: BTreeMap<(String, String), apelora_persistence::BackupSummary> =
        apelora_persistence::backup_summaries(pool)
            .await?
            .into_iter()
            .map(|s| ((s.project.clone(), s.source.clone()), s))
            .collect();
    let pairs = records
        .iter()
        .map(|r| (r.volume.project.clone(), r.volume.storage_pool.clone()))
        .collect();
    let (observed, incus_error) = match observe(&state, pairs).await {
        Ok(o) => (Some(o), None),
        Err(e) => {
            tracing::warn!(error = %e, "storage: volumes could not be read from Incus");
            (None, Some(e.to_string()))
        }
    };
    let items = records
        .into_iter()
        .map(|r| {
            let v = &r.volume;
            let live = observed.as_ref().map(|o| {
                find(o, &v.project, &v.storage_pool, &v.incus_name, &v.member)
                    .map(|c| instances_using(&c.used_by))
            });
            let summary = summaries.get(&(v.project.clone(), v.incus_name.clone()));
            StorageVolume {
                state: live.as_ref().map(|l| match l {
                    None => VolumeUse::Missing,
                    Some(u) if u.is_empty() => VolumeUse::Retained,
                    Some(_) => VolumeUse::Attached,
                }),
                used_by: live.flatten().unwrap_or_default(),
                schedules: schedules_of(v),
                backups: summary.map_or(0, |s| s.count),
                last_backup_at: summary.and_then(|s| s.newest).map(rfc3339),
                application: r.application.clone(),
                module: v.module.clone(),
                instance: v.instance.clone(),
                name: v.name.clone(),
                incus_name: v.incus_name.clone(),
                pool: v.storage_pool.clone(),
                member: v.member.clone(),
                size_mib: v.size_mib,
                class: v.class.clone(),
            }
        })
        .collect();
    Ok(Json(StorageVolumeList {
        source: source(state.provider.mode()),
        backup_pool: state.backup_pool.as_deref().map(str::to_owned),
        incus_error,
        items,
    }))
}

// ---- orphans ----------------------------------------------------------------------------------

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OrphanVolume {
    pub project: String,
    pub pool: String,
    pub name: String,
    /// Member of a member-local pool; empty otherwise.
    pub member: String,
    /// Size as configured, `None` when Incus reports none in a unit Apelora reads.
    pub size_mib: Option<u32>,
    pub content: String,
    pub used_by: Vec<String>,
    /// `user.apelora.application`, when Apelora created it.
    pub application: Option<String>,
    /// `user.apelora.backup-of`, for a backup copy.
    pub backup_of: Option<String>,
    pub created_at: String,
    /// Why it is listed, in words.
    pub reason: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OrphanList {
    pub source: Source,
    /// The projects looked at (Apelora's deploy projects).
    pub projects: Vec<String>,
    pub items: Vec<OrphanVolume>,
}

/// Why a custom volume nobody records is there, in words.
fn orphan_reason(v: &apelora_incus_adapter::CustomVolume) -> String {
    match (v.managed, v.application.as_deref(), v.backup_of.as_deref()) {
        (true, Some(app), Some(of)) => format!(
            "A backup copy of {of} made by Apelora for {app}; no backup record refers to it"
        ),
        (true, Some(app), None) => {
            format!("Created by Apelora for {app}; no application records it now")
        }
        _ => "Not created by Apelora (no ownership labels); Apelora never changes it".into(),
    }
}

/// Custom volumes in Apelora's projects that no application volume or backup record refers
/// to, in every pool, with their size. Read-only: deleting one is a decision made in Incus or,
/// for a kept application volume, on the application. Needs `cluster.view`.
#[utoipa::path(get, path = "/api/v1/storage/orphans", tag = "inventory", operation_id = "orphan_volumes",
    responses((status = 200, body = OrphanList), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 502, body = ErrorBody)))]
pub(super) async fn orphans(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Res<Json<OrphanList>> {
    allow_platform(&p, "cluster.view")?;
    let pool = store(&state)?;
    let referenced: BTreeSet<(String, String, String)> =
        apelora_persistence::referenced_volumes(pool)
            .await?
            .into_iter()
            .collect();
    let pools = state.provider.storage_pools().await?;
    // Apelora's projects: the configured deploy projects and every environment's.
    let mut projects: BTreeSet<String> = state.deploy_projects.iter().cloned().collect();
    projects.extend(apelora_persistence::identity::environment_incus_projects(pool).await?);
    let projects: Vec<String> = projects.into_iter().collect();
    let mut items = Vec::new();
    for project in &projects {
        for sp in &pools {
            let found = match state.provider.custom_volumes(project, &sp.name).await {
                Ok(found) => found,
                // An environment whose Incus project does not exist (yet): nothing to list.
                Err(apelora_incus_adapter::AdapterError::NotFound(_)) => continue,
                Err(e) => return Err(e.into()),
            };
            for v in found {
                if referenced.contains(&(project.clone(), sp.name.clone(), v.name.clone())) {
                    continue;
                }
                items.push(OrphanVolume {
                    reason: orphan_reason(&v),
                    project: project.clone(),
                    pool: sp.name.clone(),
                    member: member(&v.location).unwrap_or_default().to_owned(),
                    size_mib: v.size_mib,
                    content: v.content,
                    used_by: instances_using(&v.used_by),
                    application: v.application,
                    backup_of: v.backup_of,
                    created_at: v.created_at,
                    name: v.name,
                });
            }
        }
    }
    Ok(Json(OrphanList {
        source: source(state.provider.mode()),
        projects,
        items,
    }))
}

// ---- one volume: backups, schedules, restore --------------------------------------------------

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct VolumeBackupItem {
    /// The backup's Incus name.
    pub name: String,
    pub pool: String,
    pub member: String,
    pub size_mib: i32,
    /// When it was taken (RFC 3339): a restore brings back the data as it was then.
    pub taken_at: String,
    /// Made by the schedule (pruned to what the schedule keeps), not on request.
    pub scheduled: bool,
    /// The restore job that took it just before overwriting the volume.
    pub before_restore: Option<Uuid>,
    /// It exists in Incus; `None` when Incus could not be read.
    pub present: Option<bool>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct VolumeBackupList {
    pub source: Source,
    pub backup_pool: Option<String>,
    pub schedules: VolumeSchedules,
    /// Oldest first.
    pub items: Vec<VolumeBackupItem>,
}

async fn recorded(state: &AppState, name: &str, volume: &str) -> Res<VolumeRecord> {
    apelora_persistence::application_volume(store(state)?, name, volume)
        .await?
        .ok_or_else(|| not_found(format!("volume {volume} of application {name}")))
}

/// The volume's backups, oldest first, whether each still exists, and its schedules. Needs
/// `application.view`.
#[utoipa::path(get, path = "/api/v1/applications/{name}/volumes/{volume}/backups", tag = "applications", operation_id = "volume_backups",
    params(("name" = String, Path, description = "Application name"), ("volume" = String, Path, description = "The volume's Incus name")),
    responses((status = 200, body = VolumeBackupList), (status = 401, body = ErrorBody), (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody)))]
pub(super) async fn backups(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path((name, volume)): Path<(String, String)>,
) -> Res<Json<VolumeBackupList>> {
    load(&state, &p, &name).await?;
    let r = recorded(&state, &name, &volume).await?;
    let list = apelora_persistence::volume_backups(store(&state)?, &name, &volume).await?;
    let pairs = list
        .iter()
        .map(|b| (b.project.clone(), b.storage_pool.clone()))
        .collect();
    let observed = match observe(&state, pairs).await {
        Ok(o) => Some(o),
        Err(e) => {
            tracing::warn!(error = %e, volume = %volume, "storage: backups could not be read from Incus");
            None
        }
    };
    let items = list
        .into_iter()
        .map(|b| VolumeBackupItem {
            present: observed
                .as_ref()
                .map(|o| find(o, &b.project, &b.storage_pool, &b.name, &b.member).is_some()),
            name: b.name,
            pool: b.storage_pool,
            member: b.member,
            size_mib: b.size_mib,
            taken_at: rfc3339(b.created_at),
            scheduled: b.scheduled,
            before_restore: b.before_restore,
        })
        .collect();
    Ok(Json(VolumeBackupList {
        source: source(state.provider.mode()),
        backup_pool: state.backup_pool.as_deref().map(str::to_owned),
        schedules: schedules_of(&r),
        items,
    }))
}

/// Queues a job for an application together with its audit row (one transaction).
#[allow(clippy::too_many_arguments)]
async fn queue(
    state: &AppState,
    p: &Principal,
    headers: &HeaderMap,
    application_id: Uuid,
    res: &Resource,
    kind: &str,
    action: &str,
    target: &str,
    payload: Value,
    detail: Value,
) -> Res<(StatusCode, Json<JobAccepted>)> {
    let pool = store(state)?;
    let key = idempotency_key(headers, p)?;
    let request = super::requests::current();
    let actor = p.actor_ref();
    let mut tx = super::authz::begin(pool).await?;
    let enqueued = apelora_persistence::enqueue_job_from(
        &mut tx,
        kind,
        Some(application_id),
        &payload,
        key.as_deref(),
        &JobOrigin {
            request_id: request.as_deref(),
            queued_by: Some(&actor),
            retry_of: None,
        },
    )
    .await?;
    let (job_id, created) = match enqueued {
        Enqueued::Created(id) => (id, true),
        Enqueued::Existing(id) => (id, false),
    };
    if created {
        let mut detail = detail;
        detail["jobId"] = json!(job_id);
        audit_scoped(&mut tx, p, action, target, "success", Some(res), &detail).await?;
    }
    super::authz::commit(tx).await?;
    Ok((StatusCode::ACCEPTED, Json(JobAccepted { job_id, created })))
}

fn no_backup_pool() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "backup_pool_missing",
        "no backup pool is configured: an administrator sets APELORA_BACKUP_POOL for the control plane and the worker",
    )
}

/// Backs the volume up now: a copy, through a short-lived snapshot, into the backup pool on
/// another member. Kept until someone deletes it (scheduled backups are pruned, these are not).
/// Queues a `volume.backup` job. Needs `application.operate`; audited.
#[utoipa::path(post, path = "/api/v1/applications/{name}/volumes/{volume}/backups", tag = "applications", operation_id = "back_up_volume",
    params(("name" = String, Path, description = "Application name"), ("volume" = String, Path, description = "The volume's Incus name"),
        ("Idempotency-Key" = Option<String>, Header, description = "Makes the request safe to repeat")),
    responses((status = 202, body = JobAccepted), (status = 401, body = ErrorBody), (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
pub(super) async fn back_up(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    headers: HeaderMap,
    Path((name, volume)): Path<(String, String)>,
) -> Res<(StatusCode, Json<JobAccepted>)> {
    let (row, res) = load(&state, &p, &name).await?;
    allow(&p, "application.operate", &res)?;
    let r = recorded(&state, &name, &volume).await?;
    let backup_pool = state.backup_pool.as_deref().ok_or_else(no_backup_pool)?;
    let payload = json!({ "application": name, "project": r.project, "module": r.module,
        "volume": r.incus_name, "pool": r.storage_pool, "member": r.member,
        "sizeMib": r.size_mib, "backupPool": backup_pool });
    let target = format!("{}/{}", r.project, r.incus_name);
    let detail = json!({ "application": name, "module": r.module, "pool": backup_pool });
    queue(
        &state,
        &p,
        &headers,
        row.id,
        &res,
        "volume.backup",
        "application.volume.backup.requested",
        &target,
        payload,
        detail,
    )
    .await
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SetVolumeSchedules {
    /// Scheduled snapshots (`auto-…`, in the volume's pool); `null` turns them off.
    pub snapshots: Option<Schedule>,
    /// Scheduled backups (into the backup pool, on another member); `null` turns them off.
    pub backups: Option<Schedule>,
}

/// Sets or turns off the volume's snapshot and backup schedules. A new or changed schedule
/// first runs at the worker's next pass (within a minute). Needs `application.deploy`; audited.
#[utoipa::path(put, path = "/api/v1/applications/{name}/volumes/{volume}/schedules", tag = "applications", operation_id = "set_volume_schedules",
    params(("name" = String, Path, description = "Application name"), ("volume" = String, Path, description = "The volume's Incus name")),
    request_body = SetVolumeSchedules,
    responses((status = 204), (status = 400, body = ErrorBody), (status = 401, body = ErrorBody), (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
pub(super) async fn set_schedules(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path((name, volume)): Path<(String, String)>,
    body: Result<Json<SetVolumeSchedules>, JsonRejection>,
) -> Res<StatusCode> {
    let (_, res) = load(&state, &p, &name).await?;
    allow(&p, "application.deploy", &res)?;
    let Json(req) = body.map_err(|e| bad_request(e.body_text()))?;
    if let Some(s) = &req.snapshots {
        s.validate("snapshots").map_err(bad_request)?;
    }
    if let Some(s) = &req.backups {
        s.validate("backups").map_err(bad_request)?;
        if state.backup_pool.is_none() {
            return Err(no_backup_pool());
        }
    }
    let r = recorded(&state, &name, &volume).await?;
    let pair = |s: &Option<Schedule>| s.map(|s| (s.every_minutes, s.keep));
    let mut tx = super::authz::begin(store(&state)?).await?;
    apelora_persistence::set_volume_schedules(
        &mut *tx,
        r.id,
        pair(&req.snapshots),
        pair(&req.backups),
    )
    .await?;
    audit_scoped(
        &mut tx,
        &p,
        "application.volume.schedules",
        &format!("{}/{}", r.project, r.incus_name),
        "success",
        Some(&res),
        &json!({ "application": name, "snapshots": req.snapshots, "backups": req.backups,
                 "before": { "snapshots": pair_of(r.snapshot_every_minutes, r.snapshot_keep),
                             "backups": pair_of(r.backup_every_minutes, r.backup_keep) } }),
    )
    .await?;
    super::authz::commit(tx).await?;
    Ok(StatusCode::NO_CONTENT)
}

fn pair_of(every: Option<i32>, keep: Option<i32>) -> Value {
    every.map_or(Value::Null, |e| json!({ "everyMinutes": e, "keep": keep }))
}

#[derive(Deserialize, IntoParams)]
pub(super) struct BackupPath {
    /// Application name.
    name: String,
    /// The volume's Incus name.
    volume: String,
    /// The backup's Incus name.
    backup: String,
}

#[derive(Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub(super) struct RestoreQuery {
    /// Take a safety backup of the current data first (default true).
    #[serde(default = "yes")]
    safety_backup: bool,
}

fn yes() -> bool {
    true
}

#[derive(Serialize, ToSchema)]
pub struct RestorePreview {
    pub source: Source,
    pub plan: RestorePlan,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RestoreVolumeRequest {
    /// The volume's Incus name again, typed by the person restoring it.
    pub confirm: String,
    /// Take a safety backup of the current data first (default true).
    #[serde(default = "yes")]
    pub safety_backup: bool,
}

/// The dry run, from Apelora's records and what Incus reports now.
async fn restore_plan(
    state: &AppState,
    name: &str,
    volume: &str,
    backup: &str,
    safety_backup: bool,
) -> Res<(RestorePlan, VolumeRecord, BackupRecord)> {
    let r = recorded(state, name, volume).await?;
    let b = apelora_persistence::volume_backup(store(state)?, name, volume, backup)
        .await?
        .ok_or_else(|| not_found(format!("backup {backup} of volume {volume}")))?;
    let live = state
        .provider
        .volume(&r.project, &r.storage_pool, member_of(&r), &r.incus_name)
        .await?;
    let backup_present = state
        .provider
        .volume(&b.project, &b.storage_pool, member(&b.member), &b.name)
        .await?
        .is_some();
    let snapshots: Vec<String> = match &live {
        Some(_) => state
            .provider
            .volume_snapshots(&r.project, &r.storage_pool, member_of(&r), &r.incus_name)
            .await?
            .into_iter()
            .map(|s| s.name)
            .collect(),
        None => Vec::new(),
    };
    let mut users = Vec::new();
    for i in live
        .as_ref()
        .map(|v| instances_using(&v.used_by))
        .unwrap_or_default()
    {
        let running = state
            .provider
            .instance(&r.project, &i)
            .await?
            .is_some_and(|s| s.status == "Running");
        users.push((i, running));
    }
    let taken_at = rfc3339(b.created_at);
    let plan = plan_restore(&RestoreSubject {
        volume: &r.incus_name,
        pool: &r.storage_pool,
        member: &r.member,
        size_mib: u32::try_from(r.size_mib).unwrap_or(0),
        volume_present: live.is_some(),
        backup: &b.name,
        backup_pool: &b.storage_pool,
        backup_member: &b.member,
        backup_taken_at: &taken_at,
        backup_present,
        snapshots: &snapshots,
        users: &users,
        safety_pool: state.backup_pool.as_deref(),
        safety_backup,
    });
    Ok((plan, r, b))
}

/// Dry run of restoring the volume from one of its backups: what is replaced (data written
/// since the backup, the volume's snapshots), which instances must be stopped, where the safety
/// backup goes, and the steps. Changes nothing. Needs `application.operate`.
#[utoipa::path(get, path = "/api/v1/applications/{name}/volumes/{volume}/backups/{backup}/restore", tag = "applications", operation_id = "preview_volume_restore",
    params(BackupPath, RestoreQuery),
    responses((status = 200, body = RestorePreview), (status = 401, body = ErrorBody), (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody), (status = 502, body = ErrorBody)))]
pub(super) async fn preview_restore(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(BackupPath {
        name,
        volume,
        backup,
    }): Path<BackupPath>,
    Query(q): Query<RestoreQuery>,
) -> Res<Json<RestorePreview>> {
    let (_, res) = load(&state, &p, &name).await?;
    allow(&p, "application.operate", &res)?;
    let (plan, _, _) = restore_plan(&state, &name, &volume, &backup, q.safety_backup).await?;
    Ok(Json(RestorePreview {
        source: source(state.provider.mode()),
        plan,
    }))
}

/// Restores the volume from one of its backups, after the dry run passes again and the
/// volume's name is typed: a safety backup of the current data first (unless declined), then
/// the backup is copied over the volume. Every instance using it must be stopped; they stay
/// stopped. Queues a `volume.restore` job. Needs `application.operate` (and an approval where
/// a policy asks); audited with the job.
#[utoipa::path(post, path = "/api/v1/applications/{name}/volumes/{volume}/backups/{backup}/restore", tag = "applications", operation_id = "restore_volume_backup",
    params(BackupPath, ("Idempotency-Key" = Option<String>, Header, description = "Makes the request safe to repeat")),
    request_body = RestoreVolumeRequest,
    responses((status = 202, body = JobAccepted), (status = 400, body = ErrorBody), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody), (status = 502, body = ErrorBody)))]
pub(super) async fn restore(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    headers: HeaderMap,
    Path(BackupPath {
        name,
        volume,
        backup,
    }): Path<BackupPath>,
    body: Result<Json<RestoreVolumeRequest>, JsonRejection>,
) -> Res<(StatusCode, Json<JobAccepted>)> {
    let (row, res) = load(&state, &p, &name).await?;
    allow(&p, "application.operate", &res)?;
    let Json(req) = body.map_err(|e| bad_request(e.body_text()))?;
    if req.confirm != volume {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "confirmation_mismatch",
            format!("type the volume's name ({volume}) to confirm"),
        ));
    }
    let (plan, r, b) = restore_plan(&state, &name, &volume, &backup, req.safety_backup).await?;
    if !plan.blocked.is_empty() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "restore_blocked",
            format!("nothing was restored: {}", plan.blocked.join("; ")),
        ));
    }
    let target = format!("{}/{}", r.project, r.incus_name);
    let approval = guard(&state, &p, &headers, "application.operate", &target).await?;
    let payload = json!({ "application": name, "project": r.project, "module": r.module,
        "volume": r.incus_name, "pool": r.storage_pool, "member": r.member, "sizeMib": r.size_mib,
        "backup": b.name, "backupPool": b.storage_pool, "backupMember": b.member,
        "safetyPool": plan.safety_backup_pool });
    let detail = json!({ "application": name, "module": r.module, "backup": b.name,
        "backupTakenAt": plan.backup_taken_at, "safetyBackupPool": plan.safety_backup_pool,
        "snapshotsDeleted": plan.snapshots_deleted, "approval": approval });
    queue(
        &state,
        &p,
        &headers,
        row.id,
        &res,
        "volume.restore",
        "application.volume.restore.requested",
        &target,
        payload,
        detail,
    )
    .await
}

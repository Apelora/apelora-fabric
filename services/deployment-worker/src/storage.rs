// SPDX-License-Identifier: AGPL-3.0-only
//! Scheduled snapshots and backups of volumes (docs/architecture/STORAGE.md, step 5), and the
//! `volume.backup` job a person starts from the API.
//!
//! A pass claims the volumes that are due (two workers never claim the same one), takes a
//! scheduled snapshot named `auto-<time>` and prunes the oldest scheduled ones beyond what the
//! schedule keeps; snapshots someone took by hand are never pruned. A backup is a copy of the
//! volume as it was at a short-lived snapshot, in the backup pool and on another member where
//! there is one, so it survives losing the volume's member; scheduled backups beyond what the
//! schedule keeps are deleted, backups someone asked for are kept until someone deletes them.
//!
//! The `volume.restore` job brings a backup back into its volume: it refuses while an instance
//! using the volume runs, copies the volume as it is now into the backup pool first (a safety
//! backup, unless the person declined it), then copies the backup over the volume with an Incus
//! `refresh` copy, which also deletes the volume's own snapshots. Each step re-reads Incus, and
//! the safety backup is recorded with the job, so a retried attempt continues instead of
//! starting over.

use std::time::Duration;

use apelora_domain::storage::{AUTO_SNAPSHOT_PREFIX, backup_name, snapshots_to_prune, stamped};
use apelora_incus_adapter::{AdapterError, Provider};
use apelora_persistence::{
    ClaimedJob, DueVolume, NewBackup, Outcome, Pool, ScheduleKind, StoreError, audit,
    backup_before_restore, claim_due, forget_backup, record_backup, record_scheduled,
    volume_backups,
};
use serde::Deserialize;
use serde_json::json;

/// Volumes one pass handles per kind; the rest wait for the next pass.
const PER_PASS: i64 = 20;

fn member(m: &str) -> Option<&str> {
    Some(m).filter(|m| !m.is_empty() && *m != "none")
}

/// What a `volume.backup` job carries; the control plane resolves the backup pool.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupRequest {
    pub application: String,
    pub project: String,
    pub module: String,
    pub volume: String,
    pub pool: String,
    #[serde(default)]
    pub member: String,
    pub size_mib: u32,
    pub backup_pool: String,
}

/// Where a backup goes: the online member that is not the volume's, first by name; the
/// volume's own member when it is the only one (a standalone server: no member at all).
async fn backup_member(
    provider: &Provider,
    source: Option<&str>,
) -> Result<Option<String>, AdapterError> {
    let mut online: Vec<String> = provider
        .cluster_members()
        .await?
        .into_iter()
        .filter(|m| m.status == "Online")
        .map(|m| m.name)
        .collect();
    online.sort();
    Ok(online
        .iter()
        .find(|m| Some(m.as_str()) != source)
        .or_else(|| online.first())
        .cloned())
}

/// Copies the volume, as it is now, into the backup pool: through a snapshot that is deleted
/// again, so the copy is one point in time. Returns the backup's name, its member and the
/// snapshot it was copied from (which names the moment).
pub async fn back_up(
    provider: &Provider,
    req: &BackupRequest,
    at: time::OffsetDateTime,
) -> Result<(String, String, String), AdapterError> {
    let snapshot = stamped("bk-", at);
    let name = backup_name(&req.volume, at);
    let from = member(&req.member);
    let to = backup_member(provider, from).await?;
    provider
        .snapshot_volume(
            &req.project,
            &req.pool,
            from,
            &req.volume,
            &req.application,
            &snapshot,
        )
        .await?;
    let copied = provider
        .copy_volume(
            &req.project,
            &req.pool,
            from,
            &req.volume,
            &req.application,
            Some(&snapshot),
            &req.backup_pool,
            to.as_deref(),
            &name,
            Some(&req.volume),
        )
        .await;
    // The snapshot only served the copy: remove it whether the copy worked or not.
    let cleaned = provider
        .delete_snapshot(
            &req.project,
            &req.pool,
            from,
            &req.volume,
            &req.application,
            &snapshot,
        )
        .await;
    copied?;
    if let Err(e) = cleaned {
        tracing::warn!(volume = %req.volume, %snapshot, error = %e, "backup: the temporary snapshot was not deleted");
    }
    Ok((name, to.unwrap_or_default(), snapshot))
}

/// Runs a `volume.backup` job.
pub async fn run_backup_job(
    pool: &Pool,
    provider: &Provider,
    payload: &serde_json::Value,
) -> Result<Outcome, StoreError> {
    let req: BackupRequest = match serde_json::from_value(payload.clone()) {
        Ok(r) => r,
        Err(e) => return Ok(Outcome::Rejected(format!("invalid payload: {e}"))),
    };
    match back_up(provider, &req, time::OffsetDateTime::now_utc()).await {
        Ok((name, to, snapshot)) => {
            record(pool, &req, &name, &to, &snapshot, false, None).await?;
            Ok(Outcome::Succeeded)
        }
        Err(e) => Ok(Outcome::Failed(e.to_string())),
    }
}

async fn record(
    pool: &Pool,
    req: &BackupRequest,
    name: &str,
    member: &str,
    snapshot: &str,
    scheduled: bool,
    before_restore: Option<uuid::Uuid>,
) -> Result<(), StoreError> {
    // The backup's record and its audit row are written together.
    let mut tx = apelora_persistence::begin(pool).await?;
    record_backup(
        &mut *tx,
        &NewBackup {
            application: &req.application,
            project: &req.project,
            source: &req.volume,
            snapshot,
            name,
            storage_pool: &req.backup_pool,
            member,
            size_mib: req.size_mib,
            scheduled,
            before_restore,
        },
    )
    .await?;
    audit(
        &mut *tx,
        "storage-scheduler",
        if scheduled { "application.volume.backup.scheduled" } else { "application.volume.backup" },
        &format!("{}/{}", req.project, req.volume),
        &json!({ "application": req.application, "module": req.module, "backup": name, "pool": req.backup_pool, "member": member, "beforeRestore": before_restore }),
    )
    .await?;
    apelora_persistence::commit(tx).await
}

async fn scheduled_snapshot(
    pool: &Pool,
    provider: &Provider,
    v: &DueVolume,
) -> Result<(), StoreError> {
    let name = stamped(AUTO_SNAPSHOT_PREFIX, time::OffsetDateTime::now_utc());
    let at = member(&v.member);
    let result = async {
        provider
            .snapshot_volume(
                &v.project,
                &v.storage_pool,
                at,
                &v.incus_name,
                &v.application,
                &name,
            )
            .await?;
        let all = provider
            .volume_snapshots(&v.project, &v.storage_pool, at, &v.incus_name)
            .await?;
        let names: Vec<&str> = all.iter().map(|s| s.name.as_str()).collect();
        let prune = snapshots_to_prune(&names, u32::try_from(v.keep).unwrap_or(1));
        for old in &prune {
            provider
                .delete_snapshot(
                    &v.project,
                    &v.storage_pool,
                    at,
                    &v.incus_name,
                    &v.application,
                    old,
                )
                .await?;
        }
        Ok::<_, AdapterError>(prune.len())
    }
    .await;
    match result {
        Ok(pruned) => {
            apelora_metrics::VOLUME_SCHEDULE_RUNS.inc(&["snapshot", "ok"]);
            record_scheduled(pool, v.id, ScheduleKind::Snapshot, None).await?;
            tracing::info!(volume = %v.incus_name, snapshot = %name, pruned, "scheduled snapshot taken");
        }
        Err(e) => {
            apelora_metrics::VOLUME_SCHEDULE_RUNS.inc(&["snapshot", "failed"]);
            record_scheduled(pool, v.id, ScheduleKind::Snapshot, Some(&e.to_string())).await?;
            tracing::warn!(volume = %v.incus_name, error = %e, "scheduled snapshot failed");
        }
    }
    Ok(())
}

async fn scheduled_backup(
    pool: &Pool,
    provider: &Provider,
    v: &DueVolume,
    backup_pool: Option<&str>,
) -> Result<(), StoreError> {
    let Some(backup_pool) = backup_pool else {
        apelora_metrics::VOLUME_SCHEDULE_RUNS.inc(&["backup", "not_configured"]);
        return record_scheduled(
            pool,
            v.id,
            ScheduleKind::Backup,
            Some("no backup pool is configured (APELORA_BACKUP_POOL)"),
        )
        .await;
    };
    let req = BackupRequest {
        application: v.application.clone(),
        project: v.project.clone(),
        module: v.module.clone(),
        volume: v.incus_name.clone(),
        pool: v.storage_pool.clone(),
        member: v.member.clone(),
        size_mib: u32::try_from(v.size_mib).unwrap_or(0),
        backup_pool: backup_pool.to_owned(),
    };
    match back_up(provider, &req, time::OffsetDateTime::now_utc()).await {
        Ok((name, to, snapshot)) => {
            apelora_metrics::VOLUME_SCHEDULE_RUNS.inc(&["backup", "ok"]);
            record(pool, &req, &name, &to, &snapshot, true, None).await?;
            // Keep the newest scheduled backups; the ones someone asked for stay.
            let scheduled: Vec<_> = volume_backups(pool, &v.application, &v.incus_name)
                .await?
                .into_iter()
                .filter(|b| b.scheduled)
                .collect();
            let excess = scheduled
                .len()
                .saturating_sub(usize::try_from(v.keep).unwrap_or(1));
            for old in &scheduled[..excess] {
                match provider
                    .delete_volume(
                        &old.project,
                        &old.storage_pool,
                        member(&old.member),
                        &old.name,
                        &v.application,
                    )
                    .await
                {
                    Ok(_) => {
                        forget_backup(pool, old.id).await?;
                    }
                    Err(e) => {
                        tracing::warn!(backup = %old.name, error = %e, "an old scheduled backup was not deleted")
                    }
                }
            }
            record_scheduled(pool, v.id, ScheduleKind::Backup, None).await
        }
        Err(e) => {
            apelora_metrics::VOLUME_SCHEDULE_RUNS.inc(&["backup", "failed"]);
            tracing::warn!(volume = %v.incus_name, error = %e, "scheduled backup failed");
            record_scheduled(pool, v.id, ScheduleKind::Backup, Some(&e.to_string())).await
        }
    }
}

/// What a `volume.restore` job carries; the control plane resolves every name from its records.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreRequest {
    pub application: String,
    pub project: String,
    pub module: String,
    /// The volume to overwrite (Incus name), its pool and member.
    pub volume: String,
    pub pool: String,
    #[serde(default)]
    pub member: String,
    pub size_mib: u32,
    /// The backup to bring back (Incus name), its pool and member.
    pub backup: String,
    pub backup_pool: String,
    #[serde(default)]
    pub backup_member: String,
    /// Where the safety backup of the current data goes; `None`: none is taken.
    #[serde(default)]
    pub safety_pool: Option<String>,
}

/// Why a restore step stopped: Incus refused or failed, or the job store did.
enum RestoreError {
    Incus(AdapterError),
    Store(StoreError),
}

impl From<AdapterError> for RestoreError {
    fn from(e: AdapterError) -> Self {
        RestoreError::Incus(e)
    }
}

impl From<StoreError> for RestoreError {
    fn from(e: StoreError) -> Self {
        RestoreError::Store(e)
    }
}

/// Instances using the volume that run now (from Incus's `used_by`).
async fn running_users(
    provider: &Provider,
    project: &str,
    used_by: &[String],
) -> Result<Vec<String>, AdapterError> {
    let mut running = Vec::new();
    for path in used_by {
        let Some(name) = path
            .strip_prefix("/1.0/instances/")
            .and_then(|r| r.split(['?', '/']).next())
        else {
            continue;
        };
        if provider
            .instance(project, name)
            .await?
            .is_some_and(|s| s.status == "Running")
        {
            running.push(name.to_owned());
        }
    }
    Ok(running)
}

/// Refuses (nothing changed) unless the volume and the backup exist and no instance using the
/// volume runs. Checked before each step that changes something.
async fn restore_preconditions(
    provider: &Provider,
    r: &RestoreRequest,
) -> Result<Option<String>, AdapterError> {
    let Some(v) = provider
        .volume(&r.project, &r.pool, member(&r.member), &r.volume)
        .await?
    else {
        return Ok(Some(format!("volume {} is not in Incus", r.volume)));
    };
    if provider
        .volume(
            &r.project,
            &r.backup_pool,
            member(&r.backup_member),
            &r.backup,
        )
        .await?
        .is_none()
    {
        return Ok(Some(format!("backup {} is not in Incus", r.backup)));
    }
    let running = running_users(provider, &r.project, &v.used_by).await?;
    if !running.is_empty() {
        return Ok(Some(format!(
            "stop {} first: a volume is never overwritten under a running instance",
            running.join(", ")
        )));
    }
    Ok(None)
}

/// Runs a `volume.restore` job (see the module documentation for its steps and safe points).
pub async fn run_restore_job(
    pool: &Pool,
    provider: &Provider,
    job: &ClaimedJob,
) -> Result<Outcome, StoreError> {
    let r: RestoreRequest = match serde_json::from_value(job.payload.clone()) {
        Ok(r) => r,
        Err(e) => return Ok(Outcome::Rejected(format!("invalid payload: {e}"))),
    };
    match restore(pool, provider, job, &r).await {
        Ok(outcome) => Ok(outcome),
        Err(RestoreError::Incus(e)) => Ok(Outcome::Failed(e.to_string())),
        Err(RestoreError::Store(e)) => Err(e),
    }
}

async fn restore(
    pool: &Pool,
    provider: &Provider,
    job: &ClaimedJob,
    r: &RestoreRequest,
) -> Result<Outcome, RestoreError> {
    if let Some(why) = restore_preconditions(provider, r).await? {
        return Ok(Outcome::Rejected(format!("nothing was changed: {why}")));
    }
    // 1. The safety backup: the volume as it is now, kept with the other backups. A retried
    //    attempt finds the one an earlier attempt took (it is recorded with this job).
    let mut safety = None;
    if let Some(safety_pool) = &r.safety_pool {
        safety = match backup_before_restore(pool, job.id).await? {
            Some(b) => Some(b.name),
            None => {
                let req = BackupRequest {
                    application: r.application.clone(),
                    project: r.project.clone(),
                    module: r.module.clone(),
                    volume: r.volume.clone(),
                    pool: r.pool.clone(),
                    member: r.member.clone(),
                    size_mib: r.size_mib,
                    backup_pool: safety_pool.clone(),
                };
                let (name, to, snapshot) =
                    back_up(provider, &req, time::OffsetDateTime::now_utc()).await?;
                record(pool, &req, &name, &to, &snapshot, false, Some(job.id)).await?;
                tracing::info!(volume = %r.volume, backup = %name, "restore: safety backup taken");
                Some(name)
            }
        };
    }
    // 2. Safe point: nothing has been overwritten yet.
    if let Some(by) = apelora_persistence::jobs::cancel_requested(pool, job.id).await? {
        return Ok(Outcome::Cancelled(format!(
            "cancelled by {by} before volume {} was changed{}",
            r.volume,
            safety
                .as_deref()
                .map(|b| format!("; the safety backup {b} is kept"))
                .unwrap_or_default()
        )));
    }
    // An instance may have been started while the safety backup was taken.
    if let Some(why) = restore_preconditions(provider, r).await? {
        return Ok(Outcome::Rejected(format!(
            "nothing was overwritten: {why}{}",
            safety
                .as_deref()
                .map(|b| format!(" (the safety backup {b} is kept)"))
                .unwrap_or_default()
        )));
    }
    // 3. Copy the backup over the volume. One Incus operation; a failure part-way leaves the
    //    volume partly copied, and the next attempt copies the whole backup again.
    let snapshots: Vec<String> = provider
        .volume_snapshots(&r.project, &r.pool, member(&r.member), &r.volume)
        .await?
        .into_iter()
        .map(|s| s.name)
        .collect();
    tracing::info!(volume = %r.volume, backup = %r.backup, snapshots = snapshots.len(), "restore: copying the backup over the volume");
    provider
        .refresh_volume(
            &r.project,
            &r.backup_pool,
            member(&r.backup_member),
            &r.backup,
            &r.pool,
            member(&r.member),
            &r.volume,
            &r.application,
        )
        .await?;
    audit(
        pool,
        "worker",
        "application.volume.restored",
        &format!("{}/{}", r.project, r.volume),
        &json!({ "application": r.application, "module": r.module, "backup": r.backup,
                 "safetyBackup": safety, "snapshotsDeleted": snapshots, "jobId": job.id }),
    )
    .await?;
    Ok(Outcome::Succeeded)
}

/// One pass: every due snapshot, then every due backup.
pub async fn scheduled_pass(
    pool: &Pool,
    provider: &Provider,
    backup_pool: Option<&str>,
) -> Result<(), StoreError> {
    for v in claim_due(pool, ScheduleKind::Snapshot, PER_PASS).await? {
        scheduled_snapshot(pool, provider, &v).await?;
    }
    for v in claim_due(pool, ScheduleKind::Backup, PER_PASS).await? {
        scheduled_backup(pool, provider, &v, backup_pool).await?;
    }
    Ok(())
}

/// Runs scheduled passes every `every` until the task is dropped.
pub async fn schedule(
    pool: Pool,
    provider: Provider,
    every: Duration,
    backup_pool: Option<String>,
) {
    loop {
        if let Err(e) = scheduled_pass(&pool, &provider, backup_pool.as_deref()).await {
            tracing::error!(error = %e, "storage schedule pass failed");
        }
        tokio::time::sleep(every).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apelora_incus_adapter::MockIncus;
    use apelora_persistence::{
        application_volumes, enqueue_job, ensure_application, set_volume_schedules,
    };

    /// An application with one module whose volume exists on the mock, as a deployment leaves it.
    async fn deployed(pool: &Pool) -> (MockIncus, Provider, uuid::Uuid) {
        let m = MockIncus::default();
        let provider = Provider::Mock(m.clone());
        ensure_application(pool, "shop").await.expect("application");
        enqueue_job(pool, "instance.create", None, &json!({ "project": "apelora", "name": "shop-db-01", "kind": "container",
            "source": { "protocol": "oci", "server": "https://registry.example", "alias": "postgres:18" }, "start": true,
            "owner": { "application": "shop", "module": "db-01", "replica": 1 },
            "volumes": [{ "name": "data", "sizeMib": 1024, "path": "/data", "pool": "local" }] }), None)
            .await
            .expect("enqueue");
        while crate::run_once(pool, &provider, "test").await.expect("run") {}
        let id = application_volumes(pool, "shop").await.expect("volumes")[0].id;
        (m, provider, id)
    }

    async fn due_now(pool: &Pool, column: &str) {
        sqlx::query(&format!(
            "UPDATE volumes SET {column} = now() - interval '1 second'"
        ))
        .execute(pool)
        .await
        .expect("due");
        tokio::time::sleep(Duration::from_millis(1100)).await; // names carry the second
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn scheduled_snapshots_keep_the_newest_and_never_touch_manual_ones(pool: Pool) {
        let (m, provider, id) = deployed(&pool).await;
        set_volume_schedules(&pool, id, Some((60, 2)), None)
            .await
            .expect("schedule");
        scheduled_pass(&pool, &provider, None)
            .await
            .expect("pass 1");
        provider
            .snapshot_volume(
                "apelora",
                "local",
                Some("mock-member-01"),
                "shop-db-01-data",
                "shop",
                "before-upgrade",
            )
            .await
            .expect("manual snapshot");
        for _ in 0..2 {
            due_now(&pool, "next_snapshot_at").await;
            scheduled_pass(&pool, &provider, None).await.expect("pass");
        }
        let names: Vec<String> = provider
            .volume_snapshots(
                "apelora",
                "local",
                Some("mock-member-01"),
                "shop-db-01-data",
            )
            .await
            .expect("snapshots")
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert_eq!(
            names.iter().filter(|n| n.starts_with("auto-")).count(),
            2,
            "{names:?}"
        );
        assert!(names.contains(&"before-upgrade".to_owned()), "{names:?}");
        // Not due again until the interval has passed.
        scheduled_pass(&pool, &provider, None).await.expect("pass");
        assert_eq!(
            m.storage().snapshots.values().map(Vec::len).sum::<usize>(),
            3
        );
        let v = &application_volumes(&pool, "shop").await.expect("volumes")[0];
        assert!(v.last_snapshot_at.is_some() && v.snapshot_error.is_none());
    }

    /// A backup of the deployed volume, made through the `volume.backup` job.
    async fn backed_up(pool: &Pool, provider: &Provider) -> apelora_persistence::BackupRecord {
        enqueue_job(pool, "volume.backup", None, &json!({ "application": "shop", "project": "apelora", "module": "db-01",
            "volume": "shop-db-01-data", "pool": "local", "member": "mock-member-01", "sizeMib": 1024, "backupPool": "backup" }), None)
            .await
            .expect("enqueue");
        while crate::run_once(pool, provider, "test").await.expect("run") {}
        volume_backups(pool, "shop", "shop-db-01-data")
            .await
            .expect("backups")
            .pop()
            .expect("a backup")
    }

    fn restore_payload(b: &apelora_persistence::BackupRecord, safety: bool) -> serde_json::Value {
        json!({ "application": "shop", "project": "apelora", "module": "db-01", "volume": "shop-db-01-data",
            "pool": "local", "member": "mock-member-01", "sizeMib": 1024, "backup": b.name,
            "backupPool": b.storage_pool, "backupMember": b.member, "safetyPool": safety.then_some("backup") })
    }

    async fn run_restore(
        pool: &Pool,
        provider: &Provider,
        payload: serde_json::Value,
    ) -> (uuid::Uuid, String, Option<String>) {
        let job = enqueue_job(pool, "volume.restore", None, &payload, None)
            .await
            .expect("enqueue")
            .id();
        while crate::run_once(pool, provider, "test").await.expect("run") {}
        let j = apelora_persistence::get_job(pool, job).await.expect("job");
        (job, j.status, j.last_error)
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn a_restore_takes_a_safety_backup_then_copies_the_backup_over_the_stopped_volume(
        pool: Pool,
    ) {
        let (m, provider, _) = deployed(&pool).await;
        let b = backed_up(&pool, &provider).await;
        provider
            .snapshot_volume(
                "apelora",
                "local",
                Some("mock-member-01"),
                "shop-db-01-data",
                "shop",
                "before",
            )
            .await
            .expect("snapshot");

        // Refused while the instance using the volume runs: nothing is copied.
        m.storage()
            .volumes
            .get_mut("apelora/local/mock-member-01/shop-db-01-data")
            .expect("volume")
            .used_by = vec!["/1.0/instances/shop-db-01?project=apelora".into()];
        let copies = m.storage().copies.len();
        let (_, status, error) = run_restore(&pool, &provider, restore_payload(&b, true)).await;
        assert_eq!(status, "failed");
        assert!(error.unwrap_or_default().contains("stop shop-db-01 first"));
        assert_eq!(m.storage().copies.len(), copies, "nothing was copied");

        // Stopped: a safety backup, then the refresh copy, which drops the volume's snapshots.
        provider
            .change_state(
                "apelora",
                "shop-db-01",
                apelora_domain::InstanceAction::Stop,
                false,
            )
            .await
            .expect("stop");
        tokio::time::sleep(Duration::from_millis(1100)).await; // backup names carry the second
        let (job, status, error) = run_restore(&pool, &provider, restore_payload(&b, true)).await;
        assert_eq!(status, "succeeded", "{error:?}");
        let safety = backup_before_restore(&pool, job)
            .await
            .expect("read")
            .expect("a safety backup is recorded with the job");
        assert!(!safety.scheduled);
        let copies = m.storage().copies.clone();
        assert!(
            copies.iter().any(|c| c.starts_with("shop-db-01-data/bk-")),
            "{copies:?}"
        );
        assert_eq!(
            copies.last().map(String::as_str),
            Some(
                format!(
                    "{} -> local/mock-member-01/shop-db-01-data (refresh)",
                    b.name
                )
                .as_str()
            )
        );
        assert!(
            provider
                .volume_snapshots(
                    "apelora",
                    "local",
                    Some("mock-member-01"),
                    "shop-db-01-data"
                )
                .await
                .expect("snapshots")
                .is_empty()
        );
        let restored = apelora_persistence::recent_audit(&pool, 20)
            .await
            .expect("audit")
            .into_iter()
            .find(|e| e.action == "application.volume.restored")
            .expect("audited");
        assert_eq!(restored.target, "apelora/shop-db-01-data");

        // Without a safety backup only the copy is made; a missing backup is refused.
        let before = volume_backups(&pool, "shop", "shop-db-01-data")
            .await
            .expect("backups")
            .len();
        let (_, status, _) = run_restore(&pool, &provider, restore_payload(&b, false)).await;
        assert_eq!(status, "succeeded");
        assert_eq!(
            volume_backups(&pool, "shop", "shop-db-01-data")
                .await
                .expect("backups")
                .len(),
            before
        );
        let mut gone = restore_payload(&b, false);
        gone["backup"] = json!("shop-db-01-data-bk-19990101000000");
        let (_, status, error) = run_restore(&pool, &provider, gone).await;
        assert_eq!(status, "failed");
        assert!(error.unwrap_or_default().contains("is not in Incus"));
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn a_restore_cancelled_after_its_safety_backup_leaves_the_volume_and_keeps_the_backup(
        pool: Pool,
    ) {
        let (m, provider, _) = deployed(&pool).await;
        let b = backed_up(&pool, &provider).await;
        provider
            .change_state(
                "apelora",
                "shop-db-01",
                apelora_domain::InstanceAction::Stop,
                false,
            )
            .await
            .expect("stop");
        tokio::time::sleep(Duration::from_millis(1100)).await;
        let job = enqueue_job(
            &pool,
            "volume.restore",
            None,
            &restore_payload(&b, true),
            None,
        )
        .await
        .expect("enqueue")
        .id();
        let claimed = apelora_persistence::claim_job(&pool, "test", 30)
            .await
            .expect("claim")
            .expect("claimed");
        assert_eq!(claimed.id, job);
        // Asked to cancel while it runs: it stops at the safe point after the safety backup.
        sqlx::query("UPDATE jobs SET cancel_requested_at = now(), cancel_requested_by = 'user:x' WHERE id = $1")
            .bind(job)
            .execute(&pool)
            .await
            .expect("cancel");
        let outcome = run_restore_job(&pool, &provider, &claimed)
            .await
            .expect("run");
        let Outcome::Cancelled(note) = outcome else {
            panic!("expected a cancellation, got {outcome:?}");
        };
        assert!(note.contains("the safety backup"), "{note}");
        assert!(
            backup_before_restore(&pool, job)
                .await
                .expect("read")
                .is_some()
        );
        assert!(
            !m.storage().copies.iter().any(|c| c.ends_with("(refresh)")),
            "the volume was not overwritten"
        );
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn backups_go_to_another_member_and_only_scheduled_ones_are_pruned(pool: Pool) {
        let (m, provider, id) = deployed(&pool).await;
        set_volume_schedules(&pool, id, None, Some((60, 1)))
            .await
            .expect("schedule");

        // Without a backup pool the schedule says so and nothing is copied.
        scheduled_pass(&pool, &provider, None).await.expect("pass");
        let v = &application_volumes(&pool, "shop").await.expect("volumes")[0];
        assert!(
            v.backup_error
                .as_deref()
                .unwrap_or_default()
                .contains("APELORA_BACKUP_POOL")
        );
        assert!(m.storage().copies.is_empty());

        due_now(&pool, "next_backup_at").await;
        scheduled_pass(&pool, &provider, Some("backup"))
            .await
            .expect("pass");
        let first = volume_backups(&pool, "shop", "shop-db-01-data")
            .await
            .expect("backups");
        assert_eq!(first.len(), 1);
        assert_eq!(
            first[0].member, "mock-member-02",
            "on another member than the volume's"
        );
        assert!(first[0].snapshot.starts_with("bk-"));
        assert!(
            m.storage().copies[0].contains("shop-db-01-data/bk-"),
            "{:?}",
            m.storage().copies
        );
        // The temporary snapshot is gone.
        assert!(m.storage().snapshots.values().all(Vec::is_empty));

        // A backup someone asked for is kept; the older scheduled one gives way to the newer.
        tokio::time::sleep(Duration::from_millis(1100)).await;
        let job = enqueue_job(&pool, "volume.backup", None, &json!({ "application": "shop", "project": "apelora", "module": "db-01",
            "volume": "shop-db-01-data", "pool": "local", "member": "mock-member-01", "sizeMib": 1024, "backupPool": "backup" }), None)
            .await
            .expect("enqueue")
            .id();
        while crate::run_once(&pool, &provider, "test")
            .await
            .expect("run")
        {}
        assert_eq!(
            apelora_persistence::get_job(&pool, job)
                .await
                .expect("job")
                .status,
            "succeeded"
        );
        due_now(&pool, "next_backup_at").await;
        scheduled_pass(&pool, &provider, Some("backup"))
            .await
            .expect("pass");
        let now = volume_backups(&pool, "shop", "shop-db-01-data")
            .await
            .expect("backups");
        assert_eq!(now.iter().filter(|b| b.scheduled).count(), 1);
        assert_eq!(now.iter().filter(|b| !b.scheduled).count(), 1);
        assert!(
            !now.iter().any(|b| b.name == first[0].name),
            "the older scheduled backup was pruned"
        );
        assert!(
            !m.storage()
                .volumes
                .keys()
                .any(|k| k.ends_with(&first[0].name))
        );
    }
}

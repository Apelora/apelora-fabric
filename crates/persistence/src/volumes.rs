// SPDX-License-Identifier: AGPL-3.0-only
//! Volumes Apelora created (storage step 2). Written by the worker, read and deleted by the API.

use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{Pool, Result};

#[derive(Debug, Clone, PartialEq, FromRow)]
pub struct VolumeRecord {
    pub id: Uuid,
    pub project: String,
    pub module: String,
    pub instance: String,
    pub name: String,
    pub incus_name: String,
    pub storage_pool: String,
    pub member: String,
    pub size_mib: i32,
    pub content: String,
    pub class: String,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub snapshot_every_minutes: Option<i32>,
    pub snapshot_keep: Option<i32>,
    pub last_snapshot_at: Option<OffsetDateTime>,
    pub snapshot_error: Option<String>,
    pub backup_every_minutes: Option<i32>,
    pub backup_keep: Option<i32>,
    pub last_backup_at: Option<OffsetDateTime>,
    pub backup_error: Option<String>,
}

/// A volume that now exists in Incus.
pub struct NewVolume<'a> {
    pub application: &'a str,
    pub project: &'a str,
    pub module: &'a str,
    pub instance: &'a str,
    pub name: &'a str,
    pub incus_name: &'a str,
    pub storage_pool: &'a str,
    pub member: &'a str,
    pub size_mib: u32,
    pub content: &'a str,
    pub class: &'a str,
}

/// Records a volume (again): repeated calls keep one row. `false` when the application is not
/// known to the database (an instance created outside a deployment).
pub async fn record_volume(pool: &Pool, v: &NewVolume<'_>) -> Result<bool> {
    let size = i32::try_from(v.size_mib).unwrap_or(i32::MAX);
    Ok(sqlx::query(
        "INSERT INTO volumes (id, application_id, project, module, instance, name, incus_name,
                              storage_pool, member, size_mib, content, class)
         SELECT $1, a.id, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12
         FROM applications a WHERE a.name = $2
         ON CONFLICT (project, storage_pool, incus_name) DO UPDATE SET
           member = EXCLUDED.member, size_mib = EXCLUDED.size_mib, updated_at = now()",
    )
    .bind(Uuid::new_v4())
    .bind(v.application)
    .bind(v.project)
    .bind(v.module)
    .bind(v.instance)
    .bind(v.name)
    .bind(v.incus_name)
    .bind(v.storage_pool)
    .bind(v.member)
    .bind(size)
    .bind(v.content)
    .bind(v.class)
    .execute(pool)
    .await?
    .rows_affected()
        > 0)
}

const COLUMNS: &str = "v.id, v.project, v.module, v.instance, v.name, v.incus_name,
    v.storage_pool, v.member, v.size_mib, v.content, v.class, v.created_at, v.updated_at,
    v.snapshot_every_minutes, v.snapshot_keep, v.last_snapshot_at, v.snapshot_error,
    v.backup_every_minutes, v.backup_keep, v.last_backup_at, v.backup_error";

/// The application's volumes, by module and name.
pub async fn application_volumes(pool: &Pool, application: &str) -> Result<Vec<VolumeRecord>> {
    Ok(sqlx::query_as::<_, VolumeRecord>(&format!(
        "SELECT {COLUMNS} FROM volumes v JOIN applications a ON a.id = v.application_id
         WHERE a.name = $1 ORDER BY v.module, v.instance, v.name"
    ))
    .bind(application)
    .fetch_all(pool)
    .await?)
}

/// Every recorded volume, of every application.
pub async fn all_volumes(pool: &Pool) -> Result<Vec<VolumeRecord>> {
    Ok(sqlx::query_as::<_, VolumeRecord>(&format!(
        "SELECT {COLUMNS} FROM volumes v ORDER BY v.member, v.project, v.incus_name"
    ))
    .fetch_all(pool)
    .await?)
}

/// One of the application's volumes, by its Incus name.
pub async fn application_volume(
    pool: &Pool,
    application: &str,
    incus_name: &str,
) -> Result<Option<VolumeRecord>> {
    Ok(sqlx::query_as::<_, VolumeRecord>(&format!(
        "SELECT {COLUMNS} FROM volumes v JOIN applications a ON a.id = v.application_id
         WHERE a.name = $1 AND v.incus_name = $2"
    ))
    .bind(application)
    .bind(incus_name)
    .fetch_optional(pool)
    .await?)
}

/// Records a volume's new size after it grew.
pub async fn set_volume_size(pool: &Pool, id: Uuid, size_mib: u32) -> Result<()> {
    sqlx::query("UPDATE volumes SET size_mib = $2, updated_at = now() WHERE id = $1")
        .bind(id)
        .bind(i32::try_from(size_mib).unwrap_or(i32::MAX))
        .execute(pool)
        .await?;
    Ok(())
}

/// Forgets a volume once it is deleted in Incus.
pub async fn forget_volume(pool: &Pool, id: Uuid) -> Result<bool> {
    Ok(sqlx::query("DELETE FROM volumes WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}

/// Which scheduled task a volume is due for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleKind {
    Snapshot,
    Backup,
}

impl ScheduleKind {
    fn columns(self) -> (&'static str, &'static str, &'static str, &'static str) {
        match self {
            ScheduleKind::Snapshot => (
                "snapshot_every_minutes",
                "next_snapshot_at",
                "last_snapshot_at",
                "snapshot_error",
            ),
            ScheduleKind::Backup => (
                "backup_every_minutes",
                "next_backup_at",
                "last_backup_at",
                "backup_error",
            ),
        }
    }
}

/// Sets (or, with `None`, clears) a volume's snapshot and backup schedules. A new schedule
/// first runs at the next pass of the worker.
pub async fn set_volume_schedules(
    pool: &Pool,
    id: Uuid,
    snapshots: Option<(u32, u32)>,
    backups: Option<(u32, u32)>,
) -> Result<()> {
    let n = |v: Option<(u32, u32)>, f: fn((u32, u32)) -> u32| {
        v.map(|x| i32::try_from(f(x)).unwrap_or(i32::MAX))
    };
    sqlx::query(
        "UPDATE volumes SET
           snapshot_every_minutes = $2, snapshot_keep = $3,
           next_snapshot_at = CASE WHEN $2::int IS NULL THEN NULL
                                   WHEN snapshot_every_minutes IS DISTINCT FROM $2 THEN now()
                                   ELSE next_snapshot_at END,
           snapshot_error = CASE WHEN $2::int IS NULL THEN NULL ELSE snapshot_error END,
           backup_every_minutes = $4, backup_keep = $5,
           next_backup_at = CASE WHEN $4::int IS NULL THEN NULL
                                 WHEN backup_every_minutes IS DISTINCT FROM $4 THEN now()
                                 ELSE next_backup_at END,
           backup_error = CASE WHEN $4::int IS NULL THEN NULL ELSE backup_error END,
           updated_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(n(snapshots, |x| x.0))
    .bind(n(snapshots, |x| x.1))
    .bind(n(backups, |x| x.0))
    .bind(n(backups, |x| x.1))
    .execute(pool)
    .await?;
    Ok(())
}

/// A volume due for a scheduled task, with the application it belongs to and how many results
/// to keep.
#[derive(Debug, Clone, PartialEq, FromRow)]
pub struct DueVolume {
    pub id: Uuid,
    pub application: String,
    pub project: String,
    pub module: String,
    pub incus_name: String,
    pub storage_pool: String,
    pub member: String,
    pub size_mib: i32,
    pub keep: i32,
}

/// Claims up to `limit` volumes due for `kind` and moves their next run one interval ahead,
/// in one statement: two workers never claim the same volume.
pub async fn claim_due(pool: &Pool, kind: ScheduleKind, limit: i64) -> Result<Vec<DueVolume>> {
    let (every, next, _, _) = kind.columns();
    let keep = match kind {
        ScheduleKind::Snapshot => "snapshot_keep",
        ScheduleKind::Backup => "backup_keep",
    };
    Ok(sqlx::query_as::<_, DueVolume>(&format!(
        "WITH due AS (
           SELECT id FROM volumes
           WHERE {every} IS NOT NULL AND {next} IS NOT NULL AND {next} <= now()
           ORDER BY {next} LIMIT $1 FOR UPDATE SKIP LOCKED)
         UPDATE volumes v SET {next} = now() + make_interval(mins => v.{every})
         FROM due, applications a
         WHERE v.id = due.id AND a.id = v.application_id
         RETURNING v.id, a.name AS application, v.project, v.module, v.incus_name,
                   v.storage_pool, v.member, v.size_mib, v.{keep} AS keep"
    ))
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// Records how a scheduled task went. A failure is retried in five minutes, not after a whole
/// interval.
pub async fn record_scheduled(
    pool: &Pool,
    id: Uuid,
    kind: ScheduleKind,
    error: Option<&str>,
) -> Result<()> {
    let (_, next, last, err) = kind.columns();
    let sql = match error {
        None => format!("UPDATE volumes SET {last} = now(), {err} = NULL WHERE id = $1"),
        Some(_) => format!(
            "UPDATE volumes SET {err} = $2, {next} = LEAST({next}, now() + interval '5 minutes') WHERE id = $1"
        ),
    };
    sqlx::query(&sql).bind(id).bind(error).execute(pool).await?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, FromRow)]
pub struct BackupRecord {
    pub id: Uuid,
    pub project: String,
    pub source: String,
    pub snapshot: String,
    pub name: String,
    pub storage_pool: String,
    pub member: String,
    pub size_mib: i32,
    pub scheduled: bool,
    pub created_at: OffsetDateTime,
}

pub struct NewBackup<'a> {
    pub application: &'a str,
    pub project: &'a str,
    pub source: &'a str,
    pub snapshot: &'a str,
    pub name: &'a str,
    pub storage_pool: &'a str,
    pub member: &'a str,
    pub size_mib: u32,
    pub scheduled: bool,
}

pub async fn record_backup(pool: &Pool, b: &NewBackup<'_>) -> Result<()> {
    sqlx::query(
        "INSERT INTO volume_backups (id, application_id, project, source, snapshot, name,
                                     storage_pool, member, size_mib, scheduled)
         SELECT $1, a.id, $3, $4, $5, $6, $7, $8, $9, $10 FROM applications a WHERE a.name = $2
         ON CONFLICT (project, storage_pool, name) DO NOTHING",
    )
    .bind(Uuid::new_v4())
    .bind(b.application)
    .bind(b.project)
    .bind(b.source)
    .bind(b.snapshot)
    .bind(b.name)
    .bind(b.storage_pool)
    .bind(b.member)
    .bind(i32::try_from(b.size_mib).unwrap_or(i32::MAX))
    .bind(b.scheduled)
    .execute(pool)
    .await?;
    Ok(())
}

/// The backups of one of the application's volumes, oldest first.
pub async fn volume_backups(
    pool: &Pool,
    application: &str,
    source: &str,
) -> Result<Vec<BackupRecord>> {
    Ok(sqlx::query_as::<_, BackupRecord>(
        "SELECT b.id, b.project, b.source, b.snapshot, b.name, b.storage_pool, b.member,
                b.size_mib, b.scheduled, b.created_at
         FROM volume_backups b JOIN applications a ON a.id = b.application_id
         WHERE a.name = $1 AND b.source = $2 ORDER BY b.created_at, b.name",
    )
    .bind(application)
    .bind(source)
    .fetch_all(pool)
    .await?)
}

pub async fn forget_backup(pool: &Pool, id: Uuid) -> Result<bool> {
    Ok(sqlx::query("DELETE FROM volume_backups WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}

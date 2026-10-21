// SPDX-License-Identifier: AGPL-3.0-only
//! Is Apelora's own database being backed up, and was a restore proven? Reads what the backup and
//! restore-drill scripts leave in the backup directory (`APELORA_DB_BACKUP_DIR`) and says what it
//! means (`apelora_domain::dbbackup`). Read-only: nothing here runs a backup, reads a dump or
//! writes a file, and the control plane needs only to read the directory (the dumps may be on
//! storage it cannot read; the manifests next to them are what it needs). Needs `cluster.view`.

use std::path::{Path, PathBuf};

use apelora_domain::Permission;
use apelora_domain::dbbackup::{self, BackupAssessment, BackupInfo, DrillInfo, Read};
use axum::extract::State;
use axum::{Extension, Json};
use serde::Serialize;
use utoipa::ToSchema;

use super::{ApiError, AppState, ErrorBody, Principal, require};

/// A record bigger than this is not one of ours.
const MAX_RECORD_BYTES: u64 = 1024 * 1024;
/// More files than this in the directory and the newest-first scan stops looking further back.
const MAX_ENTRIES: usize = 20_000;

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseBackupReport {
    /// `APELORA_DB_BACKUP_DIR` is set. When false, nothing else is known.
    pub configured: bool,
    /// The directory the records are read from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub directory: Option<String>,
    /// How many backup manifests the directory holds.
    pub backups: usize,
    pub newest_backup: Option<BackupInfo>,
    /// The latest restore drill, whether it passed or failed.
    pub last_drill: Option<DrillInfo>,
    /// The latest drill that passed.
    pub last_verified: Option<DrillInfo>,
    pub assessment: Option<BackupAssessment>,
}

fn read_text(path: &Path) -> Result<String, String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if meta.len() > MAX_RECORD_BYTES {
        return Err("larger than a record should be".into());
    }
    std::fs::read_to_string(path).map_err(|e| e.to_string())
}

fn read_record<T>(path: &Path, parse: fn(&str) -> Result<T, String>) -> Read<T> {
    if !path.exists() {
        return Read::Missing;
    }
    match read_text(path) {
        Ok(t) => parse(&t).map_or_else(Read::Unreadable, Read::Found),
        Err(e) => Read::Unreadable(e),
    }
}

/// Reads the directory: the newest backup manifest (by its own recorded time), how many there
/// are, and the two drill records.
fn read_directory(dir: &Path) -> DatabaseBackupReport {
    let listing = match std::fs::read_dir(dir) {
        Ok(l) => l,
        Err(e) => {
            return report(
                dir,
                0,
                Read::Unreadable(format!("the backup directory cannot be read: {e}")),
                Read::Missing,
                Read::Missing,
            );
        }
    };
    let mut manifests: Vec<PathBuf> = listing
        .filter_map(Result::ok)
        .take(MAX_ENTRIES)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("apelora-db-") && n.ends_with(".json"))
        })
        .collect();
    // File names carry their UTC time, so the last one by name is the newest; every candidate
    // is still judged by the time inside it.
    manifests.sort();
    let count = manifests.len();
    let mut newest: Read<BackupInfo> = Read::Missing;
    for path in manifests.iter().rev().take(5) {
        match read_record(path, dbbackup::parse_backup) {
            Read::Found(b) => {
                let better = match &newest {
                    Read::Found(cur) => b.created_at > cur.created_at,
                    _ => true,
                };
                if better {
                    newest = Read::Found(b);
                }
            }
            Read::Unreadable(why) if matches!(newest, Read::Missing) => {
                newest = Read::Unreadable(format!("{}: {why}", path.display()));
            }
            _ => {}
        }
    }
    report(
        dir,
        count,
        newest,
        read_record(&dir.join("restore-drill-last.json"), dbbackup::parse_drill),
        read_record(
            &dir.join("restore-drill-last-verified.json"),
            dbbackup::parse_drill,
        ),
    )
}

fn report(
    dir: &Path,
    backups: usize,
    newest: Read<BackupInfo>,
    last: Read<DrillInfo>,
    verified: Read<DrillInfo>,
) -> DatabaseBackupReport {
    let assessment = dbbackup::assess(&newest, &last, &verified, time::OffsetDateTime::now_utc());
    fn found<T>(r: Read<T>) -> Option<T> {
        match r {
            Read::Found(v) => Some(v),
            _ => None,
        }
    }
    DatabaseBackupReport {
        configured: true,
        directory: Some(dir.display().to_string()),
        backups,
        newest_backup: found(newest),
        last_drill: found(last),
        last_verified: found(verified),
        assessment: Some(assessment),
    }
}

/// Whether Apelora's database is being backed up and a restore has been proven, from the records
/// the scripts leave in `APELORA_DB_BACKUP_DIR`. Needs `cluster.view`.
#[utoipa::path(get, path = "/api/v1/database-backups", tag = "system", operation_id = "database_backups",
    responses((status = 200, body = DatabaseBackupReport), (status = 401, body = ErrorBody), (status = 403, body = ErrorBody)))]
pub(super) async fn database_backups(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<DatabaseBackupReport>, ApiError> {
    require(&p, Permission::ReadInventory)?;
    let Some(dir) = state.db_backup_dir.as_deref().cloned() else {
        return Ok(Json(DatabaseBackupReport {
            configured: false,
            directory: None,
            backups: 0,
            newest_backup: None,
            last_drill: None,
            last_verified: None,
            assessment: None,
        }));
    };
    let report = tokio::task::spawn_blocking(move || read_directory(&dir))
        .await
        .map_err(|e| {
            ApiError::new(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                e.to_string(),
            )
        })?;
    Ok(Json(report))
}

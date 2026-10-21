// SPDX-License-Identifier: AGPL-3.0-only
//! Is Apelora's own database being backed up, and has a restore been proven? Judged from the
//! records the backup and restore-drill scripts leave in the backup directory
//! (`scripts/ops/db-backup.sh`, `db-restore-drill.sh`). Pure: the control plane reads the files,
//! this decides what they mean. Apelora never runs the scripts and never reads a backup itself.

use serde::Serialize;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// A backup older than this has missed its daily run (the runbook alerts at 26 hours).
pub const BACKUP_WARN_HOURS: i64 = 26;
/// Older than this, and a night of failures is no longer the likely explanation.
pub const BACKUP_ERROR_HOURS: i64 = 72;
/// The runbook alerts when no restore has been verified for 35 days.
pub const DRILL_WARN_DAYS: i64 = 35;
pub const DRILL_ERROR_DAYS: i64 = 70;

/// The manifest next to a dump (`apelora-db-backup/1`), as far as the Console shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct BackupInfo {
    pub file: String,
    pub created_at: String,
    pub size_bytes: u64,
    /// `none`, or how the dump is encrypted (`age`, `cms`).
    pub encryption: String,
    pub pg_dump_version: String,
    pub duration_seconds: u64,
    /// Migrations the database had when it was dumped.
    pub migrations: Option<u64>,
}

/// A restore drill's record (`apelora-restore-drill/1`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct DrillInfo {
    pub drilled_at: String,
    /// The backup file the drill restored.
    pub backup: String,
    pub passed: bool,
    pub reason: String,
    pub duration_seconds: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum BackupState {
    Ok,
    Warning,
    Failing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct BackupFinding {
    pub level: BackupState,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct BackupAssessment {
    pub state: BackupState,
    pub backup_age_hours: Option<i64>,
    pub drill_age_days: Option<i64>,
    /// Every reason the state is not `ok`, worst first; one line saying so when it is.
    pub findings: Vec<BackupFinding>,
}

/// What reading one record gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Read<T> {
    Found(T),
    /// No such record yet.
    Missing,
    /// The file exists but cannot be used (permissions, damaged, unknown format).
    Unreadable(String),
}

fn age(at: &str, now: OffsetDateTime) -> Option<i64> {
    OffsetDateTime::parse(at, &Rfc3339)
        .ok()
        .map(|t| (now - t).whole_seconds().max(0))
}

/// The judgement, from the newest backup, the newest drill (pass or fail) and the newest drill that
/// passed.
pub fn assess(
    backup: &Read<BackupInfo>,
    last_drill: &Read<DrillInfo>,
    last_verified: &Read<DrillInfo>,
    now: OffsetDateTime,
) -> BackupAssessment {
    use BackupState::{Failing, Ok, Warning};
    let mut findings: Vec<BackupFinding> = Vec::new();
    let mut add = |level, text: String| findings.push(BackupFinding { level, text });

    let mut backup_age_hours = None;
    match backup {
        Read::Missing => add(
            Failing,
            "No backup of Apelora's database has been written yet.".into(),
        ),
        Read::Unreadable(why) => add(
            Warning,
            format!("The newest backup record cannot be read: {why}"),
        ),
        Read::Found(b) => match age(&b.created_at, now) {
            None => add(
                Warning,
                "The newest backup record has no readable time.".into(),
            ),
            Some(secs) => {
                let hours = secs / 3600;
                backup_age_hours = Some(hours);
                if hours >= BACKUP_ERROR_HOURS {
                    add(
                        Failing,
                        format!(
                            "The newest backup is {hours} hours old; daily backups have stopped."
                        ),
                    );
                } else if hours >= BACKUP_WARN_HOURS {
                    add(
                        Warning,
                        format!("The newest backup is {hours} hours old; a daily run was missed."),
                    );
                }
                if b.encryption == "none" {
                    add(Warning, "The newest backup is not encrypted. It holds the audit trail and every stored secret reference.".into());
                }
                if b.size_bytes == 0 {
                    add(Failing, "The newest backup is empty.".into());
                }
            }
        },
    }

    let mut drill_age_days = None;
    match last_verified {
        Read::Missing => add(Warning, "No restore of Apelora's database has been verified yet. A backup that was never restored is a hope, not a backup.".into()),
        Read::Unreadable(why) => add(Warning, format!("The record of the last verified restore cannot be read: {why}")),
        Read::Found(d) => match age(&d.drilled_at, now) {
            None => add(Warning, "The last verified restore has no readable time.".into()),
            Some(secs) => {
                let days = secs / 86_400;
                drill_age_days = Some(days);
                if days >= DRILL_ERROR_DAYS {
                    add(Failing, format!("The last verified restore was {days} days ago."));
                } else if days >= DRILL_WARN_DAYS {
                    add(Warning, format!("The last verified restore was {days} days ago; drill it again."));
                }
            }
        },
    }
    // A failed drill newer than the last pass is the most important thing to know.
    if let (Read::Found(last), verified) = (last_drill, last_verified)
        && !last.passed
    {
        let newer = match verified {
            Read::Found(v) => {
                OffsetDateTime::parse(&last.drilled_at, &Rfc3339).ok()
                    > OffsetDateTime::parse(&v.drilled_at, &Rfc3339).ok()
            }
            _ => true,
        };
        if newer {
            let why = if last.reason.is_empty() {
                "no reason recorded"
            } else {
                last.reason.as_str()
            };
            add(Failing, format!("The latest restore drill failed: {why}"));
        }
    }

    findings.sort_by_key(|f| std::cmp::Reverse(f.level));
    let state = findings.first().map_or(Ok, |f| f.level);
    if findings.is_empty() {
        findings.push(BackupFinding {
            level: Ok,
            text: "Backed up recently, and a restore was verified recently.".into(),
        });
    }
    BackupAssessment {
        state,
        backup_age_hours,
        drill_age_days,
        findings,
    }
}

/// The manifest of a dump, from its JSON.
pub fn parse_backup(text: &str) -> Result<BackupInfo, String> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("not JSON ({e})"))?;
    if v["format"] != "apelora-db-backup/1" {
        return Err("not an apelora-db-backup/1 manifest".into());
    }
    let s = |k: &str| v[k].as_str().unwrap_or_default().to_owned();
    let n = |k: &str| v[k].as_u64().unwrap_or(0);
    if s("created_at").is_empty() || s("file").is_empty() {
        return Err("the manifest has no file name or time".into());
    }
    Ok(BackupInfo {
        file: s("file"),
        created_at: s("created_at"),
        size_bytes: n("size_bytes"),
        encryption: s("encryption"),
        pg_dump_version: s("pg_dump_version"),
        duration_seconds: n("duration_seconds"),
        migrations: v["contents"]["migrations"]["count"].as_u64(),
    })
}

/// A restore drill's record, from its JSON.
pub fn parse_drill(text: &str) -> Result<DrillInfo, String> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("not JSON ({e})"))?;
    if v["format"] != "apelora-restore-drill/1" {
        return Err("not an apelora-restore-drill/1 record".into());
    }
    let s = |k: &str| v[k].as_str().unwrap_or_default().to_owned();
    if s("drilled_at").is_empty() {
        return Err("the record has no time".into());
    }
    Ok(DrillInfo {
        drilled_at: s("drilled_at"),
        backup: s("backup"),
        passed: v["verdict"] == "pass",
        reason: s("reason"),
        duration_seconds: v["duration_seconds"].as_u64().unwrap_or(0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_792_000_000;

    fn now() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(NOW).expect("time")
    }
    fn ago(secs: i64) -> String {
        OffsetDateTime::from_unix_timestamp(NOW - secs)
            .expect("time")
            .format(&Rfc3339)
            .expect("format")
    }
    fn backup(age_secs: i64, encryption: &str) -> Read<BackupInfo> {
        Read::Found(BackupInfo {
            file: "apelora-db-x.dump".into(),
            created_at: ago(age_secs),
            size_bytes: 1000,
            encryption: encryption.into(),
            pg_dump_version: "pg_dump 18".into(),
            duration_seconds: 3,
            migrations: Some(28),
        })
    }
    fn drill(age_secs: i64, passed: bool, reason: &str) -> Read<DrillInfo> {
        Read::Found(DrillInfo {
            drilled_at: ago(age_secs),
            backup: "apelora-db-x.dump".into(),
            passed,
            reason: reason.into(),
            duration_seconds: 9,
        })
    }
    const H: i64 = 3600;
    const D: i64 = 86_400;

    #[test]
    fn a_recent_encrypted_backup_and_a_recent_verified_restore_is_ok() {
        let a = assess(
            &backup(2 * H, "age"),
            &drill(D, true, ""),
            &drill(D, true, ""),
            now(),
        );
        assert_eq!(a.state, BackupState::Ok);
        assert_eq!((a.backup_age_hours, a.drill_age_days), (Some(2), Some(1)));
        assert_eq!(a.findings.len(), 1);
    }

    #[test]
    fn a_backup_goes_from_fine_to_warning_to_failing_with_age() {
        let state = |h: i64| {
            assess(
                &backup(h * H, "age"),
                &drill(D, true, ""),
                &drill(D, true, ""),
                now(),
            )
            .state
        };
        assert_eq!(state(25), BackupState::Ok);
        assert_eq!(state(26), BackupState::Warning);
        assert_eq!(state(71), BackupState::Warning);
        assert_eq!(state(72), BackupState::Failing);
    }

    #[test]
    fn a_verified_restore_goes_stale_after_five_weeks() {
        let state = |d: i64| {
            assess(
                &backup(H, "age"),
                &drill(d * D, true, ""),
                &drill(d * D, true, ""),
                now(),
            )
            .state
        };
        assert_eq!(state(34), BackupState::Ok);
        assert_eq!(state(35), BackupState::Warning);
        assert_eq!(state(69), BackupState::Warning);
        assert_eq!(state(70), BackupState::Failing);
    }

    #[test]
    fn missing_pieces_are_said_plainly() {
        let none = assess(&Read::Missing, &Read::Missing, &Read::Missing, now());
        assert_eq!(none.state, BackupState::Failing);
        assert!(none.findings[0].text.contains("No backup"));
        let no_drill = assess(&backup(H, "age"), &Read::Missing, &Read::Missing, now());
        assert_eq!(no_drill.state, BackupState::Warning);
        assert!(
            no_drill.findings[0].text.contains("never restored")
                || no_drill.findings[0].text.contains("No restore")
        );
        let unreadable = assess(
            &Read::Unreadable("permission denied".into()),
            &Read::Missing,
            &Read::Missing,
            now(),
        );
        assert!(
            unreadable
                .findings
                .iter()
                .any(|f| f.text.contains("permission denied"))
        );
    }

    #[test]
    fn an_unencrypted_or_empty_backup_is_called_out() {
        let plain = assess(
            &backup(H, "none"),
            &drill(D, true, ""),
            &drill(D, true, ""),
            now(),
        );
        assert_eq!(plain.state, BackupState::Warning);
        assert!(plain.findings[0].text.contains("not encrypted"));
        let Read::Found(mut empty) = backup(H, "age") else {
            unreachable!()
        };
        empty.size_bytes = 0;
        let a = assess(
            &Read::Found(empty),
            &drill(D, true, ""),
            &drill(D, true, ""),
            now(),
        );
        assert_eq!(a.state, BackupState::Failing);
    }

    #[test]
    fn a_failed_drill_newer_than_the_last_pass_is_failing_and_an_older_one_is_not() {
        let a = assess(
            &backup(H, "age"),
            &drill(H, false, "the audit trigger is missing"),
            &drill(10 * D, true, ""),
            now(),
        );
        assert_eq!(a.state, BackupState::Failing);
        assert!(a.findings[0].text.contains("audit trigger is missing"));
        // A pass since then clears it.
        let b = assess(
            &backup(H, "age"),
            &drill(D, true, ""),
            &drill(D, true, ""),
            now(),
        );
        assert_eq!(b.state, BackupState::Ok);
        // A failure that happened before the last pass is history.
        let c = assess(
            &backup(H, "age"),
            &drill(5 * D, false, "old"),
            &drill(D, true, ""),
            now(),
        );
        assert_eq!(c.state, BackupState::Ok);
        // A failed drill with no pass ever recorded counts.
        let d = assess(
            &backup(H, "age"),
            &drill(H, false, ""),
            &Read::Missing,
            now(),
        );
        assert_eq!(d.state, BackupState::Failing);
        assert!(
            d.findings
                .iter()
                .any(|f| f.text.contains("no reason recorded"))
        );
    }

    #[test]
    fn the_scripts_records_are_read_and_anything_else_is_refused() {
        let b = parse_backup(r#"{"format":"apelora-db-backup/1","file":"apelora-db-a.dump","created_at":"2026-10-21T01:00:00Z","sha256":"x","size_bytes":4096,"encryption":"age","pg_dump_version":"pg_dump 18.1","duration_seconds":7,"contents":{"migrations":{"count":28,"latest":28}}}"#).expect("manifest");
        assert_eq!(
            (b.size_bytes, b.migrations, b.encryption.as_str()),
            (4096, Some(28), "age")
        );
        assert!(parse_backup(r#"{"format":"other/1"}"#).is_err());
        assert!(parse_backup("{").is_err());
        assert!(
            parse_backup(r#"{"format":"apelora-db-backup/1","file":"","created_at":""}"#).is_err()
        );
        let d = parse_drill(r#"{"format":"apelora-restore-drill/1","drilled_at":"2026-10-20T04:00:00Z","backup":"a","verdict":"pass","reason":"","duration_seconds":12}"#).expect("drill");
        assert!(d.passed && d.duration_seconds == 12);
        let f = parse_drill(r#"{"format":"apelora-restore-drill/1","drilled_at":"2026-10-20T04:00:00Z","verdict":"fail","reason":"checksum"}"#).expect("drill");
        assert!(!f.passed && f.reason == "checksum");
        assert!(parse_drill(r#"{"format":"apelora-restore-drill/1"}"#).is_err());
        assert!(parse_drill(r#"{"verdict":"pass"}"#).is_err());
    }
}

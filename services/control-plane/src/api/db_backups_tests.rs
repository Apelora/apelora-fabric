// The database backup check through the real router: records written the way the backup and
// restore-drill scripts write them, in a temporary directory (MOCK VERIFIED; no real backup).

#![allow(clippy::unwrap_used)] // test fixtures: a failed unwrap is the failure

use std::path::{Path, PathBuf};

use apelora_incus_adapter::MockIncus;
use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::identity_tests::{lab, world};
use super::tests::send;
use super::*;

const PATH: &str = "/api/v1/database-backups";

fn temp() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("apelora-dbb-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn ago(hours: i64) -> String {
    (OffsetDateTime::now_utc() - time::Duration::hours(hours))
        .format(&Rfc3339)
        .unwrap()
}

fn manifest(dir: &Path, name: &str, hours_ago: i64, encryption: &str, size: u64) {
    let body = json!({ "format": "apelora-db-backup/1", "file": format!("{name}.dump"), "created_at": ago(hours_ago), "sha256": "x",
        "size_bytes": size, "encryption": encryption, "pg_dump_version": "pg_dump 18", "duration_seconds": 5,
        "contents": { "migrations": { "count": 28, "latest": 28 } } });
    std::fs::write(dir.join(format!("{name}.json")), body.to_string()).unwrap();
}

fn drill(dir: &Path, file: &str, hours_ago: i64, verdict: &str, reason: &str) {
    let body = json!({ "format": "apelora-restore-drill/1", "drilled_at": ago(hours_ago), "backup": "x.dump", "verdict": verdict, "reason": reason, "duration_seconds": 9 });
    std::fs::write(dir.join(file), body.to_string()).unwrap();
}

async fn report(dir: Option<&Path>) -> Value {
    let state = AppState::new(
        Provider::Mock(MockIncus::default()),
        Auth::DisabledForDevelopment,
    )
    .with_db_backup_dir(dir.map(Path::to_path_buf));
    let (status, body) = send(state, "GET", PATH, "unused-in-development", None, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

#[tokio::test]
async fn without_a_backup_directory_it_says_nothing_is_known() {
    let r = report(None).await;
    assert_eq!(r["configured"], json!(false));
    assert!(r["assessment"].is_null() && r["newestBackup"].is_null());
}

#[tokio::test]
async fn a_recent_backup_and_a_verified_restore_are_ok() {
    let dir = temp();
    manifest(&dir, "apelora-db-20261021T020000Z", 3, "age", 4096);
    drill(&dir, "restore-drill-last.json", 20, "pass", "");
    drill(&dir, "restore-drill-last-verified.json", 20, "pass", "");
    let r = report(Some(&dir)).await;
    assert_eq!(r["configured"], json!(true));
    assert_eq!(r["backups"], json!(1));
    assert_eq!(r["assessment"]["state"], json!("ok"), "{r}");
    assert_eq!(r["assessment"]["backupAgeHours"], json!(3));
    assert_eq!(r["newestBackup"]["encryption"], json!("age"));
    assert_eq!(r["newestBackup"]["migrations"], json!(28));
    assert_eq!(r["lastVerified"]["passed"], json!(true));
    assert_eq!(r["directory"], json!(dir.display().to_string()));
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn the_newest_backup_is_chosen_by_its_recorded_time_and_stray_files_are_ignored() {
    let dir = temp();
    manifest(&dir, "apelora-db-20261019T020000Z", 60, "age", 10);
    manifest(&dir, "apelora-db-20261020T020000Z", 30, "age", 10);
    // Named earlier but recorded as the newest: the recorded time decides.
    manifest(&dir, "apelora-db-20261018T020000Z", 1, "age", 77);
    std::fs::write(dir.join("apelora-db-broken.json"), "not json").unwrap();
    std::fs::write(dir.join("notes.json"), "{}").unwrap();
    std::fs::write(dir.join("apelora-db-big.json"), vec![b' '; 2 * 1024 * 1024]).unwrap();
    drill(&dir, "restore-drill-last-verified.json", 5, "pass", "");
    let r = report(Some(&dir)).await;
    assert_eq!(
        r["backups"],
        json!(5),
        "every apelora-db-*.json counts, readable or not"
    );
    assert_eq!(r["newestBackup"]["sizeBytes"], json!(77), "{r}");
    assert_eq!(r["assessment"]["state"], json!("ok"));
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn old_missing_or_failed_things_are_reported_in_words() {
    // An empty directory: nothing has been backed up.
    let dir = temp();
    let r = report(Some(&dir)).await;
    assert_eq!(r["assessment"]["state"], json!("failing"));
    assert!(
        r["assessment"]["findings"][0]["text"]
            .as_str()
            .unwrap()
            .contains("No backup")
    );

    // A day-old backup with no restore ever verified is a warning, not a failure.
    manifest(&dir, "apelora-db-a", 27, "age", 10);
    let r = report(Some(&dir)).await;
    assert_eq!(r["assessment"]["state"], json!("warning"), "{r}");

    // A failed drill after the last pass is failing, with its reason.
    drill(
        &dir,
        "restore-drill-last-verified.json",
        24 * 10,
        "pass",
        "",
    );
    drill(
        &dir,
        "restore-drill-last.json",
        2,
        "fail",
        "the audit trigger is missing",
    );
    manifest(&dir, "apelora-db-b", 1, "none", 10);
    let r = report(Some(&dir)).await;
    assert_eq!(r["assessment"]["state"], json!("failing"));
    let texts: Vec<&str> = r["assessment"]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["text"].as_str().unwrap())
        .collect();
    assert!(
        texts.iter().any(|t| t.contains("audit trigger is missing")),
        "{texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.contains("not encrypted")),
        "{texts:?}"
    );
    assert_eq!(r["lastDrill"]["passed"], json!(false));
    std::fs::remove_dir_all(&dir).ok();

    // A directory that does not exist is a finding, not a crash.
    let r = report(Some(Path::new("/nonexistent/apelora-backups"))).await;
    assert_ne!(r["assessment"]["state"], json!("ok"));
    assert!(
        r["assessment"]["findings"][0]["text"]
            .as_str()
            .unwrap()
            .contains("cannot be read"),
        "{r}"
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn the_backup_check_needs_cluster_view(pool: Pool) {
    let l = lab(pool);
    let w = world(&l).await;
    assert_eq!(
        l.req("GET", PATH, "", None).await.0,
        StatusCode::UNAUTHORIZED
    );
    let des = l.user("sub-des", "des").await;
    l.grant(
        &w.admin,
        des,
        "application-designer",
        json!({ "type": "project", "id": w.acme_prod }),
    )
    .await;
    assert_eq!(
        l.req("GET", PATH, &l.token("sub-des", "des"), None).await.0,
        StatusCode::FORBIDDEN
    );
    let viewer = l.user("sub-view", "viewer").await;
    l.grant(&w.admin, viewer, "viewer", json!({ "type": "platform" }))
        .await;
    let (s, b) = l
        .req("GET", PATH, &l.token("sub-view", "viewer"), None)
        .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["configured"], json!(false));
}

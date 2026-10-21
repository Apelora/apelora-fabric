// The Storage and Networks areas through the real router, against PostgreSQL and the mock
// Incus: backups on request, schedules, the restore dry run and job, orphaned volumes, pool use,
// and per-application networks with drift. Each route is checked for who may use it (allowed,
// another tenant's application answers 404, missing permission 403, no token 401).

use apelora_incus_adapter::MockIncus;
use serde_json::{Value, json};

use super::identity_tests::Lab;
use super::manage_tests::{Shop, mock_lab, settle, shop};
use super::*;

const VOLUME: &str = "/api/v1/applications/shop/volumes/shop-db-01-data";

fn module(id: &str, availability: &str, volume: bool) -> Value {
    let mut m = super::tests::module(id);
    m["availability"] = json!(availability);
    if volume {
        m["spec"]["volumes"] = json!([{ "name": "data", "sizeMib": 1024, "path": "/data" }]);
    }
    m
}

/// `shop` deployed with a stateless web module (published on 443, internet access for the
/// web) and a database with a volume; the backup pool is configured.
async fn deployed_shop(pool: Pool) -> (Lab, MockIncus, Shop) {
    let (mut l, mock) = mock_lab(pool);
    l.state = l
        .state
        .clone()
        .with_volume_pool(Some("mock-local".into()))
        .with_backup_pool(Some("mock-backup".into()))
        .with_service_range(Some("198.18.100.0/28".into()));
    let s = shop(&l).await;
    let (st, v) = l
        .req(
            "POST",
            "/api/v1/applications/shop/versions",
            &s.owner,
            Some(json!({ "label": "first" })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{v}");
    let mut web = module("web-01", "stateless", false);
    web["internet"] = json!("web");
    let (st, d) = l
        .req(
            "POST",
            "/api/v1/applications/shop/deployments",
            &s.owner,
            Some(json!({ "project": "acme-prod-live", "document": {}, "version": v["revision"],
                "modules": [web, module("db-01", "stateful", true)],
                "connections": [{ "from": "web-01", "to": "db-01", "transport": "tcp", "port": 5432 }],
                "published": [{ "module": "web-01", "transport": "tcp", "port": 443 }] })),
        )
        .await;
    assert_eq!(st, StatusCode::ACCEPTED, "{d}");
    settle(&l).await;
    (l, mock, s)
}

async fn job_status(l: &Lab, id: &Value) -> (String, Option<String>) {
    let id: uuid::Uuid = id.as_str().expect("job id").parse().expect("uuid");
    let j = apelora_persistence::get_job(&l.pool, id)
        .await
        .expect("job");
    (j.status, j.last_error)
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn a_volume_is_backed_up_on_request_and_restored_from_a_backup_through_a_job(pool: Pool) {
    let (l, mock, s) = deployed_shop(pool).await;
    let o = &s.owner;

    // The Storage list shows the volume, attached, with no backups yet.
    let (st, list) = l.req("GET", "/api/v1/storage/volumes", o, None).await;
    assert_eq!(st, StatusCode::OK, "{list}");
    let v = &list["items"][0];
    assert_eq!(
        (
            v["incusName"].as_str(),
            v["state"].as_str(),
            v["backups"].as_i64(),
            list["backupPool"].as_str()
        ),
        (
            Some("shop-db-01-data"),
            Some("attached"),
            Some(0),
            Some("mock-backup")
        ),
        "{list}"
    );

    // Back up now: a job with its audit row.
    let (st, b) = l.req("POST", &format!("{VOLUME}/backups"), o, None).await;
    assert_eq!(st, StatusCode::ACCEPTED, "{b}");
    settle(&l).await;
    assert_eq!(job_status(&l, &b["jobId"]).await.0, "succeeded");
    let (st, backups) = l.req("GET", &format!("{VOLUME}/backups"), o, None).await;
    assert_eq!(st, StatusCode::OK, "{backups}");
    let backup = backups["items"][0]["name"]
        .as_str()
        .expect("a backup")
        .to_owned();
    assert_eq!(backups["items"][0]["present"], json!(true));
    assert_eq!(backups["items"][0]["scheduled"], json!(false));

    // The dry run refuses while the database runs, and says why.
    let restore = format!("{VOLUME}/backups/{backup}/restore");
    let (st, plan) = l.req("GET", &restore, o, None).await;
    assert_eq!(st, StatusCode::OK, "{plan}");
    assert_eq!(plan["plan"]["running"], json!(["shop-db-01"]));
    assert!(
        plan["plan"]["blocked"][0]
            .as_str()
            .unwrap_or_default()
            .starts_with("stop shop-db-01 first")
    );
    let (st, e) = l
        .req(
            "POST",
            &restore,
            o,
            Some(json!({ "confirm": "shop-db-01-data" })),
        )
        .await;
    assert_eq!(
        (st, e["error"]["code"].as_str()),
        (StatusCode::CONFLICT, Some("restore_blocked")),
        "{e}"
    );

    // Stopped: the dry run passes, lists the snapshots it deletes and the safety backup.
    mock.storage()
        .snapshots
        .entry("acme-prod-live/mock-local/mock-member-01/shop-db-01-data".into())
        .or_default()
        .push(apelora_incus_adapter::SnapshotInfo {
            name: "snap-1".into(),
            created_at: "x".into(),
        });
    let (st, op) = l
        .req(
            "POST",
            "/api/v1/applications/shop/operations",
            o,
            Some(json!({ "action": "stop", "confirmDisruption": true })),
        )
        .await;
    assert_eq!(st, StatusCode::ACCEPTED, "{op}");
    settle(&l).await;
    let (_, plan) = l.req("GET", &restore, o, None).await;
    assert_eq!(plan["plan"]["blocked"], json!([]), "{plan}");
    assert_eq!(plan["plan"]["snapshotsDeleted"], json!(["snap-1"]));
    assert_eq!(plan["plan"]["safetyBackupPool"], json!("mock-backup"));

    // The volume's name must be typed.
    let (st, e) = l
        .req(
            "POST",
            &restore,
            o,
            Some(json!({ "confirm": "shop-db-01" })),
        )
        .await;
    assert_eq!(
        (st, e["error"]["code"].as_str()),
        (StatusCode::BAD_REQUEST, Some("confirmation_mismatch"))
    );

    tokio::time::sleep(std::time::Duration::from_millis(1100)).await; // backup names carry the second
    let (st, job) = l
        .req(
            "POST",
            &restore,
            o,
            Some(json!({ "confirm": "shop-db-01-data" })),
        )
        .await;
    assert_eq!(st, StatusCode::ACCEPTED, "{job}");
    let audited: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE action = 'application.volume.restore.requested' AND detail->>'jobId' = $1",
    )
    .bind(job["jobId"].as_str())
    .fetch_one(&l.pool)
    .await
    .expect("audit");
    assert_eq!(audited, 1, "queued together with its audit row");
    settle(&l).await;
    let (status, error) = job_status(&l, &job["jobId"]).await;
    assert_eq!(status, "succeeded", "{error:?}");
    let copies = mock.storage().copies.clone();
    assert!(
        copies
            .last()
            .is_some_and(|c| c.starts_with(&backup) && c.ends_with("(refresh)")),
        "{copies:?}"
    );
    let (_, backups) = l.req("GET", &format!("{VOLUME}/backups"), o, None).await;
    let safety = backups["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|b| !b["beforeRestore"].is_null())
        .expect("a safety backup");
    assert_eq!(safety["beforeRestore"], job["jobId"]);

    // A failed restore is not retried as is: the dry run is read and the name typed again.
    let id = job["jobId"].as_str().unwrap_or_default();
    sqlx::query("UPDATE jobs SET status = 'failed' WHERE id = $1::uuid")
        .bind(id)
        .execute(&l.pool)
        .await
        .expect("fail it");
    let (_, detail) = l
        .req("GET", &format!("/api/v1/jobs/{id}/detail"), o, None)
        .await;
    assert_eq!(
        detail["job"]["actions"]["retry"]["allowed"],
        json!(false),
        "{detail}"
    );
    assert!(
        detail["job"]["actions"]["retry"]["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("volume's name typed"),
        "{detail}"
    );
    let (st, e) = l
        .req("POST", &format!("/api/v1/jobs/{id}/retry"), o, None)
        .await;
    assert_eq!(
        (st, e["error"]["code"].as_str()),
        (StatusCode::CONFLICT, Some("not_retryable")),
        "{e}"
    );

    // Deleting a backup removes that copy only. The name must be typed; the volume stays.
    let safety_name = safety["name"].as_str().expect("safety backup").to_owned();
    let del = format!("{VOLUME}/backups/{backup}");
    let (st, e) = l
        .req("DELETE", &del, o, Some(json!({ "confirm": "nope" })))
        .await;
    assert_eq!(
        (st, e["error"]["code"].as_str()),
        (StatusCode::BAD_REQUEST, Some("confirmation_mismatch")),
        "{e}"
    );
    let (st, _) = l
        .req(
            "DELETE",
            &format!("{VOLUME}/backups/missing-bk"),
            o,
            Some(json!({ "confirm": "missing-bk" })),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = l
        .req("DELETE", &del, o, Some(json!({ "confirm": backup })))
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let audited: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE action = 'application.volume.backup.delete' AND detail->>'backup' = $1",
    )
    .bind(&backup)
    .fetch_one(&l.pool)
    .await
    .expect("audit");
    assert_eq!(audited, 1);
    let (_, left) = l.req("GET", &format!("{VOLUME}/backups"), o, None).await;
    let names: Vec<_> = left["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter_map(|b| b["name"].as_str())
        .collect();
    assert!(!names.contains(&backup.as_str()), "{names:?}");
    assert!(names.contains(&safety_name.as_str()), "{names:?}");
    let (st, vols) = l.req("GET", "/api/v1/storage/volumes", o, None).await;
    assert_eq!(st, StatusCode::OK, "{vols}");
    assert!(
        vols["items"]
            .as_array()
            .expect("volumes")
            .iter()
            .any(|v| v["incusName"] == "shop-db-01-data"),
        "{vols}"
    );

    // A record whose copy is already gone is still forgotten.
    let keys: Vec<String> = mock
        .storage()
        .volumes
        .keys()
        .filter(|k| k.ends_with(safety_name.as_str()))
        .cloned()
        .collect();
    assert_eq!(keys.len(), 1, "{keys:?}");
    for k in keys {
        mock.storage().volumes.remove(&k);
    }
    let (st, _) = l
        .req(
            "DELETE",
            &format!("{VOLUME}/backups/{safety_name}"),
            o,
            Some(json!({ "confirm": safety_name })),
        )
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (_, left) = l.req("GET", &format!("{VOLUME}/backups"), o, None).await;
    assert!(
        left["items"]
            .as_array()
            .expect("items")
            .iter()
            .all(|b| b["name"] != safety_name),
        "{left}"
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn schedules_are_set_validated_and_need_a_backup_pool_for_backups(pool: Pool) {
    let (mut l, _, s) = deployed_shop(pool).await;
    let o = &s.owner;
    let path = format!("{VOLUME}/schedules");
    let (st, e) = l
        .req(
            "PUT",
            &path,
            o,
            Some(json!({ "snapshots": { "everyMinutes": 5, "keep": 3 }, "backups": null })),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{e}");
    let (st, e) = l.req("PUT", &path, o, Some(json!({ "snapshots": { "everyMinutes": 60, "keep": 3 }, "backups": { "everyMinutes": 1440, "keep": 7 } }))).await;
    assert_eq!(st, StatusCode::NO_CONTENT, "{e}");
    let (_, list) = l.req("GET", "/api/v1/storage/volumes", o, None).await;
    assert_eq!(
        list["items"][0]["schedules"]["snapshots"]["everyMinutes"],
        json!(60)
    );
    assert_eq!(list["items"][0]["schedules"]["backups"]["keep"], json!(7));
    let (_, backups) = l.req("GET", &format!("{VOLUME}/backups"), o, None).await;
    assert_eq!(backups["schedules"]["backups"]["everyMinutes"], json!(1440));

    l.state = l.state.clone().with_backup_pool(None);
    let (st, e) = l
        .req(
            "PUT",
            &path,
            o,
            Some(json!({ "snapshots": null, "backups": { "everyMinutes": 1440, "keep": 7 } })),
        )
        .await;
    assert_eq!(
        (st, e["error"]["code"].as_str()),
        (StatusCode::CONFLICT, Some("backup_pool_missing"))
    );
    let (st, e) = l.req("POST", &format!("{VOLUME}/backups"), o, None).await;
    assert_eq!(
        (st, e["error"]["code"].as_str()),
        (StatusCode::CONFLICT, Some("backup_pool_missing"))
    );
    // Turning both off needs no pool.
    let (st, _) = l
        .req(
            "PUT",
            &path,
            o,
            Some(json!({ "snapshots": null, "backups": null })),
        )
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn storage_routes_check_the_exact_application_and_infrastructure_needs_cluster_view(
    pool: Pool,
) {
    let (l, mock, s) = deployed_shop(pool).await;
    // Someone else's tenant: an owner of the other organisation.
    let bob = l.user("sub-bob", "bob").await;
    l.grant(
        &s.w.admin,
        bob,
        "organization-owner",
        json!({ "type": "organization", "id": s.w.other }),
    )
    .await;
    let other = l.token("sub-bob", "bob");
    let (st, list) = l.req("GET", "/api/v1/storage/volumes", &other, None).await;
    assert_eq!(
        (st, list["items"].as_array().map(Vec::len)),
        (StatusCode::OK, Some(0)),
        "{list}"
    );
    let (st, _) = l
        .req("GET", &format!("{VOLUME}/backups"), &other, None)
        .await;
    assert_eq!(
        st,
        StatusCode::NOT_FOUND,
        "another tenant's volume does not exist for them"
    );
    let (st, _) = l
        .req("POST", &format!("{VOLUME}/backups"), &other, None)
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, n) = l
        .req("GET", "/api/v1/networks/applications", &other, None)
        .await;
    assert_eq!(
        (st, n["items"].as_array().map(Vec::len)),
        (StatusCode::OK, Some(0)),
        "{n}"
    );

    // A viewer of the organisation reads but may not back up, restore or change schedules.
    let vic = l.user("sub-vic", "vic").await;
    l.grant(
        &s.w.admin,
        vic,
        "viewer",
        json!({ "type": "organization", "id": s.w.acme }),
    )
    .await;
    let viewer = l.token("sub-vic", "vic");
    let (st, _) = l
        .req("GET", &format!("{VOLUME}/backups"), &viewer, None)
        .await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = l
        .req("POST", &format!("{VOLUME}/backups"), &viewer, None)
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = l
        .req(
            "PUT",
            &format!("{VOLUME}/schedules"),
            &viewer,
            Some(json!({ "snapshots": null, "backups": null })),
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = l
        .req(
            "GET",
            &format!("{VOLUME}/backups/x-bk-1/restore"),
            &viewer,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = l
        .req(
            "DELETE",
            &format!("{VOLUME}/backups/x"),
            &viewer,
            Some(json!({ "confirm": "x" })),
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = super::tests::send(
        l.state.clone(),
        "DELETE",
        &format!("{VOLUME}/backups/x"),
        "",
        Some(json!({ "confirm": "x" })),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    // Pools, orphans and network settings are infrastructure: an organisation owner may not.
    for path in [
        "/api/v1/storage/pools",
        "/api/v1/storage/orphans",
        "/api/v1/networks/settings",
    ] {
        let (st, _) = l.req("GET", path, &s.owner, None).await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{path}");
        let (st, _) = super::tests::send(l.state.clone(), "GET", path, "", None, None).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "{path}");
    }

    // A custom volume nobody records is an orphan; recorded volumes are not.
    mock.storage().volumes.insert(
        "acme-prod-live/mock-local/mock-member-02/left-behind".into(),
        apelora_incus_adapter::VolumeState {
            managed: true,
            application: Some("gone".into()),
            content: "filesystem".into(),
            size_mib: Some(2048),
            location: "mock-member-02".into(),
            used_by: vec![],
        },
    );
    let admin = &s.w.admin;
    let (st, o) = l.req("GET", "/api/v1/storage/orphans", admin, None).await;
    assert_eq!(st, StatusCode::OK, "{o}");
    let names: Vec<&str> = o["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter_map(|v| v["name"].as_str())
        .collect();
    assert_eq!(names, vec!["left-behind"], "{o}");
    assert_eq!(o["items"][0]["sizeMib"], json!(2048));
    assert!(
        o["items"][0]["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("gone")
    );
    assert!(
        o["projects"]
            .as_array()
            .expect("projects")
            .contains(&json!("acme-prod-live"))
    );

    let (st, p) = l.req("GET", "/api/v1/storage/pools", admin, None).await;
    assert_eq!(st, StatusCode::OK, "{p}");
    let local = p["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|i| i["name"] == "mock-local")
        .expect("pool");
    assert_eq!(local["role"], json!("volumes"));
    assert!(
        local["total"]["totalBytes"].as_u64().unwrap_or(0) > 0,
        "{local}"
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn application_networks_show_the_compiled_rules_next_to_incus_and_flag_drift(pool: Pool) {
    let (l, mock, s) = deployed_shop(pool).await;
    let (st, n) = l
        .req("GET", "/api/v1/networks/applications", &s.owner, None)
        .await;
    assert_eq!(st, StatusCode::OK, "{n}");
    let app = &n["items"][0];
    assert_eq!(
        (
            app["application"].as_str(),
            app["enforced"].as_bool(),
            app["network"].as_str()
        ),
        (Some("shop"), Some(true), Some("shop")),
        "{app}"
    );
    assert_eq!(
        app["connections"][0],
        json!({ "from": "web-01", "to": "db-01", "transport": "tcp", "port": 5432 })
    );
    assert_eq!(app["drift"], json!([]), "{app}");
    let db = app["modules"]
        .as_array()
        .expect("modules")
        .iter()
        .find(|m| m["module"] == "db-01")
        .expect("db");
    assert_eq!(db["ingress"][0]["source"], json!("$shop-web-01"));
    assert_eq!(db["ingress"][0]["port"], json!("5432"));
    assert_eq!(db["ingress"][0]["state"], json!("applied"));
    assert_eq!(db["drift"], json!([]));
    let web = app["modules"]
        .as_array()
        .expect("modules")
        .iter()
        .find(|m| m["module"] == "web-01")
        .expect("web");
    assert_eq!(web["internet"], json!("web"));
    let svc = app["services"]
        .as_array()
        .expect("services")
        .iter()
        .find(|x| x["module"] == "web-01")
        .expect("web service");
    assert_eq!(svc["published"], json!(true));
    assert!(
        svc["address"]
            .as_str()
            .is_some_and(|a| a.starts_with("198.18.100.")),
        "{svc}"
    );
    assert_eq!(svc["drift"], json!([]), "{svc}");

    // Someone changes the database's rules in Incus: the page says what differs.
    mock.net().acls.insert(
        "acme-prod-live/shop-db-01".into(),
        (
            vec![apelora_domain::network::AclRule {
                action: "allow".into(),
                source: String::new(),
                destination: String::new(),
                protocol: "tcp".into(),
                destination_port: "22".into(),
                state: "enabled".into(),
                description: "added by hand".into(),
            }],
            vec![],
        ),
    );
    let (_, n) = l
        .req("GET", "/api/v1/networks/applications", &s.owner, None)
        .await;
    let db = n["items"][0]["modules"]
        .as_array()
        .expect("modules")
        .iter()
        .find(|m| m["module"] == "db-01")
        .expect("db")
        .clone();
    assert_eq!(db["ingress"][0]["state"], json!("missing"));
    assert_eq!(db["ingress"][1]["state"], json!("unexpected"));
    assert_eq!(db["ingress"][1]["port"], json!("22"));
    assert_eq!(db["drift"].as_array().map(Vec::len), Some(2), "{db}");

    // Settings in effect for an administrator, with the deploy projects' limits.
    let (st, set) = l
        .req("GET", "/api/v1/networks/settings", &s.w.admin, None)
        .await;
    assert_eq!(st, StatusCode::OK, "{set}");
    assert_eq!(
        (set["uplink"].as_str(), set["serviceRange"].as_str()),
        (Some("incusbr0"), Some("198.18.100.0/28"))
    );
    assert_eq!(set["serviceAddressesTotal"], json!(14));
    assert!(set["serviceAddressesUsed"].as_i64().unwrap_or(0) >= 1);
}

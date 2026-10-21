// SPDX-License-Identifier: AGPL-3.0-only
//! Alerts through the API (docs/design/alerts.md): what each permission opens, acknowledgement,
//! silences, rule parameters and webhook channels, with alerts made by the worker's real
//! evaluator against PostgreSQL and the mock Incus. Audit rows for every change are checked in
//! `audit_coverage_tests.rs`.

use apelora_auth::{Builtin, Scope};
use apelora_persistence::identity as idb;
use serde_json::{Value, json};

use super::identity_tests::{Lab, code, lab, new_assignment};
use super::*;

async fn person(l: &Lab, sub: &str, role: Option<Builtin>) -> String {
    let id = l.user(sub, sub).await;
    if let Some(role) = role {
        idb::insert_assignment(&l.pool, &new_assignment(id, role, Scope::Platform))
            .await
            .expect("assign");
    }
    l.token(sub, sub)
}

/// One failed job, then one evaluator pass: a firing `job-failures` alert.
pub(super) async fn fire_an_alert(pool: &Pool) {
    let id = apelora_persistence::enqueue_job(pool, "network.sync", None, &json!({}), None)
        .await
        .expect("job")
        .id();
    sqlx::query("UPDATE jobs SET status = 'failed', last_error = 'no uplink' WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .expect("fail");
    evaluate(pool).await;
}

async fn evaluate(pool: &Pool) {
    let cfg = apelora_deployment_worker::alerts::Config {
        every: std::time::Duration::from_secs(30),
        restore_drill_record: None,
    };
    let provider = Provider::Mock(apelora_incus_adapter::MockIncus::default());
    apelora_deployment_worker::alerts::evaluate_once(pool, &provider, &cfg, None, "worker-t")
        .await
        .expect("evaluate");
}

fn channel_body(name: &str) -> Value {
    json!({ "name": name, "url": "https://hooks.example/apelora?token=abc", "secretRef": "APELORA_ALERT_SECRET_OPS", "minSeverity": "warning" })
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn people_see_acknowledge_and_silence_alerts_by_permission(pool: Pool) {
    let l = lab(pool);
    let viewer = person(&l, "viewer", Some(Builtin::Viewer)).await;
    let admin = person(&l, "fabric", Some(Builtin::FabricAdministrator)).await;
    let nobody = person(&l, "designer", None).await;

    // Before any worker evaluates: nothing, and the evaluator is reported as not running.
    let (s, v) = l.req("GET", "/api/v1/alerts", &viewer, None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["alerts"], json!([]));
    assert_eq!(v["evaluator"]["stale"], true);
    assert_eq!(v["actions"]["respond"]["allowed"], false);
    let (s, v) = l.req("GET", "/api/v1/alerts", &nobody, None).await;
    assert_eq!((s, code(&v)), (StatusCode::FORBIDDEN, Some("forbidden")));

    fire_an_alert(&l.pool).await;
    let (_, v) = l.req("GET", "/api/v1/alerts", &viewer, None).await;
    assert_eq!(v["evaluator"]["stale"], false);
    assert_eq!(v["evaluator"]["worker"], "worker-t");
    let alert = &v["alerts"][0];
    assert_eq!(
        (
            alert["rule"].as_str(),
            alert["subject"].as_str(),
            alert["state"].as_str()
        ),
        (Some("job-failures"), Some("network.sync"), Some("firing"))
    );
    assert_eq!(alert["title"], "Jobs are failing");
    assert_eq!(alert["runbook"], "alerts.md#jobs-are-failing");
    assert_eq!(v["counts"]["warning"], 1);
    assert_eq!(v["counts"]["needsAttention"], 1);
    let id = alert["id"].as_i64().expect("id");

    // A viewer may not acknowledge; an administrator may, once, with a note.
    let ack = format!("/api/v1/alerts/{id}/acknowledge");
    let (s, _) = l.req("POST", &ack, &viewer, Some(json!({}))).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, v) = l
        .req(
            "POST",
            &ack,
            &admin,
            Some(json!({ "note": "looking at the uplink" })),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["acknowledged"]["byName"], "fabric");
    assert_eq!(v["acknowledged"]["note"], "looking at the uplink");
    let (s, v) = l.req("POST", &ack, &admin, Some(json!({}))).await;
    assert_eq!(
        (s, code(&v)),
        (StatusCode::CONFLICT, Some("already_acknowledged"))
    );
    let (s, v) = l
        .req(
            "POST",
            "/api/v1/alerts/999999/acknowledge",
            &admin,
            Some(json!({})),
        )
        .await;
    assert_eq!((s, code(&v)), (StatusCode::NOT_FOUND, Some("not_found")));
    let (s, _) = l
        .req(
            "POST",
            &ack,
            &admin,
            Some(json!({ "note": "x".repeat(501) })),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (_, v) = l.req("GET", "/api/v1/alerts", &viewer, None).await;
    assert_eq!(v["counts"]["needsAttention"], 0, "acknowledged");

    // Silences: checked, listed with the alert they cover, ended once.
    for bad in [
        json!({ "rule": "no-such-rule", "minutes": 30 }),
        json!({ "rule": "job-failures", "minutes": 0 }),
        json!({ "rule": "job-failures", "minutes": 20000 }),
    ] {
        let (s, v) = l
            .req("POST", "/api/v1/alerts/silences", &admin, Some(bad.clone()))
            .await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{bad} {v}");
    }
    let (s, _) = l
        .req(
            "POST",
            "/api/v1/alerts/silences",
            &viewer,
            Some(json!({ "rule": "job-failures", "minutes": 30 })),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, v) = l
        .req("POST", "/api/v1/alerts/silences", &admin, Some(json!({ "rule": "job-failures", "subject": "network.sync", "minutes": 30, "note": "uplink work" })))
        .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let silence = v["id"].as_str().expect("id").to_owned();
    let (_, v) = l.req("GET", "/api/v1/alerts", &viewer, None).await;
    assert_eq!(v["silences"][0]["ruleTitle"], "Jobs are failing");
    assert_eq!(v["alerts"][0]["silencedBy"]["id"], json!(silence));
    let end = format!("/api/v1/alerts/silences/{silence}");
    assert_eq!(
        l.req("DELETE", &end, &admin, None).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        l.req("DELETE", &end, &admin, None).await.0,
        StatusCode::NOT_FOUND
    );

    // History: once the failure ages out, the alert resolves and appears there.
    sqlx::query("UPDATE jobs SET updated_at = now() - interval '1 hour'")
        .execute(&l.pool)
        .await
        .expect("age");
    evaluate(&l.pool).await;
    let (_, v) = l.req("GET", "/api/v1/alerts", &viewer, None).await;
    assert_eq!(v["alerts"], json!([]));
    let (s, v) = l
        .req("GET", "/api/v1/alerts/history?limit=10", &viewer, None)
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["items"][0]["id"], json!(id));
    assert_eq!(v["items"][0]["state"], "resolved");
    assert_eq!(v["items"][0]["acknowledged"]["byName"], "fabric");
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn rules_are_tuned_within_bounds_by_those_who_may_configure_them(pool: Pool) {
    let l = lab(pool);
    let viewer = person(&l, "viewer", Some(Builtin::Viewer)).await;
    let admin = person(&l, "fabric", Some(Builtin::FabricAdministrator)).await;
    let (s, v) = l.req("GET", "/api/v1/alerts/rules", &viewer, None).await;
    assert_eq!(s, StatusCode::OK);
    let rules = v["rules"].as_array().expect("rules");
    assert_eq!(rules.len(), apelora_domain::alerts::RULES.len());
    let wait = rules
        .iter()
        .find(|r| r["id"] == "job-queue-wait")
        .expect("rule");
    assert_eq!(wait["params"], wait["defaults"]);
    assert_eq!(wait["sourceState"], "unknown", "no pass yet");
    assert_eq!(v["actions"]["configure"]["allowed"], false);

    let path = "/api/v1/alerts/rules/job-queue-wait";
    let good = json!({ "enabled": true, "threshold": 900, "forSecs": 120, "severity": "critical" });
    assert_eq!(
        l.req("PUT", path, &viewer, Some(good.clone())).await.0,
        StatusCode::FORBIDDEN
    );
    let (s, v) = l
        .req(
            "PUT",
            path,
            &admin,
            Some(
                json!({ "enabled": true, "threshold": 1, "forSecs": 120, "severity": "critical" }),
            ),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "below the rule's minimum: {v}");
    let (s, _) = l
        .req(
            "PUT",
            "/api/v1/alerts/rules/member-offline",
            &admin,
            Some(json!({ "enabled": true, "threshold": 3, "forSecs": 60, "severity": "critical" })),
        )
        .await;
    assert_eq!(
        s,
        StatusCode::BAD_REQUEST,
        "a state rule has no threshold to change"
    );
    assert_eq!(
        l.req(
            "PUT",
            "/api/v1/alerts/rules/nope",
            &admin,
            Some(good.clone())
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let (s, v) = l.req("PUT", path, &admin, Some(good)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["params"]["threshold"], 900.0);
    assert_eq!(v["params"]["severity"], "critical");
    assert_eq!(v["defaults"]["threshold"], 300.0);
    assert!(v["updatedBy"].as_str().expect("by").starts_with("user:"));
    evaluate(&l.pool).await;
    let (_, v) = l.req("GET", "/api/v1/alerts/rules", &viewer, None).await;
    let cert = v["rules"]
        .as_array()
        .expect("rules")
        .iter()
        .find(|r| r["id"] == "certificate-expiry")
        .cloned()
        .expect("rule");
    assert_eq!(
        cert["sourceState"], "not-configured",
        "the mock has no certificates"
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn channels_hold_a_secret_reference_and_https_urls_only(pool: Pool) {
    let l = lab(pool);
    let viewer = person(&l, "viewer", Some(Builtin::Viewer)).await;
    let admin = person(&l, "fabric", Some(Builtin::FabricAdministrator)).await;
    let path = "/api/v1/alerts/channels";
    assert_eq!(
        l.req("POST", path, &viewer, Some(channel_body("ops")))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    for (field, value) in [
        ("url", "http://hooks.example/apelora"),
        ("url", "https://user:pass@hooks.example/"),
        ("secretRef", "DATABASE_URL"),
        ("secretRef", "s3cr3t-value"),
        ("name", ""),
    ] {
        let mut body = channel_body("ops");
        body[field] = json!(value);
        let (s, v) = l.req("POST", path, &admin, Some(body)).await;
        assert_eq!(
            (s, code(&v)),
            (StatusCode::BAD_REQUEST, Some("invalid_request")),
            "{field}={value}: {v}"
        );
    }
    let mut extra = channel_body("ops");
    extra["secret"] = json!("value");
    assert_eq!(
        l.req("POST", path, &admin, Some(extra)).await.0,
        StatusCode::BAD_REQUEST,
        "a secret value is not a field"
    );
    let (s, v) = l.req("POST", path, &admin, Some(channel_body("ops"))).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let id = v["id"].as_str().expect("id").to_owned();
    assert_eq!(
        l.req("POST", path, &admin, Some(channel_body("ops")))
            .await
            .0,
        StatusCode::CONFLICT
    );

    // Viewers see the host, not the URL (it may carry a receiver's token).
    let (_, v) = l.req("GET", path, &viewer, None).await;
    assert_eq!(v["channels"][0]["host"], "hooks.example");
    assert_eq!(v["channels"][0]["url"], Value::Null);
    assert_eq!(v["channels"][0]["secretRef"], "APELORA_ALERT_SECRET_OPS");
    assert_eq!(v["secretPrefix"], "APELORA_ALERT_SECRET_");
    let (_, v) = l.req("GET", path, &admin, None).await;
    assert_eq!(
        v["channels"][0]["url"],
        "https://hooks.example/apelora?token=abc"
    );

    // A test goes through the outbox like a real notification.
    let (s, v) = l
        .req("POST", &format!("{path}/{id}/test"), &admin, None)
        .await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    let message = v["messageId"].as_i64().expect("message");
    let (_, v) = l.req("GET", path, &admin, None).await;
    assert_eq!(v["channels"][0]["delivery"]["pending"], 1);
    let payload: Value = sqlx::query_scalar("SELECT payload FROM outbox WHERE id = $1")
        .bind(message)
        .fetch_one(&l.pool)
        .await
        .expect("message");
    assert_eq!(
        (payload["event"].as_str(), payload["channel"].as_str()),
        (Some("test"), Some(id.as_str()))
    );

    // Turned off: no test; deleted: gone. The audit rows name the host, never the URL.
    let mut off = channel_body("ops");
    off["enabled"] = json!(false);
    let (s, v) = l
        .req("PUT", &format!("{path}/{id}"), &admin, Some(off))
        .await;
    assert_eq!((s, v["enabled"].as_bool()), (StatusCode::OK, Some(false)));
    let (s, v) = l
        .req("POST", &format!("{path}/{id}/test"), &admin, None)
        .await;
    assert_eq!(
        (s, code(&v)),
        (StatusCode::CONFLICT, Some("channel_disabled"))
    );
    assert_eq!(
        l.req("DELETE", &format!("{path}/{id}"), &admin, None)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        l.req("DELETE", &format!("{path}/{id}"), &admin, None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let details: Vec<String> = sqlx::query_scalar(
        "SELECT detail::text FROM audit_events WHERE action LIKE 'alert.channel.%'",
    )
    .fetch_all(&l.pool)
    .await
    .expect("audit");
    assert_eq!(details.len(), 4, "create, update, test, delete");
    assert!(
        details.iter().all(|d| !d.contains("token=abc")),
        "{details:?}"
    );
}

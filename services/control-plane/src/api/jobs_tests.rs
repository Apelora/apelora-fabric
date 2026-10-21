// Job control and the instance page through the real router, against PostgreSQL and the mock
// Incus: retry and cancel follow the permission that queued the job, are audited and refuse
// with reasons; a running job stops at a safe point; the instance page and its logs are shown
// by permission. Evidence: MOCK VERIFIED with a real PostgreSQL.

use serde_json::{Value, json};
use uuid::Uuid;

use super::identity_tests::{Lab, World, code, lab, world};
use super::manage_tests::settle;
use super::*;

fn spec(name: &str) -> Value {
    json!({ "project": "acme-prod-live", "name": name, "kind": "container", "start": true,
            "source": { "protocol": "oci", "server": "https://registry.example", "alias": "nginx:1.29" } })
}

struct People {
    w: World,
    alice: String,
    alice_id: Uuid,
    bob: String,
    viewer: String,
}

/// Alice owns organisation acme (its production environment maps to `acme-prod-live`), Bob owns
/// the other organisation, and a platform viewer sees everything and may change nothing.
async fn people(l: &Lab) -> People {
    let w = world(l).await;
    let alice_id = l.user("sub-alice", "alice").await;
    let bob_id = l.user("sub-bob", "bob").await;
    let viewer_id = l.user("sub-vera", "vera").await;
    for (who, role, scope) in [
        (
            alice_id,
            "organization-owner",
            json!({ "type": "organization", "id": w.acme }),
        ),
        (
            bob_id,
            "organization-owner",
            json!({ "type": "organization", "id": w.other }),
        ),
        (viewer_id, "viewer", json!({ "type": "platform" })),
    ] {
        let (s, b) = l.grant(&w.admin, who, role, scope).await;
        assert_eq!(s, StatusCode::CREATED, "{b}");
    }
    People {
        alice: l.token_mfa("sub-alice", "alice"),
        alice_id,
        bob: l.token_mfa("sub-bob", "bob"),
        viewer: l.token("sub-vera", "vera"),
        w,
    }
}

async fn queue(l: &Lab, token: &str, method: &str, path: &str, body: Option<Value>) -> Uuid {
    let (s, b) = l.req(method, path, token, body).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{b}");
    b["jobId"].as_str().expect("job").parse().expect("uuid")
}

async fn fail(l: &Lab, id: Uuid) {
    sqlx::query("UPDATE jobs SET status = 'failed', attempts = max_attempts, last_error = 'Incus said no' WHERE id = $1")
        .bind(id)
        .execute(&l.pool)
        .await
        .expect("fail");
}

async fn audited(l: &Lab, action: &str, outcome: &str) -> Vec<Value> {
    sqlx::query_scalar::<_, Value>(
        "SELECT detail FROM audit_events WHERE action = $1 AND outcome = $2 ORDER BY id",
    )
    .bind(action)
    .bind(outcome)
    .fetch_all(&l.pool)
    .await
    .expect("audit")
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn a_pending_job_is_cancelled_by_who_may_queue_it_and_nobody_else(pool: Pool) {
    let l = lab(pool);
    let p = people(&l).await;
    let job = queue(
        &l,
        &p.alice,
        "POST",
        "/api/v1/instances",
        Some(spec("web-01")),
    )
    .await;

    let (s, b) = l
        .req("GET", &format!("/api/v1/jobs/{job}"), &p.alice, None)
        .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["queuedBy"], json!(format!("user:{}", p.alice_id)));
    assert_eq!(b["actions"]["cancel"]["allowed"], json!(true));
    assert_eq!(b["actions"]["retry"]["allowed"], json!(false));
    assert!(
        b["actions"]["retry"]["reason"]
            .as_str()
            .is_some_and(|r| r.contains("Only failed"))
    );

    // The viewer sees it, may do nothing with it, and is told why.
    let (s, b) = l
        .req("GET", &format!("/api/v1/jobs/{job}"), &p.viewer, None)
        .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["actions"]["cancel"]["allowed"], json!(false));
    assert!(
        b["actions"]["cancel"]["reason"]
            .as_str()
            .is_some_and(|r| r.contains("instance.manage"))
    );
    let (s, _) = l
        .req(
            "POST",
            &format!("/api/v1/jobs/{job}/cancel"),
            &p.viewer,
            None,
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // Bob cannot even see it: the same answer as for a job that does not exist.
    for path in [
        format!("/api/v1/jobs/{job}/cancel"),
        format!("/api/v1/jobs/{job}/retry"),
    ] {
        let (s, b) = l.req("POST", &path, &p.bob, None).await;
        assert_eq!(
            (s, code(&b)),
            (StatusCode::NOT_FOUND, Some("not_found")),
            "{path}"
        );
    }
    let (s, _) = l
        .req("GET", &format!("/api/v1/jobs/{job}/detail"), &p.bob, None)
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    let (s, b) = l
        .req(
            "POST",
            &format!("/api/v1/jobs/{job}/cancel"),
            &p.alice,
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(
        (b["outcome"].as_str(), b["job"]["status"].as_str()),
        (Some("cancelled"), Some("cancelled"))
    );
    assert_eq!(
        b["job"]["cancelRequestedBy"],
        json!(format!("user:{}", p.alice_id))
    );
    // Nothing runs it.
    settle(&l).await;
    assert_eq!(
        l.state.provider.instance("acme-prod-live", "web-01").await,
        Ok(None)
    );
    let (s, b) = l
        .req(
            "POST",
            &format!("/api/v1/jobs/{job}/cancel"),
            &p.alice,
            None,
        )
        .await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::CONFLICT, Some("job_finished")),
        "{b}"
    );

    let ok = audited(&l, "job.cancel", "success").await;
    assert_eq!(ok.len(), 1);
    assert_eq!(ok[0]["jobId"], json!(job));
    assert_eq!(ok[0]["outcome"], json!("cancelled"));
    assert!(
        ok[0]["requestId"].is_string(),
        "the request is named: {}",
        ok[0]
    );
    assert_eq!(audited(&l, "job.cancel", "denied").await.len(), 1);
    assert!(
        !audited(&l, "access.denied", "denied").await.is_empty(),
        "the viewer's 403 is audited"
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn a_failed_job_is_retried_as_a_new_linked_job_once_and_never_over_a_newer_request(
    pool: Pool,
) {
    let l = lab(pool);
    let p = people(&l).await;
    queue(
        &l,
        &p.alice,
        "POST",
        "/api/v1/instances",
        Some(spec("web-01")),
    )
    .await;
    settle(&l).await;
    let stop = queue(
        &l,
        &p.alice,
        "POST",
        "/api/v1/instances/acme-prod-live/web-01/state",
        Some(json!({ "action": "stop" })),
    )
    .await;
    fail(&l, stop).await;

    let (s, b) = l
        .req("GET", &format!("/api/v1/jobs/{stop}"), &p.alice, None)
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["actions"]["retry"]["allowed"], json!(true), "{b}");
    assert_eq!(b["actions"]["cancel"]["allowed"], json!(false));

    let retry = queue(
        &l,
        &p.alice,
        "POST",
        &format!("/api/v1/jobs/{stop}/retry"),
        None,
    )
    .await;
    assert_ne!(retry, stop);
    let (_, b) = l
        .req("GET", &format!("/api/v1/jobs/{retry}"), &p.alice, None)
        .await;
    assert_eq!(
        (
            b["retryOf"].as_str(),
            b["kind"].as_str(),
            b["status"].as_str(),
            b["attempts"].as_i64()
        ),
        (
            Some(stop.to_string().as_str()),
            Some("instance.stop"),
            Some("pending"),
            Some(0)
        )
    );
    let (_, old) = l
        .req("GET", &format!("/api/v1/jobs/{stop}"), &p.alice, None)
        .await;
    assert_eq!(
        (
            old["status"].as_str(),
            old["lastError"].as_str(),
            old["retriedBy"].as_str()
        ),
        (
            Some("failed"),
            Some("Incus said no"),
            Some(retry.to_string().as_str())
        ),
        "the failed job stays as it was"
    );
    let (s, b) = l
        .req(
            "POST",
            &format!("/api/v1/jobs/{stop}/retry"),
            &p.alice,
            None,
        )
        .await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::CONFLICT, Some("already_retried")),
        "{b}"
    );

    // The retry runs and stops the instance; its detail links back.
    settle(&l).await;
    let (s, d) = l
        .req(
            "GET",
            &format!("/api/v1/jobs/{retry}/detail"),
            &p.alice,
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{d}");
    assert_eq!(d["job"]["status"], json!("succeeded"));
    assert_eq!(d["target"], json!("acme-prod-live/web-01"));
    assert_eq!(d["attempts"][0]["outcome"], json!("succeeded"));
    assert_eq!(
        d["attempts"][0]["incusOperations"].as_array().map(Vec::len),
        Some(1)
    );
    assert!(d["startedAt"].is_string() && d["finishedAt"].is_string());
    assert!(
        d["safePoints"]
            .as_str()
            .is_some_and(|s| s.contains("Before each change"))
    );
    assert!(
        d["linked"]
            .as_array()
            .expect("linked")
            .iter()
            .any(|j| j["id"] == json!(stop) && j["relation"] == json!("retry-of"))
    );
    assert_eq!(
        l.state
            .provider
            .instance("acme-prod-live", "web-01")
            .await
            .map(|i| i.map(|i| i.status)),
        Ok(Some("Stopped".into()))
    );

    // A failed start, then a newer stop for the same instance: retrying the start would undo it.
    let start = queue(
        &l,
        &p.alice,
        "POST",
        "/api/v1/instances/acme-prod-live/web-01/state",
        Some(json!({ "action": "start" })),
    )
    .await;
    fail(&l, start).await;
    queue(
        &l,
        &p.alice,
        "POST",
        "/api/v1/instances/acme-prod-live/web-01/state",
        Some(json!({ "action": "stop" })),
    )
    .await;
    let (s, b) = l
        .req(
            "POST",
            &format!("/api/v1/jobs/{start}/retry"),
            &p.alice,
            None,
        )
        .await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::CONFLICT, Some("superseded")),
        "{b}"
    );

    let ok = audited(&l, "job.retry", "success").await;
    assert_eq!(ok.len(), 1);
    assert_eq!(
        (ok[0]["jobId"].clone(), ok[0]["retryOf"].clone()),
        (json!(retry), json!(stop))
    );
    assert_eq!(audited(&l, "job.retry", "denied").await.len(), 2);
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn a_running_job_asked_to_cancel_stops_at_its_next_safe_point(pool: Pool) {
    let l = lab(pool);
    let p = people(&l).await;
    queue(
        &l,
        &p.alice,
        "POST",
        "/api/v1/instances",
        Some(spec("web-01")),
    )
    .await;
    settle(&l).await;
    let stop = queue(
        &l,
        &p.alice,
        "POST",
        "/api/v1/instances/acme-prod-live/web-01/state",
        Some(json!({ "action": "stop" })),
    )
    .await;
    // A worker claims it and stalls before reaching Incus.
    let claimed = apelora_persistence::claim_job(&l.pool, "stalled", 30)
        .await
        .expect("claim")
        .expect("job");
    assert_eq!(claimed.id, stop);
    let (s, b) = l
        .req(
            "POST",
            &format!("/api/v1/jobs/{stop}/cancel"),
            &p.alice,
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(
        (b["outcome"].as_str(), b["job"]["status"].as_str()),
        (Some("requested"), Some("running"))
    );
    assert_eq!(b["job"]["actions"]["cancel"]["allowed"], json!(false));
    // Asking again changes nothing.
    let (s, b) = l
        .req(
            "POST",
            &format!("/api/v1/jobs/{stop}/cancel"),
            &p.alice,
            None,
        )
        .await;
    assert_eq!(
        (s, b["outcome"].as_str()),
        (StatusCode::OK, Some("requested"))
    );
    // Its lease runs out; the next worker takes it over and stops before touching Incus.
    sqlx::query("UPDATE jobs SET lease_until = now() - interval '1 second' WHERE id = $1")
        .bind(stop)
        .execute(&l.pool)
        .await
        .expect("expire");
    settle(&l).await;
    let (_, d) = l
        .req(
            "GET",
            &format!("/api/v1/jobs/{stop}/detail"),
            &p.alice,
            None,
        )
        .await;
    assert_eq!(d["job"]["status"], json!("cancelled"), "{d}");
    let outcomes: Vec<&str> = d["attempts"]
        .as_array()
        .expect("attempts")
        .iter()
        .filter_map(|a| a["outcome"].as_str())
        .collect();
    assert_eq!(outcomes, ["lost", "cancelled"]);
    assert_eq!(
        l.state
            .provider
            .instance("acme-prod-live", "web-01")
            .await
            .map(|i| i.map(|i| i.status)),
        Ok(Some("Running".into())),
        "nothing was changed in Incus"
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn an_application_operation_is_not_retried(pool: Pool) {
    let l = lab(pool);
    let (_, admin) = l.super_admin("sub-root", "root").await;
    let app = apelora_persistence::ensure_application(&l.pool, "shop")
        .await
        .expect("app");
    let id = apelora_persistence::enqueue_job(
        &l.pool,
        "application.operation",
        Some(app.id),
        &json!({ "operation": Uuid::new_v4() }),
        None,
    )
    .await
    .expect("enqueue")
    .id();
    fail(&l, id).await;
    let (s, b) = l
        .req("GET", &format!("/api/v1/jobs/{id}"), &admin, None)
        .await;
    assert_eq!(s, StatusCode::OK);
    assert!(
        b["actions"]["retry"]["reason"]
            .as_str()
            .is_some_and(|r| r.contains("from its page")),
        "{b}"
    );
    let (s, b) = l
        .req("POST", &format!("/api/v1/jobs/{id}/retry"), &admin, None)
        .await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::CONFLICT, Some("not_retryable")),
        "{b}"
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn the_instance_page_and_its_logs_follow_who_may_see_and_manage_it(pool: Pool) {
    let l = lab(pool);
    let p = people(&l).await;
    queue(
        &l,
        &p.alice,
        "POST",
        "/api/v1/instances",
        Some(spec("web-01")),
    )
    .await;
    settle(&l).await;
    let page = "/api/v1/instances/acme-prod-live/web-01";

    let (s, b) = l.req("GET", page, &p.alice, None).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["source"]["provider"], json!("mock"));
    assert_eq!(
        (
            b["instance"]["status"].as_str(),
            b["instance"]["managed"].as_bool()
        ),
        (Some("Running"), Some(true))
    );
    assert_eq!(
        b["jobs"].as_array().map(Vec::len),
        Some(1),
        "its create job"
    );
    for (action, allowed) in [
        ("start", false),
        ("stop", true),
        ("restart", true),
        ("move", true),
        ("delete", true),
        ("logs", true),
    ] {
        assert_eq!(
            b["actions"][action]["allowed"],
            json!(allowed),
            "{action}: {}",
            b["actions"]
        );
    }
    assert!(
        b["actions"]["start"]["reason"]
            .as_str()
            .is_some_and(|r| r.contains("running"))
    );

    // The viewer sees it, cannot change it, and cannot read its logs.
    let (s, b) = l.req("GET", page, &p.viewer, None).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["actions"]["stop"]["allowed"], json!(false));
    assert_eq!(b["actions"]["logs"]["allowed"], json!(false));
    let (s, _) = l.req("GET", &format!("{page}/logs"), &p.viewer, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // Bob does not learn it exists.
    for path in [
        page.to_owned(),
        format!("{page}/logs"),
        format!("{page}/console-log"),
    ] {
        let (s, _) = l.req("GET", &path, &p.bob, None).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{path}");
    }

    let (s, b) = l.req("GET", &format!("{page}/logs"), &p.alice, None).await;
    assert_eq!(
        (s, b["files"].clone()),
        (StatusCode::OK, json!(["lxc.log"])),
        "{b}"
    );
    let (s, b) = l
        .req(
            "GET",
            &format!("{page}/logs/lxc.log?tailKib=1"),
            &p.alice,
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(
        (b["file"].as_str(), b["tailKib"].as_u64()),
        (Some("lxc.log"), Some(1))
    );
    assert!(
        b["log"]["content"]
            .as_str()
            .is_some_and(|c| c.starts_with("Mock · "))
    );
    let (s, b) = l
        .req("GET", &format!("{page}/console-log"), &p.alice, None)
        .await;
    assert_eq!(
        (s, b["file"].as_str()),
        (StatusCode::OK, Some("console")),
        "{b}"
    );
    for (path, want) in [
        (format!("{page}/logs/qemu.log"), StatusCode::NOT_FOUND),
        (format!("{page}/logs/lxc.log;x"), StatusCode::BAD_REQUEST),
        (
            format!("{page}/logs/lxc.log?tailKib=0"),
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("{page}/logs/lxc.log?tailKib=2048"),
            StatusCode::BAD_REQUEST,
        ),
        (
            "/api/v1/instances/acme-prod-live/nope".to_owned(),
            StatusCode::NOT_FOUND,
        ),
    ] {
        let (s, b) = l.req("GET", &path, &p.alice, None).await;
        assert_eq!(s, want, "{path}: {b}");
    }

    // An instance Apelora did not create: shown, never changed.
    let (s, b) = l
        .req(
            "GET",
            "/api/v1/instances/default/mock-web-01",
            &p.w.admin,
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert!(
        b["actions"]["stop"]["reason"]
            .as_str()
            .is_some_and(|r| r.contains("Not managed"))
    );
    assert_eq!(b["actions"]["logs"]["allowed"], json!(true));
}

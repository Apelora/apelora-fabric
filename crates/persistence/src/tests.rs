// Run against a real PostgreSQL (DATABASE_URL); each test gets a fresh database with the
// migrations applied. Locally: `docker compose -f infra/dev/compose.yaml up -d` and
// DATABASE_URL=postgres://apelora:apelora-dev-only@127.0.0.1:15432/apelora

use std::collections::HashSet;
use std::time::Duration;

use serde_json::json;

use super::*;

#[sqlx::test(migrator = "crate::MIGRATOR")]
async fn revisions_are_numbered_and_concurrent_saves_never_collide(pool: PgPool) {
    let app = create_application(&pool, "shop").await.expect("app");
    let r1 = add_revision(&pool, app.id, &json!({ "v": 1 }), "tester")
        .await
        .expect("rev 1");
    assert_eq!(r1.revision, 1);
    let mut handles = Vec::new();
    for i in 0..10 {
        let pool = pool.clone();
        handles.push(tokio::spawn(async move {
            add_revision(&pool, app.id, &json!({ "v": i }), "tester")
                .await
                .expect("rev")
                .revision
        }));
    }
    let mut numbers = HashSet::new();
    for h in handles {
        assert!(
            numbers.insert(h.await.expect("join")),
            "duplicate revision number"
        );
    }
    assert_eq!(numbers, (2..=11).collect());
    assert_eq!(
        latest_revision(&pool, app.id)
            .await
            .expect("latest")
            .expect("some")
            .revision,
        11
    );
}

#[sqlx::test(migrator = "crate::MIGRATOR")]
async fn duplicate_application_names_conflict_and_unknown_apps_are_not_found(pool: PgPool) {
    create_application(&pool, "shop").await.expect("app");
    assert!(matches!(
        create_application(&pool, "shop").await,
        Err(StoreError::Conflict(_))
    ));
    assert!(matches!(
        add_revision(&pool, Uuid::new_v4(), &json!({}), "t").await,
        Err(StoreError::NotFound(_))
    ));
}

#[sqlx::test(migrator = "crate::MIGRATOR")]
async fn enqueue_with_the_same_idempotency_key_creates_one_job(pool: PgPool) {
    let a = enqueue_job(&pool, "deploy", None, &json!({}), Some("deploy-shop-rev-3"))
        .await
        .expect("first");
    let b = enqueue_job(&pool, "deploy", None, &json!({}), Some("deploy-shop-rev-3"))
        .await
        .expect("second");
    assert!(matches!(a, Enqueued::Created(_)));
    assert_eq!(b, Enqueued::Existing(a.id()));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(count, 1);
}

#[sqlx::test(migrator = "crate::MIGRATOR")]
async fn concurrent_workers_never_claim_the_same_job(pool: PgPool) {
    for _ in 0..30 {
        enqueue_job(&pool, "noop", None, &json!({}), None)
            .await
            .expect("enqueue");
    }
    let mut workers = Vec::new();
    for w in 0..8 {
        let pool = pool.clone();
        workers.push(tokio::spawn(async move {
            let name = format!("worker-{w}");
            let mut mine = Vec::new();
            while let Some(job) = claim_job(&pool, &name, 30).await.expect("claim") {
                complete_job(&pool, &job, &name, &Outcome::Succeeded)
                    .await
                    .expect("complete");
                mine.push(job.id);
            }
            mine
        }));
    }
    let mut seen = HashSet::new();
    for w in workers {
        for id in w.await.expect("join") {
            assert!(seen.insert(id), "job {id} was claimed twice");
        }
    }
    assert_eq!(seen.len(), 30);
}

#[sqlx::test(migrator = "crate::MIGRATOR")]
async fn an_expired_lease_is_reclaimed_and_the_old_worker_is_fenced_out(pool: PgPool) {
    let id = enqueue_job(&pool, "deploy", None, &json!({}), None)
        .await
        .expect("enqueue")
        .id();
    let old = claim_job(&pool, "worker-a", 1)
        .await
        .expect("claim")
        .expect("job");
    assert!(
        claim_job(&pool, "worker-b", 30)
            .await
            .expect("claim")
            .is_none(),
        "lease still valid"
    );
    record_operation(&pool, &old, "worker-a", Some("op-1"))
        .await
        .expect("owner records its operation");
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let new = claim_job(&pool, "worker-b", 30)
        .await
        .expect("claim")
        .expect("reclaimed");
    assert_eq!(new.id, id);
    assert_eq!(new.generation, old.generation + 1);
    assert_eq!(
        new.external_operation.as_deref(),
        Some("op-1"),
        "the next attempt sees the operation the previous one started"
    );
    assert!(matches!(
        record_operation(&pool, &old, "worker-a", Some("op-2")).await,
        Err(StoreError::Conflict(_))
    ));
    let view = get_job(&pool, id).await.expect("view");
    assert_eq!(
        (
            view.status.as_str(),
            view.attempts,
            view.external_operation.as_deref()
        ),
        ("running", 2, Some("op-1"))
    );
    assert!(matches!(
        get_job(&pool, Uuid::new_v4()).await,
        Err(StoreError::NotFound(_))
    ));
    assert!(matches!(
        renew_lease(&pool, &old, "worker-a", 30).await,
        Err(StoreError::Conflict(_))
    ));
    assert!(matches!(
        complete_job(&pool, &old, "worker-a", &Outcome::Succeeded).await,
        Err(StoreError::Conflict(_))
    ));
    complete_job(&pool, &new, "worker-b", &Outcome::Succeeded)
        .await
        .expect("owner completes");
    assert_eq!(job_status(&pool, id).await.expect("status").0, "succeeded");
}

#[sqlx::test(migrator = "crate::MIGRATOR")]
async fn failures_retry_with_backoff_then_fail_permanently(pool: PgPool) {
    let id = enqueue_job(&pool, "deploy", None, &json!({}), None)
        .await
        .expect("enqueue")
        .id();
    sqlx::query("UPDATE jobs SET max_attempts = 2 WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .expect("cap");
    let j1 = claim_job(&pool, "w", 30)
        .await
        .expect("claim")
        .expect("job");
    complete_job(
        &pool,
        &j1,
        "w",
        &Outcome::Failed("incus unreachable".into()),
    )
    .await
    .expect("fail 1");
    let (status, attempts, err) = job_status(&pool, id).await.expect("status");
    assert_eq!(
        (status.as_str(), attempts, err.as_deref()),
        ("pending", 1, Some("incus unreachable"))
    );
    assert!(
        claim_job(&pool, "w", 30).await.expect("claim").is_none(),
        "backoff delays the retry"
    );
    sqlx::query("UPDATE jobs SET run_after = now() WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .expect("skip backoff");
    let j2 = claim_job(&pool, "w", 30)
        .await
        .expect("claim")
        .expect("retry");
    complete_job(
        &pool,
        &j2,
        "w",
        &Outcome::Failed("still unreachable".into()),
    )
    .await
    .expect("fail 2");
    assert_eq!(job_status(&pool, id).await.expect("status").0, "failed");
    assert!(
        claim_job(&pool, "w", 30).await.expect("claim").is_none(),
        "failed jobs are not retried"
    );
}

#[sqlx::test(migrator = "crate::MIGRATOR")]
async fn rejected_jobs_fail_without_retrying(pool: PgPool) {
    let id = enqueue_job(&pool, "instance.delete", None, &json!({}), None)
        .await
        .expect("enqueue")
        .id();
    let job = claim_job(&pool, "w", 30)
        .await
        .expect("claim")
        .expect("job");
    complete_job(&pool, &job, "w", &Outcome::Rejected("not Apelora's".into()))
        .await
        .expect("complete");
    let (status, attempts, err) = job_status(&pool, id).await.expect("status");
    assert_eq!(
        (status.as_str(), attempts, err.as_deref()),
        ("failed", 1, Some("not Apelora's"))
    );
    assert!(claim_job(&pool, "w", 30).await.expect("claim").is_none());
}

#[sqlx::test(migrator = "crate::MIGRATOR")]
async fn audit_events_are_recorded(pool: PgPool) {
    audit(
        &pool,
        "tester",
        "application.create",
        "shop",
        &json!({ "via": "test" }),
    )
    .await
    .expect("audit");
    let action: String = sqlx::query_scalar("SELECT action FROM audit_events")
        .fetch_one(&pool)
        .await
        .expect("row");
    assert_eq!(action, "application.create");
}

#[sqlx::test(migrator = "crate::MIGRATOR")]
async fn recent_jobs_are_newest_first_and_bounded(pool: PgPool) {
    let mut ids = Vec::new();
    for n in 0..3 {
        ids.push(
            enqueue_job(&pool, "noop", None, &json!({ "n": n }), None)
                .await
                .expect("enqueue")
                .id(),
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let jobs = recent_jobs(&pool, 2).await.expect("jobs");
    assert_eq!(
        jobs.iter().map(|j| j.id).collect::<Vec<_>>(),
        vec![ids[2], ids[1]]
    );
    assert_eq!(
        recent_jobs(&pool, 0).await.expect("jobs").len(),
        1,
        "at least one"
    );
}

#[sqlx::test(migrator = "crate::MIGRATOR")]
async fn applications_are_found_or_created_by_name_and_listed_with_their_latest_revision(
    pool: PgPool,
) {
    let a = ensure_application(&pool, "shop").await.expect("create");
    assert_eq!(
        ensure_application(&pool, "shop").await.expect("again"),
        a,
        "same row"
    );
    ensure_application(&pool, "blog").await.expect("create");
    add_revision(&pool, a.id, &json!({ "v": 1 }), "alex")
        .await
        .expect("rev");
    add_revision(&pool, a.id, &json!({ "v": 2 }), "alex")
        .await
        .expect("rev");
    let list = list_applications(&pool).await.expect("list");
    assert_eq!(
        list.iter()
            .map(|s| (s.name.as_str(), s.latest_revision))
            .collect::<Vec<_>>(),
        vec![("blog", None), ("shop", Some(2))]
    );
}

#[sqlx::test(migrator = "crate::MIGRATOR")]
async fn deployments_are_recorded_replaced_and_removed_when_empty(pool: PgPool) {
    let app = ensure_application(&pool, "shop").await.expect("app");
    record_deployment(
        &pool,
        app.id,
        "apelora",
        1,
        &json!([{ "appliance": "web-01" }]),
        None,
        None,
    )
    .await
    .expect("record");
    record_deployment(
        &pool,
        app.id,
        "apelora",
        2,
        &json!([{ "appliance": "web-01" }, { "appliance": "db-01" }]),
        Some(&json!([])),
        Some("incusbr0"),
    )
    .await
    .expect("replace");
    let all = list_deployments(&pool).await.expect("list");
    assert_eq!(all.len(), 1);
    assert_eq!(
        (
            all[0].application.as_str(),
            all[0].revision,
            all[0].uplink.as_deref()
        ),
        ("shop", 2, Some("incusbr0"))
    );
    assert_eq!(all[0].desired.as_array().map(Vec::len), Some(2));
    record_deployment(&pool, app.id, "apelora", 3, &json!([]), None, None)
        .await
        .expect("remove");
    assert!(list_deployments(&pool).await.expect("list").is_empty());
}

#[sqlx::test(migrator = "crate::MIGRATOR")]
async fn in_flight_jobs_are_found_by_instance_and_by_application(pool: PgPool) {
    let app = ensure_application(&pool, "shop").await.expect("app");
    let id = enqueue_job(
        &pool,
        "instance.create",
        Some(app.id),
        &json!({ "project": "apelora", "name": "shop-web-01" }),
        None,
    )
    .await
    .expect("job")
    .id();
    assert!(
        job_in_flight(&pool, "instance.create", "apelora", "shop-web-01")
            .await
            .expect("q")
    );
    assert!(
        !job_in_flight(&pool, "instance.create", "apelora", "shop-web-01-r2")
            .await
            .expect("q")
    );
    assert!(
        application_job_in_flight(&pool, "instance.create", app.id)
            .await
            .expect("q")
    );
    sqlx::query("UPDATE jobs SET status = 'succeeded' WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .expect("done");
    assert!(
        !job_in_flight(&pool, "instance.create", "apelora", "shop-web-01")
            .await
            .expect("q")
    );
}

use serde_json::json;
use time::Duration;

use super::*;

fn new<'a>(rule: &'a str, subject: &'a str, firing: bool) -> NewAlert<'a> {
    NewAlert {
        rule,
        subject,
        severity: "warning",
        firing,
        summary: "it happened",
        value: 2.0,
        threshold: 1.0,
    }
}

#[sqlx::test(migrator = "crate::MIGRATOR")]
async fn one_open_alert_per_rule_and_subject_and_its_life(pool: PgPool) {
    let t0 = now(&pool).await.expect("now");
    let mut c = pool.acquire().await.expect("conn");
    let a = open_alert(&mut c, &new("job-failures", "instance.create", false), t0)
        .await
        .expect("open");
    assert_eq!((a.state.as_str(), a.fired_at), ("pending", None));
    assert!(
        open_alert(&mut c, &new("job-failures", "instance.create", true), t0)
            .await
            .is_err(),
        "deduplicated"
    );
    // Another subject is another alert.
    open_alert(&mut c, &new("job-failures", "instance.delete", true), t0)
        .await
        .expect("other subject");
    let seen = Seen {
        severity: "critical",
        summary: "3 failed",
        value: 3.0,
        threshold: 1.0,
    };
    let t1 = t0 + Duration::seconds(60);
    let fired = refresh(&mut c, a.id, &seen, true, t1).await.expect("fire");
    assert_eq!((fired.state.as_str(), fired.fired_at), ("firing", Some(t1)));
    assert_eq!((fired.severity.as_str(), fired.value), ("critical", 3.0));
    clearing(&mut c, a.id, t1).await.expect("clear");
    clearing(&mut c, a.id, t1 + Duration::seconds(30))
        .await
        .expect("kept");
    assert_eq!(alert(&pool, a.id).await.expect("row").cleared_at, Some(t1));
    // Holding again forgets the clearing.
    let again = refresh(&mut c, a.id, &seen, false, t1 + Duration::seconds(40))
        .await
        .expect("refresh");
    assert_eq!(again.cleared_at, None);

    // Acknowledged once, by whom, with a note; not twice.
    let mut tx = crate::begin(&pool).await.expect("begin");
    match acknowledge(&mut tx, a.id, "user:1", "ada", "looking")
        .await
        .expect("ack")
    {
        Ack::Done(row) => assert_eq!(
            (row.acknowledged_by_name.as_deref(), row.ack_note.as_deref()),
            (Some("ada"), Some("looking"))
        ),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        acknowledge(&mut tx, a.id, "user:2", "bob", "")
            .await
            .expect("again"),
        Ack::Already("ada".into())
    );
    crate::commit(tx).await.expect("commit");

    let t2 = t1 + Duration::seconds(120);
    let resolved = resolve(&mut c, a.id, t2).await.expect("resolve");
    assert_eq!(
        (resolved.state.as_str(), resolved.resolved_at),
        ("resolved", Some(t2))
    );
    assert!(resolve(&mut c, a.id, t2).await.is_err(), "only once");
    let mut tx = crate::begin(&pool).await.expect("begin");
    assert_eq!(
        acknowledge(&mut tx, a.id, "user:2", "bob", "")
            .await
            .expect("resolved"),
        Ack::NotFiring("resolved".into())
    );
    drop(tx);
    // Once resolved, the same condition opens a new alert.
    let b = open_alert(&mut c, &new("job-failures", "instance.create", false), t2)
        .await
        .expect("new occurrence");
    assert_ne!(b.id, a.id);
    drop_pending(&mut c, b.id).await.expect("drop");
    let open: Vec<String> = open_alerts(&pool)
        .await
        .expect("open")
        .into_iter()
        .map(|r| r.subject)
        .collect();
    assert_eq!(open, vec!["instance.delete"]);
    assert_eq!(
        history(&pool, None, 10)
            .await
            .expect("history")
            .iter()
            .map(|r| r.id)
            .collect::<Vec<_>>(),
        vec![a.id]
    );
    assert!(
        history(&pool, Some(a.id), 10)
            .await
            .expect("older")
            .is_empty()
    );
    assert_eq!(
        prune_resolved(&mut c, 3600.0, t2 + Duration::seconds(10))
            .await
            .expect("prune"),
        0,
        "kept while recent"
    );
    assert_eq!(
        prune_resolved(&mut c, 3600.0, t2 + Duration::seconds(3601))
            .await
            .expect("prune"),
        1
    );
}

#[sqlx::test(migrator = "crate::MIGRATOR")]
async fn silences_run_for_their_window_and_end_early_once(pool: PgPool) {
    let s = create_silence(
        &pool,
        &NewSilence {
            rule: "member-offline",
            subject: Some("node-2"),
            minutes: 30,
            note: "maintenance",
            by: "user:1",
            by_name: "ada",
        },
    )
    .await
    .expect("silence");
    let t = now(&pool).await.expect("now");
    assert_eq!(active_silences(&pool, t).await.expect("active").len(), 1);
    assert!(
        active_silences(&pool, t + Duration::minutes(31))
            .await
            .expect("later")
            .is_empty(),
        "runs out"
    );
    assert!(end_silence(&pool, s.id, "user:1").await.expect("end"));
    assert!(!end_silence(&pool, s.id, "user:1").await.expect("again"));
    let ended = silence(&pool, s.id).await.expect("row");
    assert_eq!(ended.ended_by.as_deref(), Some("user:1"));
}

#[sqlx::test(migrator = "crate::MIGRATOR")]
async fn channels_keep_a_secret_reference_and_their_delivery_state_comes_from_the_outbox(
    pool: PgPool,
) {
    let input = ChannelInput {
        name: "ops",
        url: "https://hooks.example/apelora",
        secret_ref: "APELORA_ALERT_SECRET_OPS",
        enabled: true,
        min_severity: "warning",
    };
    let c = create_channel(&pool, &input, "user:1")
        .await
        .expect("create");
    assert!(matches!(
        create_channel(&pool, &input, "user:1").await,
        Err(StoreError::Conflict(_))
    ));
    // The database refuses what the API would refuse too.
    for (url, secret) in [
        ("http://hooks.example/x", "APELORA_ALERT_SECRET_OPS"),
        ("https://hooks.example/x", "DATABASE_URL"),
    ] {
        let bad = ChannelInput {
            name: "bad",
            url,
            secret_ref: secret,
            ..input
        };
        assert!(
            create_channel(&pool, &bad, "user:1").await.is_err(),
            "{url} {secret}"
        );
    }
    let mut tx = crate::begin(&pool).await.expect("begin");
    assert_eq!(
        notify(&mut tx, &[c.id, c.id], &json!({ "event": "fired" }))
            .await
            .expect("notify")
            .len(),
        2
    );
    crate::commit(tx).await.expect("commit");
    let due = crate::outbox::due(&pool, 10).await.expect("due");
    assert_eq!(due.len(), 2);
    assert_eq!(due[0].topic, ALERT_TOPIC);
    assert_eq!(due[0].payload["channel"], json!(c.id.to_string()));
    crate::outbox::mark_failed(&pool, due[0].id, "status 502", 60.0)
        .await
        .expect("fail");
    crate::outbox::mark_delivered(&pool, &[due[1].id])
        .await
        .expect("delivered");
    let d = channel_deliveries(&pool).await.expect("deliveries");
    assert_eq!(d.len(), 1);
    assert_eq!(
        (
            d[0].channel.as_str(),
            d[0].pending,
            d[0].failing,
            d[0].last_error.as_deref()
        ),
        (c.id.to_string().as_str(), 1, 1, Some("status 502"))
    );
    assert!(d[0].last_delivered_at.is_some());
    let renamed = update_channel(
        &pool,
        c.id,
        &ChannelInput {
            name: "ops-2",
            enabled: false,
            ..input
        },
    )
    .await
    .expect("update");
    assert_eq!((renamed.name.as_str(), renamed.enabled), ("ops-2", false));
    assert!(delete_channel(&pool, c.id).await.expect("delete"));
    assert!(matches!(
        channel(&pool, c.id).await,
        Err(StoreError::NotFound(_))
    ));
}

#[sqlx::test(migrator = "crate::MIGRATOR")]
async fn rule_settings_and_the_evaluator_row_are_upserted(pool: PgPool) {
    assert!(rule_settings(&pool).await.expect("none").is_empty());
    put_rule_settings(
        &pool,
        "job-queue-wait",
        true,
        600.0,
        120,
        "critical",
        "user:1",
    )
    .await
    .expect("put");
    let row = put_rule_settings(
        &pool,
        "job-queue-wait",
        false,
        900.0,
        0,
        "warning",
        "user:2",
    )
    .await
    .expect("put again");
    assert_eq!(
        (
            row.enabled,
            row.threshold,
            row.for_secs,
            row.updated_by.as_str()
        ),
        (false, 900.0, 0, "user:2")
    );
    assert_eq!(rule_settings(&pool).await.expect("one").len(), 1);
    assert!(evaluator(&pool).await.expect("none").is_none());
    let t = now(&pool).await.expect("now");
    record_pass(
        &pool,
        t,
        0.2,
        "worker-a",
        30,
        &json!({ "job-failures": "ok" }),
    )
    .await
    .expect("pass");
    record_pass(
        &pool,
        t,
        0.3,
        "worker-b",
        30,
        &json!({ "job-failures": "ok" }),
    )
    .await
    .expect("pass");
    let e = evaluator(&pool).await.expect("read").expect("row");
    assert_eq!(
        (e.worker.as_str(), e.duration_secs, e.every_secs),
        ("worker-b", 0.3, 30)
    );
}

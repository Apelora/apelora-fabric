// SPDX-License-Identifier: AGPL-3.0-only
//! The deployment worker's job loop: claim a job with a lease, run its handler, record the
//! outcome fenced by the claim's generation. Instance lifecycle jobs change Incus through the
//! [`Provider`]; see [`lifecycle`].

pub mod health;
pub mod lifecycle;
pub mod metrics;
pub mod mobility;
pub mod network;
pub mod operations;
pub mod storage;
pub mod supervisor;
pub mod wake;

use std::time::Duration;

use apelora_incus_adapter::Provider;
use apelora_persistence::{
    ClaimedJob, Outcome, Pool, StoreError, claim_job, complete_job, renew_lease,
};
use tracing::Instrument;

/// Lease length; renewed at a third of this while a handler runs.
pub const LEASE_SECS: i64 = 30;

/// Runs the handler for one job kind. Unknown kinds fail (and are retried, then marked failed),
/// so a newer API cannot silently enqueue work an older worker ignores.
async fn handle(
    pool: &Pool,
    provider: &Provider,
    job: &ClaimedJob,
    worker: &str,
) -> Result<Outcome, StoreError> {
    if job.kind == "noop" {
        return Ok(Outcome::Succeeded);
    }
    if job.kind == "application.operation" {
        return operations::run(pool, provider, job).await;
    }
    if job.kind == "instance.move" {
        return mobility::run(pool, provider, job, worker).await;
    }
    if job.kind == "volume.backup" {
        return storage::run_backup_job(pool, provider, &job.payload).await;
    }
    if job.kind == "network.sync" {
        return Ok(match network::from_payload(&job.payload) {
            Ok(s) => network::sync(pool, provider, &s).await,
            Err(msg) => Outcome::Rejected(msg),
        });
    }
    match lifecycle::Goal::from_job(&job.kind, &job.payload) {
        Some(Ok(goal)) => lifecycle::run(pool, provider, job, worker, goal).await,
        Some(Err(msg)) => Ok(Outcome::Rejected(msg)),
        None => Ok(Outcome::Failed(format!(
            "this worker has no handler for job kind '{}'",
            job.kind
        ))),
    }
}

/// What a job works on, for logs: `project/name` for instance jobs, the application otherwise.
pub fn target_of(job: &ClaimedJob) -> String {
    let field = |k: &str| job.payload.get(k).and_then(serde_json::Value::as_str);
    match (field("project"), field("name"), field("application")) {
        (Some(p), Some(n), _) => format!("{p}/{n}"),
        (_, _, Some(a)) => a.to_owned(),
        _ => job
            .application_id
            .map_or_else(|| "-".to_owned(), |id| id.to_string()),
    }
}

/// Claims and runs at most one job. Returns `Ok(false)` when nothing was runnable.
pub async fn run_once(pool: &Pool, provider: &Provider, worker: &str) -> Result<bool, StoreError> {
    let Some(job) = claim_job(pool, worker, LEASE_SECS).await? else {
        return Ok(false);
    };
    // Every line logged while the job runs (lifecycle, moves, storage, networks) carries it,
    // with the API request that queued the job ("-" when none did: supervisor, scheduler).
    let span = tracing::error_span!(
        "job",
        job = %job.id,
        kind = %job.kind,
        target = %target_of(&job),
        request_id = job.request_id.as_deref().unwrap_or("-"),
    );
    let id = job.id;
    run_claimed(pool, provider, worker, job)
        .instrument(span)
        .await
        // A database error here loses the job's context otherwise: name the job.
        .inspect_err(|e| tracing::error!(job = %id, error = %e, "job: database error while running the job; it is retried once its lease expires"))
}

async fn run_claimed(
    pool: &Pool,
    provider: &Provider,
    worker: &str,
    job: ClaimedJob,
) -> Result<bool, StoreError> {
    tracing::info!(
        generation = job.generation,
        attempt = job.attempts,
        max_attempts = job.max_attempts,
        "job: claimed"
    );
    let renew = {
        let (pool, job, worker) = (pool.clone(), job.clone(), worker.to_owned());
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs((LEASE_SECS / 3) as u64)).await;
                if let Err(e) = renew_lease(&pool, &job, &worker, LEASE_SECS).await {
                    if matches!(e, StoreError::Conflict(_)) {
                        apelora_metrics::LEASE_LOST.inc(&["renew"]);
                    }
                    tracing::warn!(job = %job.id, error = %e, "job: lost the lease; stopping renewals");
                    return;
                }
            }
        })
    };
    let started = std::time::Instant::now();
    let outcome = if cancelled_before_start(&job) {
        Ok(Outcome::Cancelled(
            "cancelled before this attempt changed anything".into(),
        ))
    } else {
        handle(pool, provider, &job, worker).await
    };
    renew.abort();
    let kind = apelora_metrics::job_kind_label(&job.kind);
    let record = |outcome: &str| {
        apelora_metrics::JOB_ATTEMPTS.inc(&[kind, outcome]);
        apelora_metrics::JOB_DURATION.observe(&[kind, outcome], started.elapsed().as_secs_f64());
    };
    let outcome = match outcome {
        Ok(o) => o,
        // Lost the job to another worker mid-way: its attempt owns the result now.
        Err(StoreError::Conflict(msg)) => {
            record("lost");
            apelora_metrics::LEASE_LOST.inc(&["handler"]);
            tracing::warn!(%msg, "job: stopped working on it (another worker holds it now)");
            return Ok(true);
        }
        Err(e) => return Err(e),
    };
    match complete_job(pool, &job, worker, &outcome).await {
        Ok(()) => {
            record(outcome_label(&job, &outcome));
            log_outcome(&job, &outcome)
        }
        // Another worker took over after our lease expired; its result wins.
        Err(StoreError::Conflict(msg)) => {
            record("lost");
            apelora_metrics::LEASE_LOST.inc(&["complete"]);
            tracing::warn!(%msg, "job: result discarded (another worker holds it now)")
        }
        Err(e) => return Err(e),
    }
    Ok(true)
}

/// The first safe point of every job kind: claimed with a cancellation request and nothing of an
/// earlier attempt still in flight in Incus. A coordinating `application.operation` decides for
/// itself (it must close its operation record), so does `instance.move` (an earlier attempt may
/// have stopped the instance, which must run again before the job ends), and a job whose earlier
/// attempt left an Incus operation unfinished waits on it first, so the job never ends while
/// Incus is still changing something on its behalf.
///
/// The later safe points, per kind:
/// - `instance.start|stop|restart|delete`: before each Incus change (`lifecycle`);
/// - `instance.create`: only before the instance exists; once created it gets its volumes and
///   is started, so a cancelled create never leaves a half-built instance;
/// - `instance.move`: while the instance is still on its original member (started again there
///   if the move had stopped it); once on the new member the move is finished;
/// - `application.operation`: between phases; the running phase's instance jobs finish;
/// - `network.sync`, `volume.backup`: none after the start (each is one idempotent pass).
pub fn cancelled_before_start(job: &ClaimedJob) -> bool {
    job.cancel_requested
        && job.external_operation.is_none()
        && !matches!(job.kind.as_str(), "application.operation" | "instance.move")
}

/// The outcome of an attempt as a metrics label: a failure that will run again is a `retry`.
fn outcome_label(job: &ClaimedJob, outcome: &Outcome) -> &'static str {
    match outcome {
        Outcome::Succeeded => "succeeded",
        Outcome::Waiting(_) => "waiting",
        Outcome::Cancelled(_) => "cancelled",
        Outcome::Failed(_) if job.attempts < job.max_attempts => "retry",
        Outcome::Failed(_) => "failed",
        Outcome::Rejected(_) => "rejected",
    }
}

/// One line per finished attempt: info when it succeeded or waits, warn when it failed and will
/// be retried (with the back-off `complete_job` applies), error when it will not run again.
fn log_outcome(job: &ClaimedJob, outcome: &Outcome) {
    let (attempt, max_attempts) = (job.attempts, job.max_attempts);
    match outcome {
        Outcome::Succeeded => tracing::info!(attempt, "job: succeeded"),
        Outcome::Waiting(secs) => tracing::debug!(after_secs = secs, "job: waiting; runs again"),
        Outcome::Cancelled(note) => {
            tracing::info!(attempt, %note, "job: cancelled on request at a safe point")
        }
        Outcome::Failed(error) if attempt < max_attempts => {
            let retry_in_secs = 2u32.saturating_pow(attempt.max(0).unsigned_abs()).min(300);
            tracing::warn!(attempt, max_attempts, retry_in_secs, %error, "job: attempt failed; will be retried")
        }
        Outcome::Failed(error) => {
            tracing::error!(attempt, max_attempts, %error, "job: failed; no attempts left")
        }
        Outcome::Rejected(error) => {
            tracing::error!(attempt, %error, "job: rejected; retrying cannot help")
        }
    }
}

/// Processes jobs until `shutdown` resolves. With no work it waits for a job to be queued
/// (`LISTEN`, see [`wake`]) and checks again every `idle` in any case.
pub async fn run(
    pool: Pool,
    provider: Provider,
    worker: String,
    idle: Duration,
    shutdown: impl std::future::Future<Output = ()>,
) {
    tokio::pin!(shutdown);
    // Says "alive" from its own task, so a long job never makes the worker look dead.
    let beats = tokio::spawn(heartbeat(pool.clone(), worker.clone(), HEARTBEAT));
    let mut wake = wake::JobWake::listen(pool.clone()).await;
    loop {
        let worked = match run_once(&pool, &provider, &worker).await {
            Ok(w) => w,
            Err(e) => {
                apelora_metrics::STORE_ERRORS.inc(&[if e.is_unavailable() {
                    "store_unavailable"
                } else {
                    "store_error"
                }]);
                tracing::error!(error = %e, idle_secs = idle.as_secs_f64(), "job loop error; backing off");
                false
            }
        };
        if !worked {
            tokio::select! {
                _ = &mut shutdown => break,
                _ = wake.wait(idle) => {}
            }
        }
    }
    beats.abort();
    if let Err(e) = apelora_persistence::status::worker_stopped(&pool, &worker).await {
        tracing::warn!(error = %e, "heartbeat: could not record the shutdown");
    }
    tracing::info!("worker stopped");
}

/// How often a worker writes its heartbeat (the Health page calls it lost after 45 s).
pub const HEARTBEAT: Duration = Duration::from_secs(15);

/// The host name for the heartbeat: the kernel's (Linux), `HOSTNAME`, or `unknown`.
fn host_name() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .or_else(|| std::env::var("HOSTNAME").ok())
        .map(|h| {
            h.trim()
                .chars()
                .filter(|c| !c.is_control())
                .take(200)
                .collect::<String>()
        })
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

/// Registers the worker, then writes a heartbeat every `every` until aborted. A database
/// outage is logged when it starts and when it ends, not on every beat.
pub async fn heartbeat(pool: Pool, worker: String, every: Duration) {
    let (host, version) = (host_name(), env!("CARGO_PKG_VERSION"));
    let mut failing =
        match apelora_persistence::status::worker_started(&pool, &worker, &host, version).await {
            Ok(()) => false,
            Err(e) => {
                tracing::warn!(error = %e, "heartbeat: could not register the worker; retrying");
                true
            }
        };
    let mut tick = tokio::time::interval(every);
    tick.tick().await;
    loop {
        tick.tick().await;
        match apelora_persistence::status::worker_heartbeat(&pool, &worker, &host, version).await {
            Ok(()) if failing => {
                tracing::info!("heartbeat: written again");
                failing = false;
            }
            Ok(()) => {}
            Err(e) if !failing => {
                tracing::warn!(error = %e, "heartbeat: could not be written; the Health page will show this worker as lost");
                failing = true;
            }
            Err(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apelora_incus_adapter::MockIncus;
    use apelora_persistence::{enqueue_job, get_job, job_status};
    use serde_json::json;

    fn mock() -> Provider {
        Provider::Mock(MockIncus::default())
    }

    #[test]
    fn a_job_names_its_target_for_logs() {
        let job = |payload: serde_json::Value| ClaimedJob {
            id: uuid::Uuid::nil(),
            kind: "x".into(),
            application_id: None,
            payload,
            attempts: 1,
            max_attempts: 5,
            generation: 1,
            external_operation: None,
            request_id: None,
            cancel_requested: false,
        };
        assert_eq!(
            target_of(&job(json!({ "project": "apelora", "name": "web-01" }))),
            "apelora/web-01"
        );
        assert_eq!(
            target_of(&job(json!({ "application": "shop", "project": "apelora" }))),
            "shop"
        );
        assert_eq!(target_of(&job(json!({}))), "-");
    }

    async fn drain(pool: &Pool, provider: &Provider) {
        while run_once(pool, provider, "test-worker").await.expect("run") {}
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn processes_known_jobs_and_fails_unknown_kinds(pool: Pool) {
        let ok = enqueue_job(&pool, "noop", None, &json!({}), None)
            .await
            .expect("enqueue")
            .id();
        let unknown = enqueue_job(&pool, "deploy-v99", None, &json!({}), None)
            .await
            .expect("enqueue")
            .id();
        sqlx::query("UPDATE jobs SET max_attempts = 1 WHERE id = $1")
            .bind(unknown)
            .execute(&pool)
            .await
            .expect("cap");
        drain(&pool, &mock()).await;
        assert_eq!(job_status(&pool, ok).await.expect("status").0, "succeeded");
        let (status, _, err) = job_status(&pool, unknown).await.expect("status");
        assert_eq!(status, "failed");
        assert!(
            err.unwrap_or_default()
                .contains("no handler for job kind 'deploy-v99'")
        );
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn run_stops_on_shutdown(pool: Pool) {
        // Shut down once the worker has registered (a fixed delay was too short when many
        // database tests run at once), so the test checks the result, not a race.
        let registered = {
            let pool = pool.clone();
            async move {
                while apelora_persistence::status::workers(&pool)
                    .await
                    .map_or(true, |w| w.is_empty())
                {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
        };
        let handle = tokio::spawn(run(
            pool.clone(),
            mock(),
            "test-worker".into(),
            Duration::from_millis(20),
            registered,
        ));
        tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("stops in time")
            .expect("join");
        // It registered its heartbeat and said it stopped cleanly.
        let w = apelora_persistence::status::workers(&pool)
            .await
            .expect("workers");
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].id, "test-worker");
        assert_eq!(w[0].version, env!("CARGO_PKG_VERSION"));
        assert!(w[0].stopped_at.is_some());
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn heartbeats_keep_coming_while_a_worker_runs(pool: Pool) {
        let beats = tokio::spawn(heartbeat(
            pool.clone(),
            "w-beat".into(),
            Duration::from_millis(50),
        ));
        let first_seen = || async {
            apelora_persistence::status::workers(&pool)
                .await
                .expect("workers")
                .first()
                .map(|w| w.last_seen)
        };
        let mut first = None;
        for _ in 0..100 {
            first = first_seen().await;
            if first.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let first = first.expect("registered");
        let mut later = first;
        for _ in 0..100 {
            later = first_seen().await.expect("still there");
            if later > first {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        beats.abort();
        assert!(later > first, "a heartbeat moved last_seen forward");
    }

    async fn job(pool: &Pool, kind: &str, payload: serde_json::Value) -> uuid::Uuid {
        enqueue_job(pool, kind, None, &payload, None)
            .await
            .expect("enqueue")
            .id()
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn instance_lifecycle_converges_through_the_queue(pool: Pool) {
        let provider = mock();
        let target = json!({ "project": "apelora", "name": "web-01" });
        let create = job(
            &pool,
            "instance.create",
            json!({ "project": "apelora", "name": "web-01", "kind": "container",
                    "source": { "protocol": "oci", "server": "https://registry.example", "alias": "nginx:1.29" },
                    "start": true }),
        )
        .await;
        drain(&pool, &provider).await;
        assert_eq!(
            get_job(&pool, create).await.expect("job").status,
            "succeeded"
        );
        assert_eq!(
            provider
                .instance("apelora", "web-01")
                .await
                .map(|s| s.map(|s| (s.status, s.managed))),
            Ok(Some(("Running".into(), true)))
        );

        // Creating again is a no-op, not an error.
        let again = enqueue_job(
            &pool,
            "instance.create",
            None,
            &get_job(&pool, create).await.expect("job").payload,
            None,
        )
        .await
        .expect("enqueue")
        .id();
        let stop = job(&pool, "instance.stop", target.clone()).await;
        drain(&pool, &provider).await;
        for id in [again, stop] {
            assert_eq!(get_job(&pool, id).await.expect("job").status, "succeeded");
        }
        assert_eq!(
            provider
                .instance("apelora", "web-01")
                .await
                .map(|s| s.map(|s| s.status)),
            Ok(Some("Stopped".into()))
        );

        // Deleting a running instance stops it first; deleting twice is a no-op.
        let start = job(&pool, "instance.start", target.clone()).await;
        drain(&pool, &provider).await;
        let delete = job(&pool, "instance.delete", target.clone()).await;
        let delete_again = job(&pool, "instance.delete", target).await;
        drain(&pool, &provider).await;
        for id in [start, delete, delete_again] {
            let j = get_job(&pool, id).await.expect("job");
            assert_eq!(
                (j.status.as_str(), j.external_operation),
                ("succeeded", None)
            );
        }
        assert_eq!(provider.instance("apelora", "web-01").await, Ok(None));
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn volumes_are_created_where_incus_placed_the_instance_and_outlive_it(pool: Pool) {
        let Provider::Mock(m) = mock() else {
            unreachable!()
        };
        let provider = Provider::Mock(m.clone());
        let spec = json!({ "project": "apelora", "name": "shop-db-01", "kind": "container",
            "source": { "protocol": "oci", "server": "https://registry.example", "alias": "postgres:18" },
            "start": true,
            "owner": { "application": "shop", "module": "db", "replica": 1 },
            "volumes": [{ "name": "data", "sizeMib": 1024, "path": "/var/lib/postgresql", "pool": "local" }] });
        // The pool is full at first: the job fails and nothing is started without its data.
        m.storage().fail_create.insert("shop-db-01-data".into());
        let create = job(&pool, "instance.create", spec).await;
        sqlx::query("UPDATE jobs SET max_attempts = 1 WHERE id = $1")
            .bind(create)
            .execute(&pool)
            .await
            .expect("cap");
        drain(&pool, &provider).await;
        let (status, _, err) = job_status(&pool, create).await.expect("status");
        assert_eq!(status, "failed");
        assert!(err.unwrap_or_default().contains("pool local is full"));
        let state = provider
            .instance("apelora", "shop-db-01")
            .await
            .expect("read")
            .expect("created");
        assert_eq!(
            (state.status.as_str(), state.volume_devices.len()),
            ("Stopped", 0)
        );

        // Room again: a new attempt creates the volume on the instance's member, attaches, starts.
        m.storage().fail_create.clear();
        let retry = enqueue_job(
            &pool,
            "instance.create",
            None,
            &get_job(&pool, create).await.expect("job").payload,
            None,
        )
        .await
        .expect("enqueue")
        .id();
        drain(&pool, &provider).await;
        assert_eq!(
            get_job(&pool, retry).await.expect("job").status,
            "succeeded"
        );
        let state = provider
            .instance("apelora", "shop-db-01")
            .await
            .expect("read")
            .expect("exists");
        assert_eq!(
            (state.status.as_str(), state.volume_devices.clone()),
            ("Running", vec!["vol-data".to_owned()])
        );
        let volume = provider
            .volume("apelora", "local", Some(&state.location), "shop-db-01-data")
            .await
            .expect("read")
            .expect("volume on the instance's member");
        assert_eq!(
            (volume.size_mib, volume.application.as_deref()),
            (Some(1024), Some("shop"))
        );
        assert_eq!(volume.used_by.len(), 1);

        // Deleting the instance keeps its data.
        let delete = job(
            &pool,
            "instance.delete",
            json!({ "project": "apelora", "name": "shop-db-01" }),
        )
        .await;
        drain(&pool, &provider).await;
        assert_eq!(
            get_job(&pool, delete).await.expect("job").status,
            "succeeded"
        );
        let kept = provider
            .volume("apelora", "local", Some(&state.location), "shop-db-01-data")
            .await
            .expect("read")
            .expect("volume kept");
        assert!(kept.used_by.is_empty());
        // Only its own application may delete it.
        assert!(
            provider
                .delete_volume(
                    "apelora",
                    "local",
                    Some(&state.location),
                    "shop-db-01-data",
                    "other"
                )
                .await
                .is_err()
        );
        assert_eq!(
            provider
                .delete_volume(
                    "apelora",
                    "local",
                    Some(&state.location),
                    "shop-db-01-data",
                    "shop"
                )
                .await,
            Ok(true)
        );
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn a_volume_that_is_not_ours_is_never_adopted(pool: Pool) {
        let Provider::Mock(m) = mock() else {
            unreachable!()
        };
        let provider = Provider::Mock(m.clone());
        m.storage().volumes.insert(
            "apelora/local/mock-member-01/shop-db-01-data".into(),
            apelora_incus_adapter::VolumeState {
                managed: false,
                application: None,
                content: "filesystem".into(),
                size_mib: Some(1024),
                location: "mock-member-01".into(),
                used_by: Vec::new(),
            },
        );
        let create = job(&pool, "instance.create", json!({ "project": "apelora", "name": "shop-db-01", "kind": "container",
            "source": { "protocol": "oci", "server": "https://registry.example", "alias": "postgres:18" },
            "start": true, "owner": { "application": "shop", "module": "db", "replica": 1 },
            "volumes": [{ "name": "data", "sizeMib": 1024, "path": "/data", "pool": "local" }] })).await;
        sqlx::query("UPDATE jobs SET max_attempts = 1 WHERE id = $1")
            .bind(create)
            .execute(&pool)
            .await
            .expect("cap");
        drain(&pool, &provider).await;
        let (status, _, err) = job_status(&pool, create).await.expect("status");
        assert_eq!(status, "failed");
        assert!(
            err.unwrap_or_default()
                .contains("was not created by Apelora")
        );
        let state = provider
            .instance("apelora", "shop-db-01")
            .await
            .expect("read")
            .expect("created");
        assert_eq!(state.status, "Stopped");
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn foreign_instances_are_never_changed(pool: Pool) {
        let provider = mock();
        let before = provider.instances().await.expect("instances");
        let ids = [
            job(
                &pool,
                "instance.delete",
                json!({ "project": "default", "name": "mock-web-01" }),
            )
            .await,
            job(
                &pool,
                "instance.stop",
                json!({ "project": "default", "name": "mock-db-01" }),
            )
            .await,
            job(
                &pool,
                "instance.delete",
                json!({ "project": "default", "name": "../../1.0" }),
            )
            .await,
        ];
        drain(&pool, &provider).await;
        for id in ids {
            let j = get_job(&pool, id).await.expect("job");
            assert_eq!(
                (j.status.as_str(), j.attempts),
                ("failed", 1),
                "rejected without retries"
            );
        }
        assert!(
            get_job(&pool, ids[0])
                .await
                .expect("job")
                .last_error
                .unwrap_or_default()
                .contains("not created by Apelora")
        );
        assert_eq!(provider.instances().await.expect("instances"), before);
    }

    /// Claims the next job, lets `between` run (a cancellation arriving mid-attempt), then runs it.
    async fn claim_then<F: std::future::Future<Output = ()>>(
        pool: &Pool,
        provider: &Provider,
        between: impl FnOnce(uuid::Uuid) -> F,
    ) -> uuid::Uuid {
        let job = claim_job(pool, "w", LEASE_SECS)
            .await
            .expect("claim")
            .expect("a job");
        let id = job.id;
        between(id).await;
        run_claimed(pool, provider, "w", job).await.expect("run");
        id
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn a_lifecycle_job_stops_before_its_incus_change_and_leaves_the_instance_as_it_was(
        pool: Pool,
    ) {
        let provider = mock();
        create_instance(&pool, &provider, "web").await;
        job(
            &pool,
            "instance.stop",
            json!({ "project": "apelora", "name": "web" }),
        )
        .await;
        let id = claim_then(&pool, &provider, |id| {
            let pool = pool.clone();
            async move {
                apelora_persistence::jobs::request_cancel(&pool, id, "user:alice")
                    .await
                    .expect("cancel");
            }
        })
        .await;
        let j = get_job(&pool, id).await.expect("job");
        assert_eq!(j.status, "cancelled");
        assert!(
            j.incus_operations.is_empty(),
            "Incus was not asked to do anything"
        );
        let a = apelora_persistence::jobs::job_attempts(&pool, id)
            .await
            .expect("attempts");
        assert!(
            a[0].error
                .as_deref()
                .is_some_and(|e| e.contains("cancelled by user:alice before stopping apelora/web")),
            "{a:?}"
        );
        assert_eq!(status_of(&provider, "web").await, "Running");
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn a_create_whose_instance_exists_finishes_instead_of_leaving_it_half_built(pool: Pool) {
        let provider = mock();
        let spec = |start: bool| {
            json!({ "project": "apelora", "name": "web", "kind": "container",
                    "source": { "protocol": "oci", "server": "https://registry.example", "alias": "nginx:1.29" },
                    "start": start })
        };
        job(&pool, "instance.create", spec(false)).await;
        drain(&pool, &provider).await;
        assert_eq!(status_of(&provider, "web").await, "Stopped");
        job(&pool, "instance.create", spec(true)).await;
        let id = claim_then(&pool, &provider, |id| {
            let pool = pool.clone();
            async move {
                apelora_persistence::jobs::request_cancel(&pool, id, "user:alice")
                    .await
                    .expect("cancel");
            }
        })
        .await;
        assert_eq!(get_job(&pool, id).await.expect("job").status, "succeeded");
        assert_eq!(status_of(&provider, "web").await, "Running");

        // Before the instance exists, a create stops.
        job(
            &pool,
            "instance.create",
            json!({ "project": "apelora", "name": "api", "kind": "container",
                    "source": { "protocol": "oci", "server": "https://registry.example", "alias": "nginx:1.29" },
                    "start": true }),
        )
        .await;
        let id = claim_then(&pool, &provider, |id| {
            let pool = pool.clone();
            async move {
                apelora_persistence::jobs::request_cancel(&pool, id, "user:alice")
                    .await
                    .expect("cancel");
            }
        })
        .await;
        assert_eq!(get_job(&pool, id).await.expect("job").status, "cancelled");
        assert_eq!(provider.instance("apelora", "api").await, Ok(None));
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn a_cancelled_operation_finishes_its_running_phase_and_skips_the_rest(pool: Pool) {
        let provider = mock();
        for n in ["web", "db"] {
            create_instance(&pool, &provider, n).await;
        }
        let app = apelora_persistence::create_application(&pool, "shop")
            .await
            .expect("app");
        let op = operation(
            &pool,
            app.id,
            "stop",
            phases(&[("stop", &["web"]), ("stop", &["db"])]),
        )
        .await;
        assert!(
            run_once(&pool, &provider, "w").await.expect("run"),
            "phase 1 queued"
        );
        let coordinator: uuid::Uuid =
            sqlx::query_scalar("SELECT id FROM jobs WHERE kind = 'application.operation'")
                .fetch_one(&pool)
                .await
                .expect("coordinator");
        assert_eq!(
            apelora_persistence::jobs::request_cancel(&pool, coordinator, "user:alice")
                .await
                .expect("cancel"),
            apelora_persistence::jobs::Cancel::Requested,
            "a coordinator always stops through its worker"
        );
        settle(&pool, &provider).await;
        let done = apelora_persistence::lifecycle::get_operation(&pool, op.id)
            .await
            .expect("op")
            .expect("row");
        assert_eq!(done.state, "cancelled");
        assert!(done.finished_at.is_some());
        assert!(
            done.error
                .as_deref()
                .is_some_and(|e| e.contains("after phase 1 (stop)")),
            "{:?}",
            done.error
        );
        let p: Vec<serde_json::Value> = serde_json::from_value(done.phases).expect("phases");
        let states: Vec<&str> = p
            .iter()
            .map(|x| x["state"].as_str().unwrap_or(""))
            .collect();
        assert_eq!(states, ["succeeded", "skipped"]);
        assert_eq!(
            (
                status_of(&provider, "web").await.as_str(),
                status_of(&provider, "db").await.as_str()
            ),
            ("Stopped", "Running"),
            "the started phase finished; the next was never started"
        );
        assert_eq!(
            get_job(&pool, coordinator).await.expect("job").status,
            "cancelled"
        );
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn a_reclaimed_job_waits_on_the_earlier_operation_then_rereads_incus(pool: Pool) {
        let provider = mock();
        let id = job(
            &pool,
            "instance.delete",
            json!({ "project": "apelora", "name": "gone" }),
        )
        .await;
        sqlx::query("UPDATE jobs SET external_operation = '00000000-0000-4000-8000-000000000099' WHERE id = $1")
            .bind(id)
            .execute(&pool)
            .await
            .expect("simulate a crashed attempt");
        drain(&pool, &provider).await;
        let j = get_job(&pool, id).await.expect("job");
        assert_eq!(
            (j.status.as_str(), j.external_operation),
            ("succeeded", None)
        );
    }
    // ---- application operations ------------------------------------------------------------

    async fn create_instance(pool: &Pool, provider: &Provider, name: &str) {
        job(
            pool,
            "instance.create",
            json!({ "project": "apelora", "name": name, "kind": "container",
                    "source": { "protocol": "oci", "server": "https://registry.example", "alias": "nginx:1.29" },
                    "start": true }),
        )
        .await;
        drain(pool, provider).await;
    }

    fn phases(list: &[(&str, &[&str])]) -> serde_json::Value {
        json!(list.iter().map(|(action, names)| json!({ "action": action, "state": "pending",
            "steps": names.iter().map(|n| json!({ "project": "apelora", "name": n, "module": n, "state": "pending" })).collect::<Vec<_>>() })).collect::<Vec<_>>())
    }

    /// Runs the worker until nothing is runnable, skipping the coordinator's waits only when
    /// nothing else is due (as time passing would).
    async fn settle(pool: &Pool, provider: &Provider) {
        settle_with(pool, provider, None).await;
    }

    async fn settle_with(pool: &Pool, provider: &Provider, cap_attempts: Option<i32>) {
        for _ in 0..400 {
            if let Some(n) = cap_attempts {
                sqlx::query("UPDATE jobs SET max_attempts = $1 WHERE kind LIKE 'instance.%'")
                    .bind(n)
                    .execute(pool)
                    .await
                    .expect("cap");
            }
            if run_once(pool, provider, "test-worker").await.expect("run") {
                continue;
            }
            let skipped = sqlx::query(
                "UPDATE jobs SET run_after = now() WHERE status = 'pending' AND run_after > now()",
            )
            .execute(pool)
            .await
            .expect("wait")
            .rows_affected();
            if skipped == 0 {
                break;
            }
        }
    }

    async fn operation(
        pool: &Pool,
        app: uuid::Uuid,
        kind: &str,
        ph: serde_json::Value,
    ) -> apelora_persistence::lifecycle::OperationRow {
        apelora_persistence::lifecycle::create_operation(
            pool,
            &apelora_persistence::lifecycle::NewOperation {
                application_id: app,
                project: "apelora",
                kind,
                phases: &ph,
                requested_by: "tester",
                confirmed: false,
                key: None,
                request_id: Some("req-operation"),
            },
        )
        .await
        .expect("operation")
        .0
    }

    async fn status_of(provider: &Provider, name: &str) -> String {
        provider
            .instance("apelora", name)
            .await
            .expect("read")
            .map(|i| i.status)
            .unwrap_or_default()
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn a_stop_operation_takes_its_phases_in_order_and_succeeds_only_when_incus_confirms(
        pool: Pool,
    ) {
        let provider = mock();
        for n in ["web", "db"] {
            create_instance(&pool, &provider, n).await;
        }
        let app = apelora_persistence::create_application(&pool, "shop")
            .await
            .expect("app");
        let op = operation(
            &pool,
            app.id,
            "stop",
            phases(&[("stop", &["web"]), ("stop", &["db"])]),
        )
        .await;

        // One pass queues the first phase only: the database is not touched until the web tier is confirmed stopped.
        assert!(run_once(&pool, &provider, "w").await.expect("run"));
        let queued: i64 =
            sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind = 'instance.stop'")
                .fetch_one(&pool)
                .await
                .expect("count");
        assert_eq!(queued, 1, "only the first phase is queued");
        let requests: Vec<Option<String>> = sqlx::query_scalar(
            "SELECT request_id FROM jobs WHERE kind IN ('application.operation', 'instance.stop')",
        )
        .fetch_all(&pool)
        .await
        .expect("request ids");
        assert_eq!(
            requests,
            vec![Some("req-operation".to_owned()); 2],
            "the instance jobs carry the request that asked for the operation"
        );
        assert_eq!(
            (
                status_of(&provider, "web").await.as_str(),
                status_of(&provider, "db").await.as_str()
            ),
            ("Running", "Running")
        );

        settle(&pool, &provider).await;
        let done = apelora_persistence::lifecycle::get_operation(&pool, op.id)
            .await
            .expect("op")
            .expect("row");
        assert_eq!((done.state.as_str(), done.error), ("succeeded", None));
        assert!(done.finished_at.is_some() && done.started_at.is_some());
        assert_eq!(
            (
                status_of(&provider, "web").await.as_str(),
                status_of(&provider, "db").await.as_str()
            ),
            ("Stopped", "Stopped")
        );
        let p: Vec<serde_json::Value> = serde_json::from_value(done.phases).expect("phases");
        assert!(p.iter().all(|x| x["state"] == "succeeded"
            && x["steps"][0]["state"] == "succeeded"
            && x["steps"][0]["job"].is_string()));
        // The web stop job was created before the database one.
        let order: Vec<String> = sqlx::query_scalar(
            "SELECT payload->>'name' FROM jobs WHERE kind = 'instance.stop' ORDER BY created_at",
        )
        .fetch_all(&pool)
        .await
        .expect("order");
        assert_eq!(order, ["web", "db"]);
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn start_and_restart_bring_instances_up_providers_first(pool: Pool) {
        let provider = mock();
        for n in ["web", "db"] {
            create_instance(&pool, &provider, n).await;
        }
        let app = apelora_persistence::create_application(&pool, "shop")
            .await
            .expect("app");
        let stop = operation(
            &pool,
            app.id,
            "stop",
            phases(&[("stop", &["web"]), ("stop", &["db"])]),
        )
        .await;
        settle(&pool, &provider).await;
        assert_eq!(
            apelora_persistence::lifecycle::get_operation(&pool, stop.id)
                .await
                .expect("op")
                .expect("row")
                .state,
            "succeeded"
        );

        let start = operation(
            &pool,
            app.id,
            "start",
            phases(&[("start", &["db"]), ("start", &["web"])]),
        )
        .await;
        settle(&pool, &provider).await;
        assert_eq!(
            apelora_persistence::lifecycle::get_operation(&pool, start.id)
                .await
                .expect("op")
                .expect("row")
                .state,
            "succeeded"
        );
        assert_eq!(
            (
                status_of(&provider, "db").await.as_str(),
                status_of(&provider, "web").await.as_str()
            ),
            ("Running", "Running")
        );
        let order: Vec<String> = sqlx::query_scalar(
            "SELECT payload->>'name' FROM jobs WHERE kind = 'instance.start' ORDER BY created_at",
        )
        .fetch_all(&pool)
        .await
        .expect("order");
        assert_eq!(
            order,
            ["db", "web"],
            "the database is up before the web tier"
        );

        let restart = operation(
            &pool,
            app.id,
            "restart",
            phases(&[
                ("stop", &["web"]),
                ("stop", &["db"]),
                ("start", &["db"]),
                ("start", &["web"]),
            ]),
        )
        .await;
        settle(&pool, &provider).await;
        let r = apelora_persistence::lifecycle::get_operation(&pool, restart.id)
            .await
            .expect("op")
            .expect("row");
        assert_eq!(r.state, "succeeded", "{:?}", r.error);
        assert_eq!(
            (
                status_of(&provider, "db").await.as_str(),
                status_of(&provider, "web").await.as_str()
            ),
            ("Running", "Running")
        );
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn a_failure_is_reported_as_what_failed_and_later_phases_are_not_attempted(pool: Pool) {
        let provider = mock();
        create_instance(&pool, &provider, "web").await; // "db" does not exist in Incus
        let app = apelora_persistence::create_application(&pool, "shop")
            .await
            .expect("app");
        let op = operation(
            &pool,
            app.id,
            "stop",
            phases(&[("stop", &["web"]), ("stop", &["db"]), ("stop", &["cache"])]),
        )
        .await;
        settle_with(&pool, &provider, Some(1)).await;
        let done = apelora_persistence::lifecycle::get_operation(&pool, op.id)
            .await
            .expect("op")
            .expect("row");
        assert_eq!(done.state, "failed");
        assert!(
            done.error.as_deref().is_some_and(|e| e.starts_with("db:")),
            "{:?}",
            done.error
        );
        let p: Vec<serde_json::Value> = serde_json::from_value(done.phases).expect("phases");
        let states: Vec<&str> = p
            .iter()
            .map(|x| x["state"].as_str().unwrap_or(""))
            .collect();
        assert_eq!(states, ["succeeded", "failed", "skipped"]);
        assert_eq!(
            status_of(&provider, "web").await,
            "Stopped",
            "the part that worked stays done and is reported"
        );
        let cache_jobs: i64 =
            sqlx::query_scalar("SELECT count(*) FROM jobs WHERE payload->>'name' = 'cache'")
                .fetch_one(&pool)
                .await
                .expect("count");
        assert_eq!(cache_jobs, 0, "nothing was attempted after the failure");
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn only_one_operation_per_application_may_be_active_and_a_key_makes_a_request_safe_to_repeat(
        pool: Pool,
    ) {
        let app = apelora_persistence::create_application(&pool, "shop")
            .await
            .expect("app");
        let ph = phases(&[("stop", &["web"])]);
        let first = apelora_persistence::lifecycle::create_operation(
            &pool,
            &apelora_persistence::lifecycle::NewOperation {
                application_id: app.id,
                project: "apelora",
                kind: "stop",
                phases: &ph,
                requested_by: "t",
                confirmed: false,
                key: Some("k1"),
                request_id: None,
            },
        )
        .await
        .expect("first");
        assert!(first.1);
        let again = apelora_persistence::lifecycle::create_operation(
            &pool,
            &apelora_persistence::lifecycle::NewOperation {
                application_id: app.id,
                project: "apelora",
                kind: "stop",
                phases: &ph,
                requested_by: "t",
                confirmed: false,
                key: Some("k1"),
                request_id: None,
            },
        )
        .await
        .expect("same key");
        assert_eq!(
            (again.0.id, again.1),
            (first.0.id, false),
            "a repeated request returns the same operation"
        );
        let other = apelora_persistence::lifecycle::create_operation(
            &pool,
            &apelora_persistence::lifecycle::NewOperation {
                application_id: app.id,
                project: "apelora",
                kind: "start",
                phases: &ph,
                requested_by: "t",
                confirmed: false,
                key: None,
                request_id: None,
            },
        )
        .await;
        assert!(matches!(other, Err(StoreError::Conflict(_))), "{other:?}");
        let wrong = apelora_persistence::lifecycle::create_operation(
            &pool,
            &apelora_persistence::lifecycle::NewOperation {
                application_id: app.id,
                project: "apelora",
                kind: "start",
                phases: &ph,
                requested_by: "t",
                confirmed: false,
                key: Some("k1"),
                request_id: None,
            },
        )
        .await;
        assert!(
            matches!(wrong, Err(StoreError::Conflict(_))),
            "the key belongs to a different request"
        );
        let jobs: i64 =
            sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind = 'application.operation'")
                .fetch_one(&pool)
                .await
                .expect("count");
        assert_eq!(jobs, 1);
    }
}

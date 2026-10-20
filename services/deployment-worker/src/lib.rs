// SPDX-License-Identifier: AGPL-3.0-only
//! The deployment worker's job loop: claim a job with a lease, run its handler, record the
//! outcome fenced by the claim's generation. Instance lifecycle jobs change Incus through the
//! [`Provider`]; see [`lifecycle`].

pub mod lifecycle;
pub mod network;
pub mod supervisor;

use std::time::Duration;

use apelora_incus_adapter::Provider;
use apelora_persistence::{
    ClaimedJob, Outcome, Pool, StoreError, claim_job, complete_job, renew_lease,
};

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

/// Claims and runs at most one job. Returns `Ok(false)` when nothing was runnable.
pub async fn run_once(pool: &Pool, provider: &Provider, worker: &str) -> Result<bool, StoreError> {
    let Some(job) = claim_job(pool, worker, LEASE_SECS).await? else {
        return Ok(false);
    };
    tracing::info!(job = %job.id, kind = %job.kind, generation = job.generation, attempt = job.attempts, "claimed job");
    let renew = {
        let (pool, job, worker) = (pool.clone(), job.clone(), worker.to_owned());
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs((LEASE_SECS / 3) as u64)).await;
                if let Err(e) = renew_lease(&pool, &job, &worker, LEASE_SECS).await {
                    tracing::warn!(job = %job.id, error = %e, "lost the lease; stopping renewals");
                    return;
                }
            }
        })
    };
    let outcome = handle(pool, provider, &job, worker).await;
    renew.abort();
    let outcome = match outcome {
        Ok(o) => o,
        // Lost the job to another worker mid-way: its attempt owns the result now.
        Err(StoreError::Conflict(msg)) => {
            tracing::warn!(job = %job.id, %msg, "stopped working on the job (fenced)");
            return Ok(true);
        }
        Err(e) => return Err(e),
    };
    match complete_job(pool, &job, worker, &outcome).await {
        Ok(()) => tracing::info!(job = %job.id, ?outcome, "job finished"),
        // Another worker took over after our lease expired; its result wins.
        Err(StoreError::Conflict(msg)) => {
            tracing::warn!(job = %job.id, %msg, "result discarded (fenced)")
        }
        Err(e) => return Err(e),
    }
    Ok(true)
}

/// Processes jobs until `shutdown` resolves, polling every `idle` when there is no work.
pub async fn run(
    pool: Pool,
    provider: Provider,
    worker: String,
    idle: Duration,
    shutdown: impl std::future::Future<Output = ()>,
) {
    tokio::pin!(shutdown);
    loop {
        let worked = match run_once(&pool, &provider, &worker).await {
            Ok(w) => w,
            Err(e) => {
                tracing::error!(error = %e, "job loop error; backing off");
                false
            }
        };
        if !worked {
            tokio::select! {
                _ = &mut shutdown => break,
                _ = tokio::time::sleep(idle) => {}
            }
        }
    }
    tracing::info!("worker stopped");
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
        let handle = tokio::spawn(run(
            pool,
            mock(),
            "test-worker".into(),
            Duration::from_millis(20),
            tokio::time::sleep(Duration::from_millis(100)),
        ));
        tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("stops in time")
            .expect("join");
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
}

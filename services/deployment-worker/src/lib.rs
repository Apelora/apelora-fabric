// SPDX-License-Identifier: AGPL-3.0-only
//! The deployment worker's job loop: claim a job with a lease, run its handler, record the
//! outcome fenced by the claim's generation. Read-only for now: no handler changes Incus yet.

use std::time::Duration;

use apelora_persistence::{
    ClaimedJob, Outcome, Pool, StoreError, claim_job, complete_job, renew_lease,
};

/// Lease length; renewed at a third of this while a handler runs.
pub const LEASE_SECS: i64 = 30;

/// Runs the handler for one job kind. Unknown kinds fail (and are retried, then marked failed),
/// so a newer API cannot silently enqueue work an older worker ignores.
async fn handle(job: &ClaimedJob) -> Outcome {
    match job.kind.as_str() {
        "noop" => Outcome::Succeeded,
        other => Outcome::Failed(format!("this worker has no handler for job kind '{other}'")),
    }
}

/// Claims and runs at most one job. Returns `Ok(false)` when nothing was runnable.
pub async fn run_once(pool: &Pool, worker: &str) -> Result<bool, StoreError> {
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
    let outcome = handle(&job).await;
    renew.abort();
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
    worker: String,
    idle: Duration,
    shutdown: impl std::future::Future<Output = ()>,
) {
    tokio::pin!(shutdown);
    loop {
        let worked = match run_once(&pool, &worker).await {
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
    use apelora_persistence::{enqueue_job, job_status};
    use serde_json::json;

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
        while run_once(&pool, "test-worker").await.expect("run") {}
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
            "test-worker".into(),
            Duration::from_millis(20),
            tokio::time::sleep(Duration::from_millis(100)),
        ));
        tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("stops in time")
            .expect("join");
    }
}

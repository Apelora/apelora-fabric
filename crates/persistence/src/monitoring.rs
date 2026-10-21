// SPDX-License-Identifier: AGPL-3.0-only
//! Reads for the metrics endpoints (`docs/architecture/MONITORING.md`): the connection pool's
//! state and a few aggregate queries over the job queue and backups. They run when a collector
//! scrapes, at most every 15 seconds per process, never per API request.

use std::time::{Duration, Instant};

use crate::{Pool, Result};

/// This process's connection pool, sampled now.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PoolSample {
    pub in_use: u32,
    pub idle: u32,
    pub max: u32,
    /// Time to get a connection; `None` when none came within `limit`.
    pub acquire_secs: Option<f64>,
}

/// Counts the pool's connections, then times getting one (given back at once).
pub async fn pool_sample(pool: &Pool, limit: Duration) -> PoolSample {
    let size = pool.size();
    let idle = u32::try_from(pool.num_idle()).unwrap_or(u32::MAX).min(size);
    let max = pool.options().get_max_connections();
    let started = Instant::now();
    let acquire_secs = match tokio::time::timeout(limit, pool.acquire()).await {
        Ok(Ok(conn)) => {
            drop(conn);
            Some(started.elapsed().as_secs_f64())
        }
        _ => None,
    };
    PoolSample {
        in_use: size - idle,
        idle,
        max,
        acquire_secs,
    }
}

/// The job queue in aggregate.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct QueueSummary {
    /// (kind, status, count) for pending and running jobs.
    pub by_kind: Vec<(String, String, i64)>,
    /// Seconds the oldest runnable pending job has waited (0 when none).
    pub oldest_runnable_secs: f64,
    /// Running jobs whose lease has expired.
    pub expired_leases: i64,
}

pub async fn queue_summary(pool: &Pool) -> Result<QueueSummary> {
    let by_kind: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT kind, status, count(*) FROM jobs
         WHERE status IN ('pending', 'running') GROUP BY kind, status ORDER BY kind, status",
    )
    .fetch_all(pool)
    .await?;
    let (oldest_runnable_secs, expired_leases): (f64, i64) = sqlx::query_as(
        "SELECT
           coalesce((SELECT extract(epoch FROM now() - min(run_after))::float8 FROM jobs
                     WHERE status = 'pending' AND run_after <= now()), 0),
           (SELECT count(*) FROM jobs WHERE status = 'running' AND lease_until < now())",
    )
    .fetch_one(pool)
    .await?;
    Ok(QueueSummary {
        by_kind,
        oldest_runnable_secs: oldest_runnable_secs.max(0.0),
        expired_leases,
    })
}

/// The newest backup per backup pool: (pool, Unix seconds).
pub async fn last_backups(pool: &Pool) -> Result<Vec<(String, f64)>> {
    Ok(sqlx::query_as(
        "SELECT storage_pool, extract(epoch FROM max(created_at))::float8
         FROM volume_backups GROUP BY storage_pool ORDER BY storage_pool",
    )
    .fetch_all(pool)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn the_queue_summary_counts_waiting_work_and_expired_leases(pool: Pool) {
        let empty = queue_summary(&pool).await.expect("summary");
        assert_eq!(empty, QueueSummary::default());
        crate::enqueue_job(&pool, "noop", None, &serde_json::json!({}), None)
            .await
            .expect("enqueue");
        crate::enqueue_job(&pool, "noop", None, &serde_json::json!({}), None)
            .await
            .expect("enqueue");
        // One is claimed with a lease that has already run out.
        crate::claim_job(&pool, "w1", -1).await.expect("claim");
        let s = queue_summary(&pool).await.expect("summary");
        assert_eq!(
            s.by_kind,
            vec![
                ("noop".to_owned(), "pending".to_owned(), 1),
                ("noop".to_owned(), "running".to_owned(), 1)
            ]
        );
        assert_eq!(s.expired_leases, 1);
        assert!(s.oldest_runnable_secs >= 0.0);
        assert!(last_backups(&pool).await.expect("backups").is_empty());
        let sample = pool_sample(&pool, Duration::from_secs(2)).await;
        assert!(sample.acquire_secs.is_some());
        assert!(sample.max >= 1 && sample.in_use + sample.idle <= sample.max);
    }
}

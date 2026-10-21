// SPDX-License-Identifier: AGPL-3.0-only
//! What the Health page reads from PostgreSQL: worker heartbeats, the job queue's counts and
//! the database's own round trip and schema level (`docs/design/health-and-capacity.md`).

use sqlx::{FromRow, Row};
use time::OffsetDateTime;

use crate::{MIGRATOR, Pool, Result};

/// Registers a worker that has just started (its start time resets) and forgets workers not
/// seen for a day.
pub async fn worker_started(pool: &Pool, id: &str, host: &str, version: &str) -> Result<()> {
    sqlx::query("DELETE FROM worker_heartbeats WHERE last_seen < now() - interval '1 day'")
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO worker_heartbeats (id, host, version) VALUES ($1, $2, $3)
         ON CONFLICT (id) DO UPDATE SET host = EXCLUDED.host, version = EXCLUDED.version,
           started_at = now(), last_seen = now(), stopped_at = NULL",
    )
    .bind(id)
    .bind(host)
    .bind(version)
    .execute(pool)
    .await?;
    Ok(())
}

/// The worker is alive. Recreates its row if someone deleted it.
pub async fn worker_heartbeat(pool: &Pool, id: &str, host: &str, version: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO worker_heartbeats (id, host, version) VALUES ($1, $2, $3)
         ON CONFLICT (id) DO UPDATE SET last_seen = now(), stopped_at = NULL",
    )
    .bind(id)
    .bind(host)
    .bind(version)
    .execute(pool)
    .await?;
    Ok(())
}

/// The worker is shutting down cleanly.
pub async fn worker_stopped(pool: &Pool, id: &str) -> Result<()> {
    sqlx::query("UPDATE worker_heartbeats SET stopped_at = now() WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, FromRow)]
pub struct WorkerRecord {
    pub id: String,
    pub host: String,
    pub version: String,
    pub started_at: OffsetDateTime,
    pub last_seen: OffsetDateTime,
    pub stopped_at: Option<OffsetDateTime>,
    /// Seconds since `last_seen`, by the database's clock (no clock skew with the caller).
    pub seconds_since_seen: i64,
}

/// Workers seen in the last day, most recently seen first.
pub async fn workers(pool: &Pool) -> Result<Vec<WorkerRecord>> {
    Ok(sqlx::query_as::<_, WorkerRecord>(
        "SELECT id, host, version, started_at, last_seen, stopped_at,
                floor(extract(epoch FROM now() - last_seen))::bigint AS seconds_since_seen
         FROM worker_heartbeats
         WHERE last_seen > now() - interval '1 day'
         ORDER BY last_seen DESC, id",
    )
    .fetch_all(pool)
    .await?)
}

/// The job queue's counts (the Health page's rules are `apelora_domain::capacity::queue_state`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QueueCounts {
    /// Ready to run, waiting for a worker.
    pub waiting: i64,
    /// Waiting for a retry or a scheduled time.
    pub retrying: i64,
    pub running: i64,
    /// Running with an expired lease (its worker died or hangs).
    pub stalled: i64,
    pub failed_last_hour: i64,
    /// Age of the oldest job ready to run.
    pub oldest_waiting_secs: Option<i64>,
}

/// The job queue's counts.
pub async fn queue_counts(pool: &Pool) -> Result<QueueCounts> {
    let row = sqlx::query(
        "SELECT
           count(*) FILTER (WHERE status = 'pending' AND run_after <= now()) AS waiting,
           count(*) FILTER (WHERE status = 'pending' AND run_after > now()) AS retrying,
           count(*) FILTER (WHERE status = 'running' AND lease_until >= now()) AS running,
           count(*) FILTER (WHERE status = 'running' AND (lease_until IS NULL OR lease_until < now())) AS stalled,
           count(*) FILTER (WHERE status = 'failed' AND updated_at > now() - interval '1 hour') AS failed_last_hour,
           floor(extract(epoch FROM now() - min(run_after) FILTER (WHERE status = 'pending' AND run_after <= now())))::bigint AS oldest
         FROM jobs
         WHERE status IN ('pending', 'running')
            OR (status = 'failed' AND updated_at > now() - interval '1 hour')",
    )
    .fetch_one(pool)
    .await?;
    Ok(QueueCounts {
        waiting: row.try_get("waiting")?,
        retrying: row.try_get("retrying")?,
        running: row.try_get("running")?,
        stalled: row.try_get("stalled")?,
        failed_last_hour: row.try_get("failed_last_hour")?,
        oldest_waiting_secs: row.try_get("oldest")?,
    })
}

/// The database's round trip and schema level.
#[derive(Debug, Clone, PartialEq)]
pub struct DatabaseCheck {
    pub round_trip_ms: f64,
    /// The newest migration applied successfully; `None` on an empty database.
    pub schema_version: Option<i64>,
    /// The newest migration this build carries.
    pub expected_version: i64,
}

pub async fn database_check(pool: &Pool) -> Result<DatabaseCheck> {
    let started = std::time::Instant::now();
    sqlx::query("SELECT 1").execute(pool).await?;
    let round_trip_ms = started.elapsed().as_secs_f64() * 1000.0;
    let schema_version: Option<i64> =
        sqlx::query_scalar("SELECT max(version) FROM _sqlx_migrations WHERE success")
            .fetch_one(pool)
            .await?;
    Ok(DatabaseCheck {
        round_trip_ms,
        schema_version,
        expected_version: expected_schema_version(),
    })
}

/// The newest migration embedded in this build.
pub fn expected_schema_version() -> i64 {
    MIGRATOR.iter().map(|m| m.version).max().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{claim_job, enqueue_job};
    use serde_json::json;

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn heartbeats_track_start_beat_and_stop(pool: Pool) {
        worker_started(&pool, "w1", "host-a", "0.0.0")
            .await
            .expect("start");
        let w = workers(&pool).await.expect("list");
        assert_eq!(w.len(), 1);
        assert_eq!((w[0].host.as_str(), w[0].stopped_at), ("host-a", None));
        assert!(w[0].seconds_since_seen <= 1);
        worker_stopped(&pool, "w1").await.expect("stop");
        assert!(workers(&pool).await.expect("list")[0].stopped_at.is_some());
        // A heartbeat clears the stop (the worker is evidently running again).
        worker_heartbeat(&pool, "w1", "host-a", "0.0.0")
            .await
            .expect("beat");
        assert_eq!(workers(&pool).await.expect("list")[0].stopped_at, None);
        // Workers silent for a day disappear when another starts.
        sqlx::query(
            "UPDATE worker_heartbeats SET last_seen = now() - interval '2 days' WHERE id = 'w1'",
        )
        .execute(&pool)
        .await
        .expect("age");
        assert!(workers(&pool).await.expect("list").is_empty());
        worker_started(&pool, "w2", "host-b", "0.0.0")
            .await
            .expect("start");
        let ids: Vec<String> = sqlx::query_scalar("SELECT id FROM worker_heartbeats")
            .fetch_all(&pool)
            .await
            .expect("ids");
        assert_eq!(ids, ["w2"]);
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn queue_counts_by_state(pool: Pool) {
        assert_eq!(
            queue_counts(&pool).await.expect("empty"),
            Default::default()
        );
        for _ in 0..3 {
            enqueue_job(&pool, "noop", None, &json!({}), None)
                .await
                .expect("enqueue");
        }
        sqlx::query("UPDATE jobs SET created_at = now() - interval '5 minutes', run_after = now() - interval '5 minutes'")
            .execute(&pool)
            .await
            .expect("age");
        let claimed = claim_job(&pool, "w", 60)
            .await
            .expect("claim")
            .expect("job");
        // One waits for a retry, one runs with a lease that expired, one failed recently.
        let another = enqueue_job(&pool, "noop", None, &json!({}), None)
            .await
            .expect("enqueue")
            .id();
        sqlx::query("UPDATE jobs SET run_after = now() + interval '1 minute' WHERE id = $1")
            .bind(another)
            .execute(&pool)
            .await
            .expect("retry");
        let failed = enqueue_job(&pool, "noop", None, &json!({}), None)
            .await
            .expect("enqueue")
            .id();
        sqlx::query("UPDATE jobs SET status = 'failed', updated_at = now() WHERE id = $1")
            .bind(failed)
            .execute(&pool)
            .await
            .expect("fail");
        let q = queue_counts(&pool).await.expect("counts");
        assert_eq!(
            (
                q.waiting,
                q.retrying,
                q.running,
                q.stalled,
                q.failed_last_hour
            ),
            (2, 1, 1, 0, 1)
        );
        assert!(q.oldest_waiting_secs.expect("oldest") >= 299);
        sqlx::query("UPDATE jobs SET lease_until = now() - interval '1 second' WHERE id = $1")
            .bind(claimed.id)
            .execute(&pool)
            .await
            .expect("expire");
        let q = queue_counts(&pool).await.expect("counts");
        assert_eq!((q.running, q.stalled), (0, 1));
        // Old failures are history.
        sqlx::query("UPDATE jobs SET updated_at = now() - interval '2 hours' WHERE id = $1")
            .bind(failed)
            .execute(&pool)
            .await
            .expect("old");
        assert_eq!(
            queue_counts(&pool).await.expect("counts").failed_last_hour,
            0
        );
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn database_check_reports_the_schema(pool: Pool) {
        let c = database_check(&pool).await.expect("check");
        assert_eq!(c.schema_version, Some(c.expected_version));
        assert!(c.expected_version >= 16);
        assert!(c.round_trip_ms >= 0.0);
    }
}

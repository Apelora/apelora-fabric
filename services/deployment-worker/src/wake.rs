// SPDX-License-Identifier: AGPL-3.0-only
//! Job wake-up (ADR 0011, EVERY-SIZE.md §2.4): the worker LISTENs on `apelora_jobs`, which every
//! committed job insert notifies (trigger `jobs_queued`), and claims at once instead of waiting
//! for its next periodic check. The periodic check stays: it is the fallback when the listener
//! is down and the safety net for a notification that was missed (one sent while the listener
//! was reconnecting is lost, so after a reconnect the worker checks once at once).

use std::time::Duration;

use apelora_persistence::{PgListener, Pool, listen_for_jobs};

/// Waits for queued work: a notification, or the idle interval, whichever comes first.
pub struct JobWake {
    pool: Pool,
    listener: Option<PgListener>,
}

impl JobWake {
    /// Subscribes at once, so a job queued after the first check is not missed. When the
    /// listener cannot be set up the worker polls and keeps trying to subscribe.
    pub async fn listen(pool: Pool) -> Self {
        let listener = match listen_for_jobs(&pool).await {
            Ok(l) => {
                tracing::debug!("job wake-up: listening for queued jobs");
                Some(l)
            }
            Err(e) => {
                tracing::warn!(error = %e, "job wake-up: cannot listen for queued jobs; checking for work on the idle interval until it can");
                None
            }
        };
        JobWake { pool, listener }
    }

    /// Whether the listener is subscribed now.
    pub fn listening(&self) -> bool {
        self.listener.is_some()
    }

    /// Returns when a job was queued, the listener reconnected (work may have been missed), or
    /// `idle` passed. Never fails: a broken listener is dropped and set up again later.
    pub async fn wait(&mut self, idle: Duration) {
        let Some(listener) = self.listener.as_mut() else {
            tokio::time::sleep(idle).await;
            match listen_for_jobs(&self.pool).await {
                Ok(l) => {
                    tracing::info!("job wake-up: listening for queued jobs again");
                    self.listener = Some(l);
                }
                Err(e) => {
                    tracing::debug!(error = %e, "job wake-up: still cannot listen; checking on the idle interval")
                }
            }
            return;
        };
        match tokio::time::timeout(idle, listener.try_recv()).await {
            // Idle interval passed: the periodic check.
            Err(_) => {}
            Ok(Ok(Some(_))) => {
                // Several jobs queued while this worker was busy: one check covers them all.
                while listener.next_buffered().is_some() {}
            }
            // The connection dropped and was re-established: notifications sent meanwhile are
            // lost, so check now.
            Ok(Ok(None)) => {
                tracing::warn!(
                    "job wake-up: the listener's connection dropped and was re-established; checking for work now"
                );
            }
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "job wake-up: the listener failed; checking for work on the idle interval until it is back");
                self.listener = None;
                // Try once at once: usually the database is there and only the connection died.
                if let Ok(l) = listen_for_jobs(&self.pool).await {
                    tracing::info!("job wake-up: listening for queued jobs again");
                    self.listener = Some(l);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use apelora_incus_adapter::{MockIncus, Provider};
    use apelora_persistence::{enqueue_job, job_status};
    use serde_json::json;

    use super::*;

    /// Far longer than any wait below: a job picked up quickly was woken, not polled.
    const IDLE: Duration = Duration::from_secs(30);

    /// The backends of this test's database that are listening for jobs.
    async fn listeners(pool: &Pool) -> Vec<i32> {
        sqlx::query_scalar(
            "SELECT pid FROM pg_stat_activity
             WHERE datname = current_database() AND query LIKE 'LISTEN%' ORDER BY pid",
        )
        .fetch_all(pool)
        .await
        .expect("pg_stat_activity")
    }

    async fn until<F: std::future::Future<Output = bool>>(what: &str, mut f: impl FnMut() -> F) {
        let started = Instant::now();
        while !f().await {
            assert!(started.elapsed() < Duration::from_secs(10), "{what}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Starts a worker that checks for work only every [`IDLE`] unless woken; stops it when the
    /// returned sender is dropped.
    async fn worker(pool: &Pool) -> tokio::sync::oneshot::Sender<()> {
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(crate::run(
            pool.clone(),
            Provider::Mock(MockIncus::default()),
            "test-worker".into(),
            IDLE,
            async {
                let _ = stopped.await;
            },
        ));
        until("the worker listens", || async {
            !listeners(pool).await.is_empty()
        })
        .await;
        stop
    }

    /// Queues a no-op job and returns how long the worker took to finish it.
    async fn picked_up_after(pool: &Pool) -> Duration {
        let started = Instant::now();
        let id = enqueue_job(pool, "noop", None, &json!({}), None)
            .await
            .expect("enqueue")
            .id();
        until("the job is picked up", || async {
            job_status(pool, id).await.expect("status").0 == "succeeded"
        })
        .await;
        started.elapsed()
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn a_queued_job_is_picked_up_well_under_the_idle_interval(pool: Pool) {
        let _stop = worker(&pool).await;
        // Let the worker finish its first (empty) check and start waiting.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let took = picked_up_after(&pool).await;
        assert!(took < Duration::from_secs(2), "woken, not polled: {took:?}");
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn the_listener_comes_back_after_its_connection_is_dropped(pool: Pool) {
        let _stop = worker(&pool).await;
        let before = listeners(&pool).await;
        sqlx::query("SELECT pg_terminate_backend(pid) FROM unnest($1::int[]) AS pid")
            .bind(&before)
            .execute(&pool)
            .await
            .expect("terminate the listener's connection");
        until("the worker listens again on a new connection", || async {
            let now = listeners(&pool).await;
            !now.is_empty() && now.iter().all(|p| !before.contains(p))
        })
        .await;
        let took = picked_up_after(&pool).await;
        assert!(
            took < Duration::from_secs(2),
            "woken again after the reconnect: {took:?}"
        );
    }
}

// SPDX-License-Identifier: AGPL-3.0-only
//! Coordination between Apelora processes through PostgreSQL (ADR 0011): leader leases for work
//! only one process may do at a time, and the wake-up channel that tells workers a job was
//! queued.
//!
//! A **leader lease** is one row per role (`leader_leases`). A process takes it when it is free
//! or expired, renews it every few seconds, and loses it when it stops renewing. Every new
//! holder gets a higher `generation`. A leader does each write inside [`begin_fenced`], which
//! first checks, and locks for the rest of the transaction, that the lease still names this
//! holder and generation and has not expired: a leader that stalled past its lease (a long
//! pause, a lost network) cannot write after another process took over. This is the same
//! fencing rule as job leases ([`crate::renew_lease`], [`crate::complete_job`]).

use sqlx::postgres::{PgListener, PgPool};
use sqlx::{Postgres, Transaction};

use crate::{Result, StoreError};

/// The notification channel a job insert signals (trigger `jobs_queued`, migration 0021).
pub const JOBS_CHANNEL: &str = "apelora_jobs";

/// A leader lease this process holds. Every write made as the leader quotes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leadership {
    /// The role (`leader_leases.name`), e.g. `replica-supervisor`.
    pub name: String,
    pub holder: String,
    pub generation: i64,
}

/// Takes the lease `name` for `holder` for `ttl_secs` if nobody holds it or the holder let it
/// expire. `None` while another holder's lease is valid. The holder of a valid lease does not
/// get it again here: it renews it with [`renew_leadership`].
pub async fn acquire_leadership(
    pool: &PgPool,
    name: &str,
    holder: &str,
    ttl_secs: f64,
) -> Result<Option<Leadership>> {
    let generation = sqlx::query_scalar::<_, i64>(
        "INSERT INTO leader_leases (name, holder, generation, expires_at)
         VALUES ($1, $2, 1, now() + make_interval(secs => $3))
         ON CONFLICT (name) DO UPDATE
            SET holder = EXCLUDED.holder, generation = leader_leases.generation + 1,
                expires_at = EXCLUDED.expires_at, acquired_at = now()
            WHERE leader_leases.expires_at <= now()
         RETURNING generation",
    )
    .bind(name)
    .bind(holder)
    .bind(ttl_secs)
    .fetch_optional(pool)
    .await?;
    Ok(generation.map(|generation| Leadership {
        name: name.to_owned(),
        holder: holder.to_owned(),
        generation,
    }))
}

/// Extends a held lease by `ttl_secs` from now. `Conflict` when it expired or another holder
/// took it: the caller is no longer the leader and must stop acting as one.
pub async fn renew_leadership(pool: &PgPool, l: &Leadership, ttl_secs: f64) -> Result<()> {
    let n = sqlx::query(
        "UPDATE leader_leases SET expires_at = now() + make_interval(secs => $4)
         WHERE name = $1 AND holder = $2 AND generation = $3 AND expires_at > now()",
    )
    .bind(&l.name)
    .bind(&l.holder)
    .bind(l.generation)
    .bind(ttl_secs)
    .execute(pool)
    .await?
    .rows_affected();
    held(n, l)
}

/// Gives the lease up at once (a clean shutdown), so another process need not wait for it to
/// expire. Does nothing if it was already lost.
pub async fn release_leadership(pool: &PgPool, l: &Leadership) -> Result<()> {
    sqlx::query(
        "UPDATE leader_leases SET expires_at = now()
         WHERE name = $1 AND holder = $2 AND generation = $3",
    )
    .bind(&l.name)
    .bind(&l.holder)
    .bind(l.generation)
    .execute(pool)
    .await?;
    Ok(())
}

/// The current holder of the lease `name`, its generation and whether the lease is still valid.
pub async fn leader_of(pool: &PgPool, name: &str) -> Result<Option<(String, i64, bool)>> {
    Ok(sqlx::query_as::<_, (String, i64, bool)>(
        "SELECT holder, generation, expires_at > now() FROM leader_leases WHERE name = $1",
    )
    .bind(name)
    .fetch_optional(pool)
    .await?)
}

/// Begins a transaction for a write made as the leader. With `Some(lease)` it first checks that
/// the lease is still held at that generation and unexpired, and keeps the row locked (`FOR
/// SHARE`) until the transaction ends, so no other process can take the lease between the
/// check and the commit. `Conflict` when the lease is no longer held. With `None` (one-shot
/// passes and tests, where no lease is in play) it is a plain transaction.
pub async fn begin_fenced(
    pool: &PgPool,
    lease: Option<&Leadership>,
) -> Result<Transaction<'static, Postgres>> {
    let mut tx = pool.begin().await?;
    if let Some(l) = lease {
        let still = sqlx::query_scalar::<_, i32>(
            "SELECT 1 FROM leader_leases
             WHERE name = $1 AND holder = $2 AND generation = $3 AND expires_at > now()
             FOR SHARE",
        )
        .bind(&l.name)
        .bind(&l.holder)
        .bind(l.generation)
        .fetch_optional(&mut *tx)
        .await?;
        held(u64::from(still.is_some()), l)?;
    }
    Ok(tx)
}

fn held(rows: u64, l: &Leadership) -> Result<()> {
    if rows == 1 {
        Ok(())
    } else {
        Err(StoreError::Conflict(format!(
            "{} lease generation {} is no longer held by {}",
            l.name, l.generation, l.holder
        )))
    }
}

/// A listener subscribed to [`JOBS_CHANNEL`], on its own connection from `pool`. It is woken
/// when a transaction that queued jobs commits.
pub async fn listen_for_jobs(pool: &PgPool) -> Result<PgListener> {
    let mut l = PgListener::connect_with(pool).await?;
    l.listen(JOBS_CHANNEL).await?;
    Ok(l)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use serde_json::json;

    use super::*;
    use crate::{
        claim_job, enqueue_job, enqueue_job_for_request, enqueue_job_in, get_job, record_operation,
    };

    const NAME: &str = "replica-supervisor";

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn one_holder_at_a_time(pool: PgPool) {
        let a = acquire_leadership(&pool, NAME, "worker-a", 30.0)
            .await
            .expect("acquire")
            .expect("free lease is taken");
        assert_eq!(a.generation, 1);
        assert_eq!(
            acquire_leadership(&pool, NAME, "worker-b", 30.0)
                .await
                .expect("acquire"),
            None,
            "a valid lease is not taken"
        );
        assert_eq!(
            acquire_leadership(&pool, NAME, "worker-a", 30.0)
                .await
                .expect("acquire"),
            None,
            "the holder renews; it does not take its own lease again"
        );
        renew_leadership(&pool, &a, 30.0).await.expect("renew");
        assert_eq!(
            leader_of(&pool, NAME).await.expect("read"),
            Some(("worker-a".into(), 1, true))
        );
        // Another role is another lease.
        assert!(
            acquire_leadership(&pool, "other-role", "worker-b", 30.0)
                .await
                .expect("acquire")
                .is_some()
        );
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn an_expired_lease_is_taken_over_and_the_old_holder_is_fenced(pool: PgPool) {
        let a = acquire_leadership(&pool, NAME, "worker-a", 1.0)
            .await
            .expect("acquire")
            .expect("taken");
        let started = Instant::now();
        // worker-a stops renewing; worker-b keeps trying.
        let b = loop {
            if let Some(b) = acquire_leadership(&pool, NAME, "worker-b", 30.0)
                .await
                .expect("acquire")
            {
                break b;
            }
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "never taken over"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        assert!(
            started.elapsed() >= Duration::from_millis(800),
            "not before the lease expired ({:?})",
            started.elapsed()
        );
        assert_eq!(b.generation, a.generation + 1);
        // The stale holder can neither renew nor write.
        assert!(matches!(
            renew_leadership(&pool, &a, 30.0).await,
            Err(StoreError::Conflict(_))
        ));
        assert!(matches!(
            begin_fenced(&pool, Some(&a)).await,
            Err(StoreError::Conflict(_))
        ));
        // The new one can; releasing hands the lease over at once.
        begin_fenced(&pool, Some(&b))
            .await
            .expect("fenced")
            .commit()
            .await
            .expect("commit");
        release_leadership(&pool, &b).await.expect("release");
        let c = acquire_leadership(&pool, NAME, "worker-c", 30.0)
            .await
            .expect("acquire")
            .expect("released lease is free");
        assert_eq!(c.generation, b.generation + 1);
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn a_fenced_write_holds_off_a_takeover_until_it_commits(pool: PgPool) {
        let a = acquire_leadership(&pool, NAME, "worker-a", 1.0)
            .await
            .expect("acquire")
            .expect("taken");
        let mut tx = begin_fenced(&pool, Some(&a)).await.expect("fenced");
        enqueue_job_in(&mut tx, "noop", None, &json!({}), None, None)
            .await
            .expect("enqueue in the leader's transaction");
        tokio::time::sleep(Duration::from_millis(1100)).await;
        // Expired, but the leader's transaction still holds the row: the takeover waits.
        let takeover = tokio::spawn({
            let pool = pool.clone();
            async move { acquire_leadership(&pool, NAME, "worker-b", 30.0).await }
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!takeover.is_finished(), "takeover waits for the commit");
        tx.commit().await.expect("commit");
        let b = takeover
            .await
            .expect("join")
            .expect("acquire")
            .expect("taken after the commit");
        assert_eq!(b.generation, 2);
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn a_job_keeps_its_request_id_and_every_operation_it_started(pool: PgPool) {
        let id = enqueue_job_for_request(
            &pool,
            "instance.start",
            None,
            &json!({ "project": "apelora", "name": "web-01" }),
            None,
            Some("req-1"),
        )
        .await
        .expect("enqueue")
        .id();
        let job = claim_job(&pool, "worker-a", 30)
            .await
            .expect("claim")
            .expect("job");
        assert_eq!(job.request_id.as_deref(), Some("req-1"));
        record_operation(&pool, &job, "worker-a", Some("op-1"))
            .await
            .expect("record");
        record_operation(&pool, &job, "worker-a", None)
            .await
            .expect("clear");
        record_operation(&pool, &job, "worker-a", Some("op-2"))
            .await
            .expect("record");
        let view = get_job(&pool, id).await.expect("view");
        assert_eq!(view.request_id.as_deref(), Some("req-1"));
        assert_eq!(view.external_operation.as_deref(), Some("op-2"));
        assert_eq!(view.incus_operations, vec!["op-1", "op-2"]);
        // Without a request (the supervisor, the scheduler): none.
        let other = enqueue_job(&pool, "noop", None, &json!({}), None)
            .await
            .expect("enqueue")
            .id();
        assert_eq!(get_job(&pool, other).await.expect("view").request_id, None);
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn queuing_a_job_notifies_listeners_at_commit_only(pool: PgPool) {
        let mut listener = listen_for_jobs(&pool).await.expect("listen");
        let mut tx = pool.begin().await.expect("begin");
        enqueue_job_in(&mut tx, "noop", None, &json!({}), None, None)
            .await
            .expect("enqueue");
        tx.rollback().await.expect("rollback");
        assert!(
            tokio::time::timeout(Duration::from_millis(300), listener.recv())
                .await
                .is_err(),
            "a rolled-back insert notifies nobody"
        );
        enqueue_job(&pool, "noop", None, &json!({}), None)
            .await
            .expect("enqueue");
        let n = tokio::time::timeout(Duration::from_secs(2), listener.recv())
            .await
            .expect("notified")
            .expect("notification");
        assert_eq!(n.channel(), JOBS_CHANNEL);
    }
}

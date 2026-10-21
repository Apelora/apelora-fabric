// SPDX-License-Identifier: AGPL-3.0-only
//! The transactional outbox (ADR 0011, EVERY-SIZE.md §2.5).
//!
//! A change writes the messages it must cause (a notification, an alert, a copy for another
//! plane) into `outbox` **in its own transaction**, so a message exists exactly when the change
//! committed. A relay in the worker, holding the `outbox-relay` leader lease, reads due messages
//! oldest first, hands them to a sink and marks them delivered; a failed delivery is retried
//! with backoff. Delivery is at least once: a relay that stops between delivering and marking
//! delivers again, so receivers ignore duplicates by `id`.
//!
//! Every audit row writes one message with the topic [`AUDIT_TOPIC`] (see
//! [`crate::identity::audit_event`]).

use serde_json::Value;
use sqlx::postgres::PgPool;
use sqlx::{FromRow, PgExecutor};
use time::OffsetDateTime;

use crate::Result;

/// The topic of the message every audit row writes: the row itself, as the API shows it.
pub const AUDIT_TOPIC: &str = "audit.recorded";

/// One undelivered message.
#[derive(Debug, Clone, PartialEq, FromRow)]
pub struct Message {
    pub id: i64,
    pub topic: String,
    pub payload: Value,
    pub created_at: OffsetDateTime,
    /// Failed attempts so far.
    pub attempts: i32,
}

/// Adds a message. Inside the caller's transaction (`&mut *tx`) it exists only if the change
/// commits. `topic` is lowercase letters, digits and `_ . -` (at most 64 characters).
pub async fn enqueue(db: impl PgExecutor<'_>, topic: &str, payload: &Value) -> Result<i64> {
    Ok(
        sqlx::query_scalar("INSERT INTO outbox (topic, payload) VALUES ($1, $2) RETURNING id")
            .bind(topic)
            .bind(payload)
            .fetch_one(db)
            .await?,
    )
}

/// Up to `limit` messages that are due (never delivered, and past their backoff), oldest first.
pub async fn due(pool: &PgPool, limit: i64) -> Result<Vec<Message>> {
    Ok(sqlx::query_as(
        "SELECT id, topic, payload, created_at, attempts FROM outbox
         WHERE delivered_at IS NULL AND next_attempt_at <= now()
         ORDER BY id LIMIT $1",
    )
    .bind(limit.clamp(1, 1000))
    .fetch_all(pool)
    .await?)
}

/// Marks messages delivered. Inside a fenced transaction ([`crate::begin_fenced`]) when the
/// caller acts as the relay leader.
pub async fn mark_delivered(db: impl PgExecutor<'_>, ids: &[i64]) -> Result<u64> {
    Ok(sqlx::query(
        "UPDATE outbox SET delivered_at = now(), last_error = NULL
         WHERE id = ANY($1) AND delivered_at IS NULL",
    )
    .bind(ids)
    .execute(db)
    .await?
    .rows_affected())
}

/// Records a failed attempt: the error (shortened to 1,000 characters) and the earliest time of
/// the next attempt, `retry_in_secs` from now.
pub async fn mark_failed(
    db: impl PgExecutor<'_>,
    id: i64,
    error: &str,
    retry_in_secs: f64,
) -> Result<()> {
    let error: String = error.chars().take(1000).collect();
    sqlx::query(
        "UPDATE outbox SET attempts = attempts + 1, last_error = $2,
                next_attempt_at = now() + make_interval(secs => $3)
         WHERE id = $1 AND delivered_at IS NULL",
    )
    .bind(id)
    .bind(error)
    .bind(retry_in_secs.max(0.0))
    .execute(db)
    .await?;
    Ok(())
}

/// What is waiting: undelivered messages and the age of the oldest, in seconds (0 when none).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Backlog {
    pub pending: i64,
    pub oldest_age_secs: f64,
    /// Undelivered messages whose last attempt failed.
    pub failing: i64,
}

pub async fn backlog(pool: &PgPool) -> Result<Backlog> {
    let (pending, oldest, failing) = sqlx::query_as::<_, (i64, Option<f64>, i64)>(
        "SELECT count(*), extract(epoch FROM now() - min(created_at))::float8,
                count(*) FILTER (WHERE attempts > 0)
         FROM outbox WHERE delivered_at IS NULL",
    )
    .fetch_one(pool)
    .await?;
    Ok(Backlog {
        pending,
        oldest_age_secs: oldest.unwrap_or(0.0).max(0.0),
        failing,
    })
}

/// Removes up to `limit` messages delivered more than `older_than_secs` ago. Undelivered
/// messages are never removed.
pub async fn prune_delivered(
    db: impl PgExecutor<'_>,
    older_than_secs: f64,
    limit: i64,
) -> Result<u64> {
    Ok(sqlx::query(
        "DELETE FROM outbox WHERE id IN (
            SELECT id FROM outbox
            WHERE delivered_at IS NOT NULL AND delivered_at < now() - make_interval(secs => $1)
            ORDER BY id LIMIT $2)",
    )
    .bind(older_than_secs)
    .bind(limit.clamp(1, 10_000))
    .execute(db)
    .await?
    .rows_affected())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::identity::{AuditEntry, audit_event};

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn an_audit_row_and_its_message_are_written_together(pool: PgPool) {
        let entry = AuditEntry {
            actor: "user:1",
            actor_type: "user",
            actor_name: Some("ada"),
            action: "group.create",
            target: "ops",
            outcome: "success",
            scope: None,
            detail: &json!({ "id": 7 }),
        };
        // Rolled back: neither the row nor the message exists.
        let mut tx = crate::begin(&pool).await.expect("begin");
        audit_event(&mut *tx, &entry).await.expect("audit");
        drop(tx);
        assert_eq!(backlog(&pool).await.expect("backlog").pending, 0);
        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_events")
            .fetch_one(&pool)
            .await
            .expect("count");
        assert_eq!(rows, 0);

        // Committed: both, and the message carries the row.
        let mut tx = crate::begin(&pool).await.expect("begin");
        audit_event(&mut *tx, &entry).await.expect("audit");
        crate::commit(tx).await.expect("commit");
        let due = due(&pool, 10).await.expect("due");
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].topic, AUDIT_TOPIC);
        assert_eq!(
            (
                due[0].payload["action"].as_str(),
                due[0].payload["actorName"].as_str(),
                due[0].payload["outcome"].as_str(),
                due[0].payload["detail"]["id"].as_i64(),
            ),
            (Some("group.create"), Some("ada"), Some("success"), Some(7))
        );
        assert!(due[0].payload["id"].as_i64().is_some());
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn failed_messages_wait_for_their_backoff_and_delivered_ones_are_pruned(pool: PgPool) {
        let a = enqueue(&pool, "test.one", &json!({ "n": 1 }))
            .await
            .expect("a");
        let b = enqueue(&pool, "test.two", &json!({ "n": 2 }))
            .await
            .expect("b");
        assert_eq!(
            due(&pool, 10)
                .await
                .expect("due")
                .iter()
                .map(|m| m.id)
                .collect::<Vec<_>>(),
            [a, b],
            "oldest first"
        );
        mark_failed(&pool, a, "sink refused", 60.0)
            .await
            .expect("fail");
        let left = due(&pool, 10).await.expect("due");
        assert_eq!(left.iter().map(|m| m.id).collect::<Vec<_>>(), [b]);
        let bl = backlog(&pool).await.expect("backlog");
        assert_eq!((bl.pending, bl.failing), (2, 1));
        assert_eq!(mark_delivered(&pool, &[b]).await.expect("delivered"), 1);
        assert_eq!(mark_delivered(&pool, &[b]).await.expect("again"), 0, "once");
        assert_eq!(backlog(&pool).await.expect("backlog").pending, 1);
        // Not old enough yet; then old enough. The undelivered one stays.
        assert_eq!(prune_delivered(&pool, 3600.0, 100).await.expect("prune"), 0);
        assert_eq!(prune_delivered(&pool, -1.0, 100).await.expect("prune"), 1);
        let left: i64 = sqlx::query_scalar("SELECT count(*) FROM outbox")
            .fetch_one(&pool)
            .await
            .expect("count");
        assert_eq!(left, 1);
        let err = enqueue(&pool, "Bad Topic", &json!({})).await;
        assert!(err.is_err(), "topics are checked");
    }
}

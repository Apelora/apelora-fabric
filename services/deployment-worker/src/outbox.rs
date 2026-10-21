// SPDX-License-Identifier: AGPL-3.0-only
//! The outbox relay (ADR 0011, EVERY-SIZE.md §2.5).
//!
//! Changes write the messages they cause into the `outbox` table in their own transaction (every
//! audit row writes one, topic `audit.recorded`). This relay delivers them **at least once**:
//! oldest first, to a [`Sink`], marking each delivered afterwards. A relay that stops between
//! delivering and marking delivers again, so receivers ignore duplicates by the message `id`.
//! A failed delivery is retried with exponential backoff (2, 4, 8 … seconds, at most five
//! minutes, with jitter); a failing message does not hold back the others. Undelivered messages
//! are never dropped; delivered ones are removed after a week.
//!
//! Only one worker relays at a time: the holder of the `outbox-relay` leader lease
//! (`crates/persistence/src/coordination.rs`), and every write it makes is fenced by the lease.
//!
//! Sinks plug in here: today [`LogSink`] (one log line per message, without its detail); wave 3
//! adds alerts and webhooks, and later replication to the global plane.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use apelora_persistence::outbox::{self, Message};
use apelora_persistence::{Leadership, Pool, StoreError, begin_fenced};
use tokio::sync::watch;

use crate::supervisor::{AbortOnDrop, LEADER_LEASE_SECS, LEADER_RENEW_EVERY, LeaderLease};

/// The leader lease the relay runs under.
pub const LEADER_LEASE: &str = "outbox-relay";
/// Messages read per pass.
const BATCH: i64 = 100;
/// Delivered messages are kept this long (for troubleshooting), then removed.
const KEEP_DELIVERED_SECS: f64 = 7.0 * 24.0 * 3600.0;
/// The longest wait between two attempts at one message.
const MAX_BACKOFF_SECS: f64 = 300.0;

/// A future a sink returns.
pub type Delivery<'a> = Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;

/// Where messages go. A sink must tolerate the same message twice (at-least-once delivery) and
/// bound its own time: the relay waits for it.
pub trait Sink: Send + Sync {
    /// A short name for logs.
    fn name(&self) -> &'static str;
    /// Delivers one message; `Err` with a reason (no secrets) to have it retried later.
    fn deliver<'a>(&'a self, m: &'a Message) -> Delivery<'a>;
}

/// Writes one log line per message (`outbox: message`, with its id, topic and, for audit rows,
/// the action, outcome, actor and target; never the detail). The stand-in delivery target until
/// alerts, webhooks and replication exist; with JSON logs it also streams the audit trail to a
/// central log store.
pub struct LogSink;

impl Sink for LogSink {
    fn name(&self) -> &'static str {
        "log"
    }

    fn deliver<'a>(&'a self, m: &'a Message) -> Delivery<'a> {
        Box::pin(async move {
            let p = &m.payload;
            let s = |k: &str| p[k].as_str().unwrap_or("-").to_owned();
            tracing::info!(
                id = m.id,
                topic = %m.topic,
                action = %s("action"),
                outcome = %s("outcome"),
                actor = %s("actor"),
                target = %s("target"),
                "outbox: message"
            );
            Ok(())
        })
    }
}

/// The sink `APELORA_OUTBOX_SINK` asks for: `log` (the default) or `off` (no relay; messages
/// wait in the table and the backlog grows).
pub fn sink_from_env(
    get: impl Fn(&str) -> Option<String>,
) -> Result<Option<Arc<dyn Sink>>, String> {
    match get("APELORA_OUTBOX_SINK")
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
        .as_deref()
    {
        None | Some("log") => Ok(Some(Arc::new(LogSink))),
        Some("off") => Ok(None),
        Some(other) => Err(format!("APELORA_OUTBOX_SINK={other:?}: use log or off")),
    }
}

/// Seconds to wait before the next attempt after `failures` failed ones: 2^failures, at most
/// five minutes, stretched by `jitter` (0 to 1) by up to a fifth so retries spread out.
pub fn backoff_secs(failures: i32, jitter: f64) -> f64 {
    let base = 2f64.powi(failures.clamp(1, 16)).min(MAX_BACKOFF_SECS);
    base * (1.0 + 0.2 * jitter.clamp(0.0, 1.0))
}

fn jitter() -> f64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    f64::from(nanos % 1000) / 1000.0
}

/// What one pass did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Pass {
    pub delivered: usize,
    pub failed: usize,
    pub pruned: u64,
    /// Undelivered messages after the pass, and how many of them last failed.
    pub pending: i64,
    pub failing: i64,
}

/// One pass as the relay: delivers what is due, oldest first, marks it delivered, records
/// failures with their backoff, removes old delivered messages and reports the backlog. Every
/// write is fenced by `lease` (`None` in tests): a relay that lost the lease gets
/// `StoreError::Conflict` and writes nothing (what it delivered is delivered again by the next
/// leader, as at-least-once allows).
pub async fn relay_once(
    pool: &Pool,
    sink: &dyn Sink,
    lease: Option<&Leadership>,
) -> Result<Pass, StoreError> {
    let mut pass = Pass::default();
    let due = outbox::due(pool, BATCH).await?;
    let mut delivered = Vec::new();
    for m in &due {
        let topic = apelora_metrics::topic_label(&m.topic);
        match sink.deliver(m).await {
            Ok(()) => {
                apelora_metrics::OUTBOX_DELIVERIES.inc(&[topic, "delivered"]);
                delivered.push(m.id);
            }
            Err(reason) => {
                apelora_metrics::OUTBOX_DELIVERIES.inc(&[topic, "failed"]);
                let wait = backoff_secs(m.attempts + 1, jitter());
                tracing::warn!(id = m.id, topic = %m.topic, sink = sink.name(), attempts = m.attempts + 1, retry_in_secs = wait.round(), reason = %reason, "outbox: delivery failed; will retry");
                let mut tx = begin_fenced(pool, lease).await?;
                outbox::mark_failed(&mut *tx, m.id, &reason, wait).await?;
                tx.commit().await?;
                pass.failed += 1;
            }
        }
    }
    if !delivered.is_empty() {
        let mut tx = begin_fenced(pool, lease).await?;
        outbox::mark_delivered(&mut *tx, &delivered).await?;
        tx.commit().await?;
        pass.delivered = delivered.len();
    }
    let mut tx = begin_fenced(pool, lease).await?;
    pass.pruned = outbox::prune_delivered(&mut *tx, KEEP_DELIVERED_SECS, 1000).await?;
    tx.commit().await?;
    let backlog = outbox::backlog(pool).await?;
    apelora_metrics::OUTBOX_PENDING.set(&[], backlog.pending as f64);
    apelora_metrics::OUTBOX_OLDEST_PENDING_AGE.set(&[], backlog.oldest_age_secs);
    apelora_metrics::OUTBOX_FAILING.set(&[], backlog.failing as f64);
    (pass.pending, pass.failing) = (backlog.pending, backlog.failing);
    if pass.delivered > 0 || pass.failed > 0 {
        tracing::debug!(
            delivered = pass.delivered,
            failed = pass.failed,
            pending = backlog.pending,
            "outbox: pass"
        );
    }
    Ok(pass)
}

/// A worker that does not relay reports no backlog (the leader does).
fn clear_backlog_gauges() {
    apelora_metrics::OUTBOX_PENDING.replace([]);
    apelora_metrics::OUTBOX_OLDEST_PENDING_AGE.replace([]);
    apelora_metrics::OUTBOX_FAILING.replace([]);
}

/// Relays every `every` while this worker (`holder`) holds the `outbox-relay` lease, until the
/// task is dropped. The lease is renewed on its own task, so a slow sink does not let it lapse.
pub async fn relay(pool: Pool, sink: Arc<dyn Sink>, every: Duration, holder: String) {
    let mut lease = LeaderLease::new(LEADER_LEASE, &holder, LEADER_LEASE_SECS);
    let (tx, rx) = watch::channel(lease.tick(&pool).await);
    let renewals = tokio::spawn({
        let pool = pool.clone();
        async move {
            loop {
                tokio::time::sleep(LEADER_RENEW_EVERY).await;
                tx.send_replace(lease.tick(&pool).await);
            }
        }
    });
    let _renewals = AbortOnDrop(renewals);
    tracing::info!(
        sink = sink.name(),
        every_ms = every.as_millis() as u64,
        "outbox relay started"
    );
    loop {
        let current = rx.borrow().clone();
        match current {
            None => clear_backlog_gauges(),
            Some(l) => match relay_once(&pool, sink.as_ref(), Some(&l)).await {
                Ok(_) => {}
                Err(StoreError::Conflict(msg)) => {
                    tracing::warn!(%msg, "outbox: pass stopped: the relay lease is no longer held")
                }
                Err(e) => tracing::error!(error = %e, "outbox: pass failed"),
            },
        }
        tokio::time::sleep(every).await;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use apelora_persistence::{acquire_leadership, enqueue_job};
    use serde_json::json;

    use super::*;

    /// Records what it was given; refuses the first `fail` deliveries.
    #[derive(Default)]
    struct Recorder {
        seen: Mutex<Vec<i64>>,
        fail: Mutex<usize>,
    }

    impl Sink for Recorder {
        fn name(&self) -> &'static str {
            "test"
        }
        fn deliver<'a>(&'a self, m: &'a Message) -> Delivery<'a> {
            Box::pin(async move {
                let mut fail = self.fail.lock().expect("lock");
                if *fail > 0 {
                    *fail -= 1;
                    return Err("receiver unavailable".into());
                }
                self.seen.lock().expect("lock").push(m.id);
                Ok(())
            })
        }
    }

    #[test]
    fn backoff_doubles_up_to_five_minutes_with_bounded_jitter() {
        assert_eq!(backoff_secs(1, 0.0), 2.0);
        assert_eq!(backoff_secs(3, 0.0), 8.0);
        assert_eq!(backoff_secs(20, 0.0), 300.0);
        assert_eq!(backoff_secs(20, 1.0), 360.0);
        assert!(backoff_secs(0, 0.5) >= 2.0, "never immediately");
    }

    #[test]
    fn the_sink_is_chosen_by_setting() {
        let env = |v: Option<&'static str>| move |_: &str| v.map(str::to_owned);
        assert_eq!(
            sink_from_env(env(None)).expect("default").map(|s| s.name()),
            Some("log")
        );
        assert!(sink_from_env(env(Some("off"))).expect("off").is_none());
        assert!(sink_from_env(env(Some("kafka"))).is_err());
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn messages_are_delivered_oldest_first_once_marked_and_failures_wait(pool: Pool) {
        // Two audit rows (each writes its message) and a message of another topic.
        apelora_persistence::audit(&pool, "test", "group.create", "ops", &json!({}))
            .await
            .expect("audit");
        apelora_persistence::audit(&pool, "test", "group.delete", "ops", &json!({}))
            .await
            .expect("audit");
        let other = outbox::enqueue(&pool, "test.other", &json!({ "n": 1 }))
            .await
            .expect("enqueue");
        let sink = Recorder::default();
        let pass = relay_once(&pool, &sink, None).await.expect("pass");
        assert_eq!(pass.delivered, 3);
        let seen = sink.seen.lock().expect("lock").clone();
        assert!(
            seen.windows(2).all(|w| w[0] < w[1]),
            "oldest first: {seen:?}"
        );
        assert_eq!(seen.last(), Some(&other));
        assert_eq!(pass.pending, 0);
        // Marked: the next pass delivers nothing again.
        assert_eq!(
            relay_once(&pool, &sink, None)
                .await
                .expect("pass")
                .delivered,
            0
        );

        // A receiver that is down: the message waits for its backoff, with the reason.
        outbox::enqueue(&pool, "test.other", &json!({ "n": 2 }))
            .await
            .expect("enqueue");
        *sink.fail.lock().expect("lock") = 1;
        let pass = relay_once(&pool, &sink, None).await.expect("pass");
        assert_eq!((pass.delivered, pass.failed), (0, 1));
        let (attempts, error, waits): (i32, Option<String>, bool) = sqlx::query_as(
            "SELECT attempts, last_error, next_attempt_at > now() FROM outbox WHERE delivered_at IS NULL",
        )
        .fetch_one(&pool)
        .await
        .expect("row");
        assert_eq!(
            (attempts, error.as_deref(), waits),
            (1, Some("receiver unavailable"), true)
        );
        assert_eq!((pass.pending, pass.failing), (1, 1));
        assert_eq!(
            relay_once(&pool, &sink, None)
                .await
                .expect("pass")
                .delivered,
            0,
            "not before its time"
        );
        sqlx::query("UPDATE outbox SET next_attempt_at = now() WHERE delivered_at IS NULL")
            .execute(&pool)
            .await
            .expect("due");
        let pass = relay_once(&pool, &sink, None).await.expect("pass");
        assert_eq!((pass.delivered, pass.pending, pass.failing), (1, 0, 0));
        // Jobs are not messages: queueing one writes no outbox row.
        enqueue_job(&pool, "test.job", None, &json!({}), None)
            .await
            .expect("job");
        assert_eq!(outbox::backlog(&pool).await.expect("backlog").pending, 0);
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn a_relay_that_lost_its_lease_marks_nothing(pool: Pool) {
        let held = acquire_leadership(&pool, LEADER_LEASE, "worker-a", 60.0)
            .await
            .expect("lease")
            .expect("free");
        let stale = Leadership {
            generation: held.generation + 1,
            ..held.clone()
        };
        outbox::enqueue(&pool, "test.other", &json!({}))
            .await
            .expect("enqueue");
        let sink = Recorder::default();
        let e = relay_once(&pool, &sink, Some(&stale)).await;
        assert!(matches!(e, Err(StoreError::Conflict(_))), "{e:?}");
        assert_eq!(
            outbox::backlog(&pool).await.expect("backlog").pending,
            1,
            "still undelivered: the leader delivers it again (at least once)"
        );
        let pass = relay_once(&pool, &sink, Some(&held)).await.expect("pass");
        assert_eq!(pass.delivered, 1);
        assert_eq!(
            sink.seen.lock().expect("lock").len(),
            2,
            "delivered twice, marked once"
        );
    }
}

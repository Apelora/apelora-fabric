// SPDX-License-Identifier: AGPL-3.0-only
//! Alerts (migration 0025, docs/design/alerts.md): operator changes to the built-in rules, the
//! alerts themselves (open and history), acknowledgements, silences, webhook channels and the
//! evaluator's last pass.
//!
//! The evaluator in the worker is the only writer of alert states and passes the time it
//! decided at (`at`), so one pass uses one clock. People's changes (acknowledge, silence, rules,
//! channels) are written by the API inside the transaction that also writes their audit row.
//! A notification is an outbox message (topic [`ALERT_TOPIC`]) written in the state change's
//! transaction, one per channel.

use serde_json::Value;
use sqlx::postgres::PgPool;
use sqlx::{FromRow, PgConnection, PgExecutor};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{Result, StoreError};

/// The outbox topic of alert notifications.
pub const ALERT_TOPIC: &str = "alert";
/// Resolved alerts are kept this long, then pruned by the evaluator.
pub const KEEP_RESOLVED_SECS: f64 = 90.0 * 24.0 * 3600.0;
/// The environment variable names a channel's secret reference may use.
pub const SECRET_REF_PREFIX: &str = "APELORA_ALERT_SECRET_";

// ---- rule settings ----------------------------------------------------------------------------

/// What an operator changed about a built-in rule.
#[derive(Debug, Clone, PartialEq, FromRow)]
pub struct RuleSettingsRow {
    pub rule: String,
    pub enabled: bool,
    pub threshold: f64,
    pub for_secs: i32,
    pub severity: String,
    pub updated_at: OffsetDateTime,
    pub updated_by: String,
}

pub async fn rule_settings(db: impl PgExecutor<'_>) -> Result<Vec<RuleSettingsRow>> {
    Ok(sqlx::query_as(
        "SELECT rule, enabled, threshold, for_secs, severity, updated_at, updated_by
         FROM alert_rule_settings ORDER BY rule",
    )
    .fetch_all(db)
    .await?)
}

/// Saves a rule's parameters (checked by the caller against the rule's bounds).
pub async fn put_rule_settings(
    db: impl PgExecutor<'_>,
    rule: &str,
    enabled: bool,
    threshold: f64,
    for_secs: u32,
    severity: &str,
    by: &str,
) -> Result<RuleSettingsRow> {
    Ok(sqlx::query_as(
        "INSERT INTO alert_rule_settings (rule, enabled, threshold, for_secs, severity, updated_by)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (rule) DO UPDATE SET enabled = EXCLUDED.enabled,
            threshold = EXCLUDED.threshold, for_secs = EXCLUDED.for_secs,
            severity = EXCLUDED.severity, updated_by = EXCLUDED.updated_by, updated_at = now()
         RETURNING rule, enabled, threshold, for_secs, severity, updated_at, updated_by",
    )
    .bind(rule)
    .bind(enabled)
    .bind(threshold)
    .bind(i32::try_from(for_secs).unwrap_or(i32::MAX))
    .bind(severity)
    .bind(by)
    .fetch_one(db)
    .await?)
}

// ---- alerts -----------------------------------------------------------------------------------

/// One alert: open (`pending`, `firing`) or history (`resolved`).
#[derive(Debug, Clone, PartialEq, FromRow)]
pub struct AlertRow {
    pub id: i64,
    pub rule: String,
    pub subject: String,
    pub severity: String,
    pub state: String,
    pub summary: String,
    pub value: f64,
    pub threshold: f64,
    pub started_at: OffsetDateTime,
    pub fired_at: Option<OffsetDateTime>,
    pub last_seen_at: OffsetDateTime,
    pub cleared_at: Option<OffsetDateTime>,
    pub resolved_at: Option<OffsetDateTime>,
    pub acknowledged_at: Option<OffsetDateTime>,
    pub acknowledged_by: Option<String>,
    pub acknowledged_by_name: Option<String>,
    pub ack_note: Option<String>,
}

const ALERT_COLS: &str = "id, rule, subject, severity, state, summary, value, threshold, started_at, fired_at, last_seen_at, cleared_at, resolved_at, acknowledged_at, acknowledged_by, acknowledged_by_name, ack_note";

/// Open alerts (pending and firing), most severe and oldest first.
pub async fn open_alerts(db: impl PgExecutor<'_>) -> Result<Vec<AlertRow>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {ALERT_COLS} FROM alerts WHERE state <> 'resolved'
         ORDER BY CASE severity WHEN 'critical' THEN 0 WHEN 'warning' THEN 1 ELSE 2 END, started_at, id"
    ))
    .fetch_all(db)
    .await?)
}

pub async fn alert(db: impl PgExecutor<'_>, id: i64) -> Result<AlertRow> {
    sqlx::query_as(&format!("SELECT {ALERT_COLS} FROM alerts WHERE id = $1"))
        .bind(id)
        .fetch_optional(db)
        .await?
        .ok_or_else(|| StoreError::NotFound(format!("alert {id}")))
}

/// Resolved alerts, newest first, older than `before` (an id) when given.
pub async fn history(pool: &PgPool, before: Option<i64>, limit: i64) -> Result<Vec<AlertRow>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {ALERT_COLS} FROM alerts WHERE state = 'resolved' AND ($1::bigint IS NULL OR id < $1)
         ORDER BY id DESC LIMIT $2"
    ))
    .bind(before)
    // One more than the largest page, so a caller can tell whether another page exists.
    .bind(limit.clamp(1, 201))
    .fetch_all(pool)
    .await?)
}

/// A new alert the evaluator opens.
pub struct NewAlert<'a> {
    pub rule: &'a str,
    pub subject: &'a str,
    pub severity: &'a str,
    pub firing: bool,
    pub summary: &'a str,
    pub value: f64,
    pub threshold: f64,
}

/// Opens an alert at `at` (pending, or firing at once). Fails on a second open alert for the
/// same rule and subject (the unique index deduplicates).
pub async fn open_alert(
    conn: &mut PgConnection,
    a: &NewAlert<'_>,
    at: OffsetDateTime,
) -> Result<AlertRow> {
    Ok(sqlx::query_as(&format!(
        "INSERT INTO alerts (rule, subject, severity, state, summary, value, threshold, started_at, fired_at, last_seen_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $8) RETURNING {ALERT_COLS}"
    ))
    .bind(a.rule)
    .bind(clip(a.subject, 300))
    .bind(a.severity)
    .bind(if a.firing { "firing" } else { "pending" })
    .bind(clip(a.summary, 500))
    .bind(a.value)
    .bind(a.threshold)
    .bind(at)
    .bind(a.firing.then_some(at))
    .fetch_one(&mut *conn)
    .await?)
}

/// What the condition looks like now, for an open alert.
pub struct Seen<'a> {
    pub severity: &'a str,
    pub summary: &'a str,
    pub value: f64,
    pub threshold: f64,
}

/// The condition still holds: refresh what it looks like and forget a clearing in progress.
/// With `fire`, a pending alert fires now.
pub async fn refresh(
    conn: &mut PgConnection,
    id: i64,
    seen: &Seen<'_>,
    fire: bool,
    at: OffsetDateTime,
) -> Result<AlertRow> {
    sqlx::query_as(&format!(
        "UPDATE alerts SET severity = $2, summary = $3, value = $4, threshold = $5, last_seen_at = $6,
                cleared_at = NULL,
                state = CASE WHEN $7 THEN 'firing' ELSE state END,
                fired_at = CASE WHEN $7 AND fired_at IS NULL THEN $6 ELSE fired_at END
         WHERE id = $1 AND state <> 'resolved' RETURNING {ALERT_COLS}"
    ))
    .bind(id)
    .bind(seen.severity)
    .bind(clip(seen.summary, 500))
    .bind(seen.value)
    .bind(seen.threshold)
    .bind(at)
    .bind(fire)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| StoreError::Conflict(format!("alert {id} is no longer open")))
}

/// A firing alert's condition is clear: note since when (kept if already noted).
pub async fn clearing(conn: &mut PgConnection, id: i64, at: OffsetDateTime) -> Result<()> {
    sqlx::query("UPDATE alerts SET cleared_at = coalesce(cleared_at, $2) WHERE id = $1 AND state = 'firing'")
        .bind(id)
        .bind(at)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// Resolves an open alert at `at`.
pub async fn resolve(conn: &mut PgConnection, id: i64, at: OffsetDateTime) -> Result<AlertRow> {
    sqlx::query_as(&format!(
        "UPDATE alerts SET state = 'resolved', resolved_at = $2,
                fired_at = coalesce(fired_at, $2), cleared_at = coalesce(cleared_at, $2)
         WHERE id = $1 AND state = 'firing' RETURNING {ALERT_COLS}"
    ))
    .bind(id)
    .bind(at)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| StoreError::Conflict(format!("alert {id} is not firing")))
}

/// Forgets a pending alert whose condition cleared before it fired.
pub async fn drop_pending(conn: &mut PgConnection, id: i64) -> Result<()> {
    sqlx::query("DELETE FROM alerts WHERE id = $1 AND state = 'pending'")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// Removes up to 1,000 resolved alerts that resolved more than `older_than_secs` before `at`.
pub async fn prune_resolved(
    conn: &mut PgConnection,
    older_than_secs: f64,
    at: OffsetDateTime,
) -> Result<u64> {
    Ok(sqlx::query(
        "DELETE FROM alerts WHERE id IN (
            SELECT id FROM alerts WHERE state = 'resolved'
              AND resolved_at < $2 - make_interval(secs => $1) ORDER BY id LIMIT 1000)",
    )
    .bind(older_than_secs)
    .bind(at)
    .execute(&mut *conn)
    .await?
    .rows_affected())
}

/// The answer to an acknowledgement.
#[derive(Debug, Clone, PartialEq)]
pub enum Ack {
    Done(Box<AlertRow>),
    /// Only firing alerts are acknowledged: it is pending or resolved.
    NotFiring(String),
    /// Somebody acknowledged it already (who).
    Already(String),
}

/// Records who acknowledged a firing alert, when, and their note. Run in the transaction that
/// writes the audit row.
pub async fn acknowledge(
    conn: &mut PgConnection,
    id: i64,
    by: &str,
    by_name: &str,
    note: &str,
) -> Result<Ack> {
    let current: AlertRow = sqlx::query_as(&format!(
        "SELECT {ALERT_COLS} FROM alerts WHERE id = $1 FOR UPDATE"
    ))
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| StoreError::NotFound(format!("alert {id}")))?;
    if current.state != "firing" {
        return Ok(Ack::NotFiring(current.state));
    }
    if let Some(who) = current.acknowledged_by_name.or(current.acknowledged_by) {
        return Ok(Ack::Already(who));
    }
    let row = sqlx::query_as(&format!(
        "UPDATE alerts SET acknowledged_at = now(), acknowledged_by = $2, acknowledged_by_name = $3,
                ack_note = NULLIF($4, '')
         WHERE id = $1 RETURNING {ALERT_COLS}"
    ))
    .bind(id)
    .bind(by)
    .bind(clip(by_name, 200))
    .bind(clip(note, 500))
    .fetch_one(&mut *conn)
    .await?;
    Ok(Ack::Done(Box::new(row)))
}

// ---- silences ---------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, FromRow)]
pub struct SilenceRow {
    pub id: Uuid,
    pub rule: String,
    pub subject: Option<String>,
    pub starts_at: OffsetDateTime,
    pub ends_at: OffsetDateTime,
    pub note: String,
    pub created_by: String,
    pub created_by_name: Option<String>,
    pub created_at: OffsetDateTime,
    pub ended_at: Option<OffsetDateTime>,
    pub ended_by: Option<String>,
}

const SILENCE_COLS: &str = "id, rule, subject, starts_at, ends_at, note, created_by, created_by_name, created_at, ended_at, ended_by";

/// Silences in force at `at` (started, not ended early, not run out).
pub async fn active_silences(
    db: impl PgExecutor<'_>,
    at: OffsetDateTime,
) -> Result<Vec<SilenceRow>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {SILENCE_COLS} FROM alert_silences
         WHERE ended_at IS NULL AND starts_at <= $1 AND ends_at > $1 ORDER BY ends_at, id"
    ))
    .bind(at)
    .fetch_all(db)
    .await?)
}

pub async fn silence(db: impl PgExecutor<'_>, id: Uuid) -> Result<SilenceRow> {
    sqlx::query_as(&format!(
        "SELECT {SILENCE_COLS} FROM alert_silences WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(db)
    .await?
    .ok_or_else(|| StoreError::NotFound(format!("silence {id}")))
}

pub struct NewSilence<'a> {
    pub rule: &'a str,
    pub subject: Option<&'a str>,
    pub minutes: u32,
    pub note: &'a str,
    pub by: &'a str,
    pub by_name: &'a str,
}

/// A silence from now for `minutes`.
pub async fn create_silence(db: impl PgExecutor<'_>, s: &NewSilence<'_>) -> Result<SilenceRow> {
    Ok(sqlx::query_as(&format!(
        "INSERT INTO alert_silences (id, rule, subject, ends_at, note, created_by, created_by_name)
         VALUES ($1, $2, $3, now() + make_interval(mins => $4), $5, $6, $7) RETURNING {SILENCE_COLS}"
    ))
    .bind(Uuid::new_v4())
    .bind(s.rule)
    .bind(s.subject)
    .bind(i32::try_from(s.minutes).unwrap_or(i32::MAX))
    .bind(clip(s.note, 500))
    .bind(s.by)
    .bind(clip(s.by_name, 200))
    .fetch_one(db)
    .await?)
}

/// Ends a silence now. `false` when it had already ended or run out.
pub async fn end_silence(db: impl PgExecutor<'_>, id: Uuid, by: &str) -> Result<bool> {
    Ok(sqlx::query(
        "UPDATE alert_silences SET ended_at = now(), ended_by = $2
         WHERE id = $1 AND ended_at IS NULL AND ends_at > now()",
    )
    .bind(id)
    .bind(by)
    .execute(db)
    .await?
    .rows_affected()
        == 1)
}

// ---- channels ---------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, FromRow)]
pub struct ChannelRow {
    pub id: Uuid,
    pub name: String,
    pub kind: String,
    pub url: String,
    pub secret_ref: String,
    pub enabled: bool,
    pub min_severity: String,
    pub created_at: OffsetDateTime,
    pub created_by: String,
    pub updated_at: OffsetDateTime,
}

const CHANNEL_COLS: &str =
    "id, name, kind, url, secret_ref, enabled, min_severity, created_at, created_by, updated_at";

pub async fn channels(db: impl PgExecutor<'_>) -> Result<Vec<ChannelRow>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {CHANNEL_COLS} FROM alert_channels ORDER BY name"
    ))
    .fetch_all(db)
    .await?)
}

pub async fn channel(db: impl PgExecutor<'_>, id: Uuid) -> Result<ChannelRow> {
    sqlx::query_as(&format!(
        "SELECT {CHANNEL_COLS} FROM alert_channels WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(db)
    .await?
    .ok_or_else(|| StoreError::NotFound(format!("channel {id}")))
}

/// A channel's settings (checked by the caller: HTTPS URL, secret reference name).
pub struct ChannelInput<'a> {
    pub name: &'a str,
    pub url: &'a str,
    pub secret_ref: &'a str,
    pub enabled: bool,
    pub min_severity: &'a str,
}

/// `Conflict` when the name is taken.
pub async fn create_channel(
    db: impl PgExecutor<'_>,
    c: &ChannelInput<'_>,
    by: &str,
) -> Result<ChannelRow> {
    sqlx::query_as(&format!(
        "INSERT INTO alert_channels (id, name, kind, url, secret_ref, enabled, min_severity, created_by)
         VALUES ($1, $2, 'webhook', $3, $4, $5, $6, $7) RETURNING {CHANNEL_COLS}"
    ))
    .bind(Uuid::new_v4())
    .bind(c.name)
    .bind(c.url)
    .bind(c.secret_ref)
    .bind(c.enabled)
    .bind(c.min_severity)
    .bind(by)
    .fetch_one(db)
    .await
    .map_err(name_taken)
}

pub async fn update_channel(
    db: impl PgExecutor<'_>,
    id: Uuid,
    c: &ChannelInput<'_>,
) -> Result<ChannelRow> {
    sqlx::query_as(&format!(
        "UPDATE alert_channels SET name = $2, url = $3, secret_ref = $4, enabled = $5,
                min_severity = $6, updated_at = now()
         WHERE id = $1 RETURNING {CHANNEL_COLS}"
    ))
    .bind(id)
    .bind(c.name)
    .bind(c.url)
    .bind(c.secret_ref)
    .bind(c.enabled)
    .bind(c.min_severity)
    .fetch_optional(db)
    .await
    .map_err(name_taken)?
    .ok_or_else(|| StoreError::NotFound(format!("channel {id}")))
}

pub async fn delete_channel(db: impl PgExecutor<'_>, id: Uuid) -> Result<bool> {
    Ok(sqlx::query("DELETE FROM alert_channels WHERE id = $1")
        .bind(id)
        .execute(db)
        .await?
        .rows_affected()
        == 1)
}

fn name_taken(e: sqlx::Error) -> StoreError {
    match &e {
        sqlx::Error::Database(d) if d.code().as_deref() == Some("23505") => {
            StoreError::Conflict("a channel with this name exists already".into())
        }
        _ => e.into(),
    }
}

/// A channel's delivery state, read from its outbox messages (kept a week after delivery).
#[derive(Debug, Clone, PartialEq, Default, FromRow)]
pub struct ChannelDelivery {
    pub channel: String,
    /// Messages waiting (never delivered).
    pub pending: i64,
    /// Waiting messages whose last attempt failed.
    pub failing: i64,
    /// The newest failure's reason, while a message is still failing.
    pub last_error: Option<String>,
    pub last_delivered_at: Option<OffsetDateTime>,
}

pub async fn channel_deliveries(pool: &PgPool) -> Result<Vec<ChannelDelivery>> {
    Ok(sqlx::query_as(
        "SELECT payload->>'channel' AS channel,
                count(*) FILTER (WHERE delivered_at IS NULL) AS pending,
                count(*) FILTER (WHERE delivered_at IS NULL AND attempts > 0) AS failing,
                (array_agg(last_error ORDER BY id DESC) FILTER (WHERE delivered_at IS NULL AND last_error IS NOT NULL))[1] AS last_error,
                max(delivered_at) AS last_delivered_at
         FROM outbox WHERE topic = 'alert' AND payload ? 'channel'
         GROUP BY 1",
    )
    .fetch_all(pool)
    .await?)
}

// ---- notifications ----------------------------------------------------------------------------

/// Writes one notification per channel in `channels` into the outbox, inside the caller's
/// transaction. `body` is the message without the channel; each copy gets `"channel": <id>`.
/// Returns the messages' ids.
pub async fn notify(conn: &mut PgConnection, channels: &[Uuid], body: &Value) -> Result<Vec<i64>> {
    let mut ids = Vec::with_capacity(channels.len());
    for c in channels {
        let mut payload = body.clone();
        if let Some(o) = payload.as_object_mut() {
            o.insert("channel".into(), Value::String(c.to_string()));
        }
        ids.push(crate::outbox::enqueue(&mut *conn, ALERT_TOPIC, &payload).await?);
    }
    Ok(ids)
}

// ---- facts the rules read ---------------------------------------------------------------------

/// Failed jobs per kind that ended in the last `window_secs`: (kind, count, the newest error).
pub async fn failed_jobs_by_kind(
    pool: &PgPool,
    window_secs: f64,
) -> Result<Vec<(String, i64, Option<String>)>> {
    Ok(sqlx::query_as(
        "SELECT kind, count(*), (array_agg(last_error ORDER BY updated_at DESC))[1]
         FROM jobs WHERE status = 'failed' AND updated_at > now() - make_interval(secs => $1)
         GROUP BY kind ORDER BY kind",
    )
    .bind(window_secs)
    .fetch_all(pool)
    .await?)
}

/// Every leader lease: (role, last holder, seconds since it expired; negative while held).
pub async fn leader_leases(pool: &PgPool) -> Result<Vec<(String, String, f64)>> {
    Ok(sqlx::query_as(
        "SELECT name, holder, floor(extract(epoch FROM now() - expires_at))::float8
         FROM leader_leases ORDER BY name",
    )
    .fetch_all(pool)
    .await?)
}

/// Audit rows the replica supervisor wrote in the last `window_secs` (one per action it took).
pub async fn supervisor_actions(pool: &PgPool, window_secs: f64) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM audit_events
         WHERE actor = 'supervisor' AND at > now() - make_interval(secs => $1)",
    )
    .bind(window_secs)
    .fetch_one(pool)
    .await?)
}

/// Instances the supervisor rates unhealthy: (project, instance, what it rests on).
pub async fn unhealthy_instances(pool: &PgPool) -> Result<Vec<(String, String, String)>> {
    Ok(sqlx::query_as(
        "SELECT project, instance, detail FROM instance_health
         WHERE health = 'unhealthy' ORDER BY project, instance",
    )
    .fetch_all(pool)
    .await?)
}

/// A volume with a schedule or a failed scheduled run.
#[derive(Debug, Clone, PartialEq, FromRow)]
pub struct VolumeScheduleFacts {
    pub project: String,
    pub incus_name: String,
    pub snapshot_error: Option<String>,
    pub backup_error: Option<String>,
    pub backup_every_minutes: Option<i32>,
    /// Seconds since its newest backup (since it was created when it has none).
    pub backup_age_secs: f64,
}

pub async fn volume_schedules(pool: &PgPool) -> Result<Vec<VolumeScheduleFacts>> {
    Ok(sqlx::query_as(
        "SELECT project, incus_name, snapshot_error, backup_error, backup_every_minutes,
                extract(epoch FROM now() - coalesce(last_backup_at, created_at))::float8 AS backup_age_secs
         FROM volumes
         WHERE snapshot_error IS NOT NULL OR backup_error IS NOT NULL OR backup_every_minutes IS NOT NULL
         ORDER BY project, incus_name",
    )
    .fetch_all(pool)
    .await?)
}

// ---- the evaluator ----------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, FromRow)]
pub struct EvaluatorRow {
    pub last_pass_at: OffsetDateTime,
    pub duration_secs: f64,
    pub worker: String,
    pub every_secs: i32,
    pub sources: Value,
}

pub async fn record_pass(
    db: impl PgExecutor<'_>,
    at: OffsetDateTime,
    duration_secs: f64,
    worker: &str,
    every_secs: u32,
    sources: &Value,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO alert_evaluator (id, last_pass_at, duration_secs, worker, every_secs, sources)
         VALUES (1, $1, $2, $3, $4, $5)
         ON CONFLICT (id) DO UPDATE SET last_pass_at = EXCLUDED.last_pass_at,
            duration_secs = EXCLUDED.duration_secs, worker = EXCLUDED.worker,
            every_secs = EXCLUDED.every_secs, sources = EXCLUDED.sources",
    )
    .bind(at)
    .bind(duration_secs)
    .bind(worker)
    .bind(i32::try_from(every_secs.max(1)).unwrap_or(i32::MAX))
    .bind(sources)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn evaluator(db: impl PgExecutor<'_>) -> Result<Option<EvaluatorRow>> {
    Ok(sqlx::query_as(
        "SELECT last_pass_at, duration_secs, worker, every_secs, sources FROM alert_evaluator WHERE id = 1",
    )
    .fetch_optional(db)
    .await?)
}

/// The database's clock: one pass decides with one time.
pub async fn now(db: impl PgExecutor<'_>) -> Result<OffsetDateTime> {
    Ok(sqlx::query_scalar("SELECT now()").fetch_one(db).await?)
}

fn clip(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

#[cfg(test)]
mod tests;

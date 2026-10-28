// SPDX-License-Identifier: AGPL-3.0-only
//! Alerts (docs/design/alerts.md): open alerts and their history, acknowledgements, silences,
//! the built-in rules' parameters and webhook channels.
//!
//! The worker's evaluator decides alert states; this API only reads them and records what people
//! do: acknowledge (who, when, note), silence a rule or one subject for a time, change a rule's
//! parameters, and manage channels. Each change and its audit row commit together. Seeing alerts
//! needs `cluster.view`; acknowledging and silencing `alert.respond`; rules and channels
//! `alert.configure` (all at platform scope).
//!
//! A channel stores a **secret reference** (the name of an `APELORA_ALERT_SECRET_*` variable on
//! the worker), never a secret. Its URL may carry a receiver's token, so the full URL is shown
//! only to callers who may configure channels; others see its host.

use apelora_domain::alerts::{
    AlertComparison, AlertRule, AlertSeverity, RULES, alert_rule, silence_covers,
};
use apelora_persistence::alerts::{self as db, AlertRow, SECRET_REF_PREFIX};
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::OffsetDateTime;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use super::authz::{allow_platform, audit_as};
use super::deploy::{bad_request, rfc3339, store};
use super::manage::ActionState;
use super::{ApiError, AppState, ErrorBody, Principal};

const VIEW: &str = "cluster.view";
const RESPOND: &str = "alert.respond";
const CONFIGURE: &str = "alert.configure";
/// Longest silence: a week.
const MAX_SILENCE_MINUTES: u32 = 7 * 24 * 60;

// ---- response types ---------------------------------------------------------------------------

/// Who acknowledged an alert.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AlertAcknowledgement {
    /// `user:<id>`, `service:<id>` or `token:<name>`.
    pub by: String,
    pub by_name: Option<String>,
    /// RFC 3339.
    pub at: String,
    pub note: Option<String>,
}

/// The silence that holds back an alert's notifications.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AlertSilencedBy {
    pub id: Uuid,
    /// RFC 3339.
    pub until: String,
}

/// One alert, firing or resolved.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AlertItem {
    pub id: i64,
    pub rule: String,
    pub title: String,
    /// What it is about: a job kind, a worker, a member, `project/instance`, …
    pub subject: String,
    pub severity: AlertSeverity,
    /// `firing` or `resolved`.
    pub state: String,
    pub summary: String,
    pub value: f64,
    pub threshold: f64,
    pub comparison: AlertComparison,
    pub unit: String,
    /// RFC 3339: when the condition was first seen, when it fired, last seen holding, resolved.
    pub started_at: String,
    pub fired_at: Option<String>,
    pub last_seen_at: String,
    pub resolved_at: Option<String>,
    /// Firing, but the condition has been clear since this time (it resolves once it stays
    /// clear for the rule's duration).
    pub clearing_since: Option<String>,
    pub acknowledged: Option<AlertAcknowledgement>,
    pub silenced_by: Option<AlertSilencedBy>,
    /// Where the operator runbooks explain this alert: a page and section, e.g.
    /// `alerts.md#jobs-are-failing`.
    pub runbook: String,
}

/// A silence in force.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AlertSilence {
    pub id: Uuid,
    pub rule: String,
    pub rule_title: String,
    /// `None`: every subject of the rule.
    pub subject: Option<String>,
    /// RFC 3339.
    pub starts_at: String,
    pub ends_at: String,
    pub note: String,
    pub created_by: String,
    pub created_by_name: Option<String>,
}

/// Firing alerts by severity, and those nobody has acknowledged or silenced.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AlertCounts {
    pub critical: u32,
    pub warning: u32,
    pub info: u32,
    /// Firing, not acknowledged and not silenced: what needs attention.
    pub needs_attention: u32,
}

/// Whether the worker's evaluator is running.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AlertEvaluator {
    /// RFC 3339; `None` when no worker has evaluated alerts yet.
    pub last_pass_at: Option<String>,
    pub every_secs: Option<i32>,
    pub worker: Option<String>,
    pub duration_secs: Option<f64>,
    /// No pass for more than three intervals (or none ever): alerts may be out of date.
    pub stale: bool,
    /// What the state means, in a sentence.
    pub message: String,
}

/// What the caller may do here.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AlertActions {
    /// Acknowledge and silence (`alert.respond`).
    pub respond: ActionState,
    /// Change rules and channels (`alert.configure`).
    pub configure: ActionState,
}

/// Open alerts (firing; pending ones are not shown), silences in force, counts and the
/// evaluator's state.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AlertList {
    pub alerts: Vec<AlertItem>,
    pub silences: Vec<AlertSilence>,
    pub counts: AlertCounts,
    pub evaluator: AlertEvaluator,
    pub actions: AlertActions,
}

/// Resolved alerts, newest first.
#[derive(Serialize, ToSchema)]
pub struct AlertHistory {
    pub items: Vec<AlertItem>,
}

/// A rule's parameters.
#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AlertRuleParams {
    pub enabled: bool,
    pub threshold: f64,
    /// Seconds the condition must hold before firing, and stay clear before resolving.
    pub for_secs: u32,
    pub severity: AlertSeverity,
}

/// A built-in rule with its parameters in force.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AlertRuleItem {
    pub id: String,
    pub title: String,
    pub description: String,
    pub unit: String,
    pub comparison: AlertComparison,
    /// `false`: the rule fires while a state lasts; only its duration and severity are tuned.
    pub threshold_editable: bool,
    pub threshold_min: f64,
    pub threshold_max: f64,
    pub params: AlertRuleParams,
    pub defaults: AlertRuleParams,
    /// Where its facts come from.
    pub source: String,
    /// What the evaluator's last pass said about that source: `ok`, `not-configured`,
    /// `unavailable`, `disabled`, or `unknown` before the first pass.
    pub source_state: String,
    pub firing: u32,
    pub runbook: String,
    /// RFC 3339 and who, when an operator changed it.
    pub updated_at: Option<String>,
    pub updated_by: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct AlertRuleList {
    pub rules: Vec<AlertRuleItem>,
    pub actions: AlertActions,
}

/// A channel's deliveries, from its outbox messages (kept a week after delivery).
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AlertChannelDelivery {
    pub pending: i64,
    pub failing: i64,
    /// Why the newest still-failing message failed.
    pub last_error: Option<String>,
    /// RFC 3339.
    pub last_delivered_at: Option<String>,
}

/// A webhook channel.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AlertChannel {
    pub id: Uuid,
    pub name: String,
    /// `webhook`.
    pub kind: String,
    /// The host it posts to.
    pub host: String,
    /// The full URL, only for callers who may configure channels (it may carry a token).
    pub url: Option<String>,
    /// The name of the environment variable on the worker that holds the signing secret.
    pub secret_ref: String,
    pub enabled: bool,
    pub min_severity: AlertSeverity,
    pub delivery: AlertChannelDelivery,
    pub created_at: String,
    pub created_by: String,
    pub updated_at: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AlertChannelList {
    pub channels: Vec<AlertChannel>,
    /// What a secret reference must start with.
    pub secret_prefix: String,
    pub actions: AlertActions,
}

/// A channel test was queued.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AlertChannelTest {
    /// The outbox message (`Apelora-Event-Id` at the receiver).
    pub message_id: i64,
}

// ---- request types ----------------------------------------------------------------------------

#[derive(Deserialize, ToSchema, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AlertAcknowledgeRequest {
    /// What is being done about it (at most 500 characters).
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AlertSilenceRequest {
    pub rule: String,
    /// One subject; absent: every subject of the rule.
    #[serde(default)]
    pub subject: Option<String>,
    /// 1 to 10080 (a week).
    pub minutes: u32,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AlertChannelRequest {
    /// 1 to 80 characters, unique.
    pub name: String,
    /// `https://…` only, without a user name or password.
    pub url: String,
    /// The name of the environment variable on the worker holding the signing secret, e.g.
    /// `APELORA_ALERT_SECRET_OPS`. Never the secret itself.
    pub secret_ref: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Alerts of this severity and above are sent (default `warning`).
    #[serde(default)]
    pub min_severity: Option<AlertSeverity>,
}

fn yes() -> bool {
    true
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct HistoryQuery {
    /// Only alerts with a smaller id (the last one of the previous page).
    pub before: Option<i64>,
    /// 1 to 200 (default 50).
    pub limit: Option<i64>,
}

// ---- helpers ----------------------------------------------------------------------------------

fn state(allowed: bool, reason: &str) -> ActionState {
    ActionState {
        allowed,
        reason: (!allowed).then(|| reason.to_owned()),
    }
}

fn actions(p: &Principal) -> AlertActions {
    AlertActions {
        respond: state(
            allow_platform(p, RESPOND).is_ok(),
            "Acknowledging and silencing alerts needs the alert.respond permission.",
        ),
        configure: state(
            allow_platform(p, CONFIGURE).is_ok(),
            "Changing rules and channels needs the alert.configure permission.",
        ),
    }
}

fn time(t: Option<OffsetDateTime>) -> Option<String> {
    t.map(rfc3339)
}

fn rule_of(id: &str) -> Option<&'static AlertRule> {
    alert_rule(id)
}

fn item(a: AlertRow, silences: &[db::SilenceRow]) -> AlertItem {
    let rule = rule_of(&a.rule);
    let silenced_by = (a.state == "firing")
        .then(|| {
            silences
                .iter()
                .find(|s| silence_covers(&s.rule, s.subject.as_deref(), &a.rule, &a.subject))
        })
        .flatten()
        .map(|s| AlertSilencedBy {
            id: s.id,
            until: rfc3339(s.ends_at),
        });
    let acknowledged = a.acknowledged_at.map(|at| AlertAcknowledgement {
        by: a.acknowledged_by.clone().unwrap_or_default(),
        by_name: a.acknowledged_by_name.clone(),
        at: rfc3339(at),
        note: a.ack_note.clone(),
    });
    AlertItem {
        id: a.id,
        title: rule.map_or_else(|| a.rule.clone(), |r| r.title.to_owned()),
        comparison: rule.map_or(AlertComparison::AtLeast, |r| r.comparison),
        unit: rule.map_or("", |r| r.unit).to_owned(),
        runbook: rule.map_or("alerts.md", |r| r.runbook).to_owned(),
        rule: a.rule,
        subject: a.subject,
        severity: AlertSeverity::parse(&a.severity).unwrap_or(AlertSeverity::Warning),
        state: a.state,
        summary: a.summary,
        value: a.value,
        threshold: a.threshold,
        started_at: rfc3339(a.started_at),
        fired_at: time(a.fired_at),
        last_seen_at: rfc3339(a.last_seen_at),
        resolved_at: time(a.resolved_at),
        clearing_since: time(a.cleared_at.filter(|_| a.resolved_at.is_none())),
        acknowledged,
        silenced_by,
    }
}

fn silence_view(s: db::SilenceRow) -> AlertSilence {
    AlertSilence {
        id: s.id,
        rule_title: rule_of(&s.rule).map_or_else(|| s.rule.clone(), |r| r.title.to_owned()),
        rule: s.rule,
        subject: s.subject,
        starts_at: rfc3339(s.starts_at),
        ends_at: rfc3339(s.ends_at),
        note: s.note,
        created_by: s.created_by,
        created_by_name: s.created_by_name,
    }
}

fn evaluator_view(e: Option<db::EvaluatorRow>, now: OffsetDateTime) -> AlertEvaluator {
    match e {
        None => AlertEvaluator {
            last_pass_at: None,
            every_secs: None,
            worker: None,
            duration_secs: None,
            stale: true,
            message: "Alerts have not been evaluated yet: no deployment worker has run the alert evaluator (APELORA_ALERT_EVALUATE_SECS, on by default).".into(),
        },
        Some(e) => {
            let age = (now - e.last_pass_at).whole_seconds();
            let limit = i64::from(e.every_secs.max(30)) * 3;
            let stale = age > limit;
            AlertEvaluator {
                last_pass_at: Some(rfc3339(e.last_pass_at)),
                every_secs: Some(e.every_secs),
                worker: Some(e.worker.clone()),
                duration_secs: Some(e.duration_secs),
                stale,
                message: if stale {
                    format!("No worker has evaluated alerts for {age} s (they run every {} s): what is shown may be out of date. Check that a deployment worker runs.", e.every_secs)
                } else {
                    format!("Evaluated every {} s by {}.", e.every_secs, e.worker)
                },
            }
        }
    }
}

fn clean_text(field: &str, s: &str, max: usize) -> Result<String, ApiError> {
    let t = s.trim();
    if t.chars().count() > max {
        return Err(bad_request(format!(
            "{field} is longer than {max} characters"
        )));
    }
    if t.chars().any(char::is_control) {
        return Err(bad_request(format!("{field} contains control characters")));
    }
    Ok(t.to_owned())
}

fn json_body<T>(body: Result<Json<T>, JsonRejection>) -> Result<T, ApiError> {
    body.map(|Json(v)| v)
        .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, "invalid_request", e.body_text()))
}

fn host_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_default()
}

/// The same rules the worker applies before every delivery (`webhook::check_url`).
fn check_url(url: &str) -> Result<(), ApiError> {
    if url.len() > 2000 {
        return Err(bad_request("the URL is longer than 2000 characters"));
    }
    let u = url::Url::parse(url).map_err(|_| bad_request("the URL is not valid"))?;
    if u.scheme() != "https" {
        return Err(bad_request(
            "webhooks are sent over HTTPS only (the URL must start with https://)",
        ));
    }
    if u.host_str().is_none_or(str::is_empty) {
        return Err(bad_request("the URL has no host"));
    }
    if !u.username().is_empty() || u.password().is_some() {
        return Err(bad_request(
            "the URL must not carry a user name or password; sign with the secret instead",
        ));
    }
    Ok(())
}

fn check_secret_ref(name: &str) -> Result<(), ApiError> {
    let rest = name.strip_prefix(SECRET_REF_PREFIX).unwrap_or("");
    if !rest.is_empty()
        && rest.len() <= 64
        && rest
            .bytes()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
    {
        Ok(())
    } else {
        Err(bad_request(format!(
            "the secret reference must name an environment variable on the worker that starts with {SECRET_REF_PREFIX} (then up to 64 capital letters, digits or _), e.g. {SECRET_REF_PREFIX}OPS; never paste the secret itself"
        )))
    }
}

fn channel_view(
    c: db::ChannelRow,
    deliveries: &[db::ChannelDelivery],
    full_url: bool,
) -> AlertChannel {
    let d = deliveries.iter().find(|d| d.channel == c.id.to_string());
    AlertChannel {
        id: c.id,
        host: host_of(&c.url),
        url: full_url.then(|| c.url.clone()),
        name: c.name,
        kind: c.kind,
        secret_ref: c.secret_ref,
        enabled: c.enabled,
        min_severity: AlertSeverity::parse(&c.min_severity).unwrap_or(AlertSeverity::Warning),
        delivery: AlertChannelDelivery {
            pending: d.map_or(0, |d| d.pending),
            failing: d.map_or(0, |d| d.failing),
            last_error: d.and_then(|d| d.last_error.clone()),
            last_delivered_at: d.and_then(|d| d.last_delivered_at).map(rfc3339),
        },
        created_at: rfc3339(c.created_at),
        created_by: c.created_by,
        updated_at: rfc3339(c.updated_at),
    }
}

fn not_found(what: &str) -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "not_found",
        format!("not found: {what}"),
    )
}

// ---- alerts -----------------------------------------------------------------------------------

/// Firing alerts (most severe and oldest first) with acknowledgement and silence, the silences
/// in force, counts by severity and whether the evaluator runs. Needs `cluster.view`.
#[utoipa::path(get, path = "/api/v1/alerts", tag = "alerts", operation_id = "alerts",
    responses((status = 200, body = AlertList), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn list(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<AlertList>, ApiError> {
    allow_platform(&p, VIEW)?;
    let pool = store(&state)?;
    let now = db::now(pool).await?;
    let silences = db::active_silences(pool, now).await?;
    let open = db::open_alerts(pool).await?;
    let alerts: Vec<AlertItem> = open
        .into_iter()
        .filter(|a| a.state == "firing")
        .map(|a| item(a, &silences))
        .collect();
    let mut counts = AlertCounts {
        critical: 0,
        warning: 0,
        info: 0,
        needs_attention: 0,
    };
    for a in &alerts {
        match a.severity {
            AlertSeverity::Critical => counts.critical += 1,
            AlertSeverity::Warning => counts.warning += 1,
            AlertSeverity::Info => counts.info += 1,
        }
        if a.acknowledged.is_none() && a.silenced_by.is_none() {
            counts.needs_attention += 1;
        }
    }
    Ok(Json(AlertList {
        alerts,
        silences: silences.into_iter().map(silence_view).collect(),
        counts,
        evaluator: evaluator_view(db::evaluator(pool).await?, now),
        actions: actions(&p),
    }))
}

/// Resolved alerts, newest first, paged by `before`. Kept 90 days. Needs `cluster.view`.
#[utoipa::path(get, path = "/api/v1/alerts/history", tag = "alerts", operation_id = "alert_history", params(HistoryQuery),
    responses((status = 200, body = AlertHistory), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn history(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Query(q): Query<HistoryQuery>,
) -> Result<Json<AlertHistory>, ApiError> {
    allow_platform(&p, VIEW)?;
    let pool = store(&state)?;
    let rows = db::history(pool, q.before, q.limit.unwrap_or(50)).await?;
    Ok(Json(AlertHistory {
        items: rows.into_iter().map(|a| item(a, &[])).collect(),
    }))
}

/// Acknowledges a firing alert: who, when and an optional note. It keeps firing until its
/// condition clears; acknowledging tells others someone is on it. Refused (409) when it is not
/// firing (`not_firing`) or was acknowledged already (`already_acknowledged`). Needs
/// `alert.respond`; audited as `alert.acknowledge`.
#[utoipa::path(post, path = "/api/v1/alerts/{id}/acknowledge", tag = "alerts", operation_id = "acknowledge_alert",
    params(("id" = i64, Path, description = "Alert id")), request_body = AlertAcknowledgeRequest,
    responses((status = 200, body = AlertItem), (status = 400, body = ErrorBody), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody),
        (status = 503, body = ErrorBody)))]
pub(super) async fn acknowledge(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(id): Path<i64>,
    body: Result<Json<AlertAcknowledgeRequest>, JsonRejection>,
) -> Result<Json<AlertItem>, ApiError> {
    allow_platform(&p, RESPOND)?;
    let req = json_body(body)?;
    let note = clean_text("the note", req.note.as_deref().unwrap_or(""), 500)?;
    let pool = store(&state)?;
    let mut tx = super::authz::begin(pool).await?;
    let row = match db::acknowledge(&mut tx, id, &p.actor_ref(), &p.name, &note).await {
        Err(apelora_persistence::StoreError::NotFound(_)) => {
            return Err(not_found(&format!("alert {id}")));
        }
        other => other?,
    };
    let row = match row {
        db::Ack::Done(row) => *row,
        db::Ack::NotFiring(state) => {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "not_firing",
                format!("only firing alerts are acknowledged; this one is {state}"),
            ));
        }
        db::Ack::Already(who) => {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "already_acknowledged",
                format!("{who} acknowledged this alert already"),
            ));
        }
    };
    audit_as(
        &mut tx,
        &p,
        "alert.acknowledge",
        &format!("alert:{id}"),
        &json!({ "rule": row.rule, "subject": row.subject, "severity": row.severity, "note": (!note.is_empty()).then_some(&note) }),
    )
    .await?;
    super::authz::commit(tx).await?;
    let silences = db::active_silences(pool, db::now(pool).await?).await?;
    Ok(Json(item(row, &silences)))
}

/// Silences a rule (every subject, or one) for `minutes` from now: no notification is sent for
/// its alerts that fire or resolve meanwhile. Evaluation and history go on. Needs
/// `alert.respond`; audited as `alert.silence.create`.
#[utoipa::path(post, path = "/api/v1/alerts/silences", tag = "alerts", operation_id = "create_alert_silence",
    request_body = AlertSilenceRequest,
    responses((status = 201, body = AlertSilence), (status = 400, body = ErrorBody), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn create_silence(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    body: Result<Json<AlertSilenceRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<AlertSilence>), ApiError> {
    allow_platform(&p, RESPOND)?;
    let req = json_body(body)?;
    if rule_of(&req.rule).is_none() {
        return Err(bad_request(format!(
            "there is no alert rule {:?}",
            req.rule
        )));
    }
    if req.minutes == 0 || req.minutes > MAX_SILENCE_MINUTES {
        return Err(bad_request(format!(
            "a silence lasts 1 to {MAX_SILENCE_MINUTES} minutes (a week)"
        )));
    }
    let subject = match req
        .subject
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(s) => Some(clean_text("the subject", s, 300)?),
        None => None,
    };
    let note = clean_text("the note", req.note.as_deref().unwrap_or(""), 500)?;
    let pool = store(&state)?;
    let actor = p.actor_ref();
    let mut tx = super::authz::begin(pool).await?;
    let s = db::create_silence(
        &mut *tx,
        &db::NewSilence {
            rule: &req.rule,
            subject: subject.as_deref(),
            minutes: req.minutes,
            note: &note,
            by: &actor,
            by_name: &p.name,
        },
    )
    .await?;
    audit_as(
        &mut tx,
        &p,
        "alert.silence.create",
        &format!("alert-rule:{}", req.rule),
        &json!({ "silenceId": s.id, "subject": subject, "minutes": req.minutes, "note": (!note.is_empty()).then_some(&note) }),
    )
    .await?;
    super::authz::commit(tx).await?;
    Ok((StatusCode::CREATED, Json(silence_view(s))))
}

/// Ends a silence now. Needs `alert.respond`; audited as `alert.silence.end`. 404 when it is
/// not in force (ended or run out).
#[utoipa::path(delete, path = "/api/v1/alerts/silences/{id}", tag = "alerts", operation_id = "end_alert_silence",
    params(("id" = Uuid, Path, description = "Silence id")),
    responses((status = 204), (status = 401, body = ErrorBody), (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn end_silence(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    allow_platform(&p, RESPOND)?;
    let pool = store(&state)?;
    let mut tx = super::authz::begin(pool).await?;
    let s = match db::silence(&mut *tx, id).await {
        Err(apelora_persistence::StoreError::NotFound(_)) => {
            return Err(not_found(&format!("silence {id}")));
        }
        other => other?,
    };
    if !db::end_silence(&mut *tx, id, &p.actor_ref()).await? {
        return Err(not_found(&format!("silence {id} (it has ended)")));
    }
    audit_as(
        &mut tx,
        &p,
        "alert.silence.end",
        &format!("alert-rule:{}", s.rule),
        &json!({ "silenceId": id, "subject": s.subject }),
    )
    .await?;
    super::authz::commit(tx).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- rules ------------------------------------------------------------------------------------

fn params_of(rule: &AlertRule, row: Option<&db::RuleSettingsRow>) -> AlertRuleParams {
    match row {
        Some(r) => AlertRuleParams {
            enabled: r.enabled,
            threshold: r.threshold,
            for_secs: u32::try_from(r.for_secs).unwrap_or(rule.for_secs),
            severity: AlertSeverity::parse(&r.severity).unwrap_or(rule.severity),
        },
        None => defaults_of(rule),
    }
}

fn defaults_of(rule: &AlertRule) -> AlertRuleParams {
    let d = rule.defaults();
    AlertRuleParams {
        enabled: d.enabled,
        threshold: d.threshold,
        for_secs: d.for_secs,
        severity: d.severity,
    }
}

async fn rule_items(pool: &apelora_persistence::Pool) -> Result<Vec<AlertRuleItem>, ApiError> {
    let settings = db::rule_settings(pool).await?;
    let evaluator = db::evaluator(pool).await?;
    let open = db::open_alerts(pool).await?;
    Ok(RULES
        .iter()
        .map(|r| {
            let row = settings.iter().find(|s| s.rule == r.id);
            AlertRuleItem {
                id: r.id.into(),
                title: r.title.into(),
                description: r.description.into(),
                unit: r.unit.into(),
                comparison: r.comparison,
                threshold_editable: r.threshold_editable,
                threshold_min: r.threshold_min,
                threshold_max: r.threshold_max,
                params: params_of(r, row),
                defaults: defaults_of(r),
                source: r.source.into(),
                source_state: evaluator
                    .as_ref()
                    .and_then(|e| e.sources.get(r.id))
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .into(),
                firing: open
                    .iter()
                    .filter(|a| a.rule == r.id && a.state == "firing")
                    .count() as u32,
                runbook: r.runbook.into(),
                updated_at: row.map(|s| rfc3339(s.updated_at)),
                updated_by: row.map(|s| s.updated_by.clone()),
            }
        })
        .collect())
}

/// The built-in rules with their parameters in force, their defaults and what the evaluator's
/// last pass said about each source. Needs `cluster.view`.
#[utoipa::path(get, path = "/api/v1/alerts/rules", tag = "alerts", operation_id = "alert_rules",
    responses((status = 200, body = AlertRuleList), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn rules(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<AlertRuleList>, ApiError> {
    allow_platform(&p, VIEW)?;
    let pool = store(&state)?;
    Ok(Json(AlertRuleList {
        rules: rule_items(pool).await?,
        actions: actions(&p),
    }))
}

/// Changes a rule's parameters (on or off, threshold within the rule's bounds, duration up to a
/// day, severity). Takes effect on the evaluator's next pass. Needs `alert.configure`; audited
/// as `alert.rule.update` with the parameters before and after.
#[utoipa::path(put, path = "/api/v1/alerts/rules/{rule}", tag = "alerts", operation_id = "update_alert_rule",
    params(("rule" = String, Path, description = "Rule id")), request_body = AlertRuleParams,
    responses((status = 200, body = AlertRuleItem), (status = 400, body = ErrorBody), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn update_rule(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(id): Path<String>,
    body: Result<Json<AlertRuleParams>, JsonRejection>,
) -> Result<Json<AlertRuleItem>, ApiError> {
    allow_platform(&p, CONFIGURE)?;
    let Some(rule) = rule_of(&id) else {
        return Err(not_found(&format!("alert rule {id}")));
    };
    let req = json_body(body)?;
    rule.check(&apelora_domain::alerts::RuleSettings {
        enabled: req.enabled,
        threshold: req.threshold,
        for_secs: req.for_secs,
        severity: req.severity,
    })
    .map_err(bad_request)?;
    let pool = store(&state)?;
    let before = params_of(
        rule,
        db::rule_settings(pool)
            .await?
            .iter()
            .find(|s| s.rule == rule.id),
    );
    let mut tx = super::authz::begin(pool).await?;
    db::put_rule_settings(
        &mut *tx,
        rule.id,
        req.enabled,
        req.threshold,
        req.for_secs,
        req.severity.as_str(),
        &p.actor_ref(),
    )
    .await?;
    audit_as(
        &mut tx,
        &p,
        "alert.rule.update",
        &format!("alert-rule:{}", rule.id),
        &json!({ "before": before, "after": req }),
    )
    .await?;
    super::authz::commit(tx).await?;
    rule_items(pool)
        .await?
        .into_iter()
        .find(|r| r.id == rule.id)
        .map(Json)
        .ok_or_else(|| not_found(&format!("alert rule {id}")))
}

// ---- channels ---------------------------------------------------------------------------------

/// Webhook channels with their delivery state. The full URL only for callers who may configure
/// channels. Needs `cluster.view`.
#[utoipa::path(get, path = "/api/v1/alerts/channels", tag = "alerts", operation_id = "alert_channels",
    responses((status = 200, body = AlertChannelList), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn channels(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<AlertChannelList>, ApiError> {
    allow_platform(&p, VIEW)?;
    let pool = store(&state)?;
    let deliveries = db::channel_deliveries(pool).await?;
    let full = allow_platform(&p, CONFIGURE).is_ok();
    Ok(Json(AlertChannelList {
        channels: db::channels(pool)
            .await?
            .into_iter()
            .map(|c| channel_view(c, &deliveries, full))
            .collect(),
        secret_prefix: SECRET_REF_PREFIX.into(),
        actions: actions(&p),
    }))
}

struct CheckedChannel {
    name: String,
    url: String,
    secret_ref: String,
    enabled: bool,
    min_severity: AlertSeverity,
}

fn checked(req: AlertChannelRequest) -> Result<CheckedChannel, ApiError> {
    let name = clean_text("the name", &req.name, 80)?;
    if name.is_empty() {
        return Err(bad_request("the name is empty"));
    }
    let url = req.url.trim().to_owned();
    check_url(&url)?;
    let secret_ref = req.secret_ref.trim().to_owned();
    check_secret_ref(&secret_ref)?;
    Ok(CheckedChannel {
        name,
        url,
        secret_ref,
        enabled: req.enabled,
        min_severity: req.min_severity.unwrap_or(AlertSeverity::Warning),
    })
}

fn input(c: &CheckedChannel) -> db::ChannelInput<'_> {
    db::ChannelInput {
        name: &c.name,
        url: &c.url,
        secret_ref: &c.secret_ref,
        enabled: c.enabled,
        min_severity: c.min_severity.as_str(),
    }
}

/// What a channel's audit row says about it: never the URL's path or query (they may carry a
/// receiver's token), never a secret (there is none here).
fn channel_detail(c: &db::ChannelRow) -> Value {
    json!({ "id": c.id, "host": host_of(&c.url), "secretRef": c.secret_ref, "enabled": c.enabled, "minSeverity": c.min_severity })
}

fn conflict_or(e: apelora_persistence::StoreError) -> ApiError {
    match e {
        apelora_persistence::StoreError::Conflict(m) => {
            ApiError::new(StatusCode::CONFLICT, "name_taken", m)
        }
        other => other.into(),
    }
}

/// Adds a webhook channel. HTTPS only; the signing secret is named, never sent. Needs
/// `alert.configure`; audited as `alert.channel.create` (host only, not the URL).
#[utoipa::path(post, path = "/api/v1/alerts/channels", tag = "alerts", operation_id = "create_alert_channel",
    request_body = AlertChannelRequest,
    responses((status = 201, body = AlertChannel), (status = 400, body = ErrorBody), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 409, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn create_channel(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    body: Result<Json<AlertChannelRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<AlertChannel>), ApiError> {
    allow_platform(&p, CONFIGURE)?;
    let c = checked(json_body(body)?)?;
    let pool = store(&state)?;
    let mut tx = super::authz::begin(pool).await?;
    let row = match db::create_channel(&mut *tx, &input(&c), &p.actor_ref()).await {
        Ok(r) => r,
        Err(e) => return Err(conflict_or(e)),
    };
    audit_as(
        &mut tx,
        &p,
        "alert.channel.create",
        &format!("alert-channel:{}", row.name),
        &channel_detail(&row),
    )
    .await?;
    super::authz::commit(tx).await?;
    Ok((StatusCode::CREATED, Json(channel_view(row, &[], true))))
}

/// Changes a webhook channel. Needs `alert.configure`; audited as `alert.channel.update`.
#[utoipa::path(put, path = "/api/v1/alerts/channels/{id}", tag = "alerts", operation_id = "update_alert_channel",
    params(("id" = Uuid, Path, description = "Channel id")), request_body = AlertChannelRequest,
    responses((status = 200, body = AlertChannel), (status = 400, body = ErrorBody), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody),
        (status = 503, body = ErrorBody)))]
pub(super) async fn update_channel(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(id): Path<Uuid>,
    body: Result<Json<AlertChannelRequest>, JsonRejection>,
) -> Result<Json<AlertChannel>, ApiError> {
    allow_platform(&p, CONFIGURE)?;
    let c = checked(json_body(body)?)?;
    let pool = store(&state)?;
    let mut tx = super::authz::begin(pool).await?;
    let row = match db::update_channel(&mut *tx, id, &input(&c)).await {
        Ok(r) => r,
        Err(apelora_persistence::StoreError::NotFound(_)) => {
            return Err(not_found(&format!("channel {id}")));
        }
        Err(e) => return Err(conflict_or(e)),
    };
    audit_as(
        &mut tx,
        &p,
        "alert.channel.update",
        &format!("alert-channel:{}", row.name),
        &channel_detail(&row),
    )
    .await?;
    super::authz::commit(tx).await?;
    let deliveries = db::channel_deliveries(pool).await?;
    Ok(Json(channel_view(row, &deliveries, true)))
}

/// Deletes a webhook channel; messages still waiting for it are dropped by the relay. Needs
/// `alert.configure`; audited as `alert.channel.delete`.
#[utoipa::path(delete, path = "/api/v1/alerts/channels/{id}", tag = "alerts", operation_id = "delete_alert_channel",
    params(("id" = Uuid, Path, description = "Channel id")),
    responses((status = 204), (status = 401, body = ErrorBody), (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn delete_channel(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    allow_platform(&p, CONFIGURE)?;
    let pool = store(&state)?;
    let mut tx = super::authz::begin(pool).await?;
    let row = match db::channel(&mut *tx, id).await {
        Err(apelora_persistence::StoreError::NotFound(_)) => {
            return Err(not_found(&format!("channel {id}")));
        }
        other => other?,
    };
    db::delete_channel(&mut *tx, id).await?;
    audit_as(
        &mut tx,
        &p,
        "alert.channel.delete",
        &format!("alert-channel:{}", row.name),
        &channel_detail(&row),
    )
    .await?;
    super::authz::commit(tx).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Queues a test notification for one channel through the same outbox path as real alerts; the
/// channel's delivery state shows the outcome within seconds. Needs `alert.configure`; audited as
/// `alert.channel.test` in the same transaction as the message.
#[utoipa::path(post, path = "/api/v1/alerts/channels/{id}/test", tag = "alerts", operation_id = "test_alert_channel",
    params(("id" = Uuid, Path, description = "Channel id")),
    responses((status = 202, body = AlertChannelTest), (status = 401, body = ErrorBody), (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody), (status = 409, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn test_channel(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(id): Path<Uuid>,
) -> Result<(StatusCode, Json<AlertChannelTest>), ApiError> {
    allow_platform(&p, CONFIGURE)?;
    let pool = store(&state)?;
    let mut tx = super::authz::begin(pool).await?;
    let c = match db::channel(&mut *tx, id).await {
        Err(apelora_persistence::StoreError::NotFound(_)) => {
            return Err(not_found(&format!("channel {id}")));
        }
        other => other?,
    };
    if !c.enabled {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "channel_disabled",
            "the channel is turned off; turn it on to send a test",
        ));
    }
    let place = apelora_metrics::place();
    let now = rfc3339(OffsetDateTime::now_utc());
    let body = json!({
        "version": 1,
        "event": "test",
        "alert": {
            "id": null, "rule": "test", "title": "Test notification", "subject": c.name,
            "severity": "info", "summary": format!("A test notification from Apelora, sent by {}", p.name),
            "value": 0, "threshold": 0, "unit": "", "startedAt": now, "firedAt": now, "resolvedAt": null,
        },
        "installation": { "region": place.region, "site": place.site, "sector": place.sector },
    });
    let message_id = db::notify(&mut tx, &[id], &body)
        .await?
        .first()
        .copied()
        .unwrap_or_default();
    audit_as(
        &mut tx,
        &p,
        "alert.channel.test",
        &format!("alert-channel:{}", c.name),
        &json!({ "id": id, "messageId": message_id }),
    )
    .await?;
    super::authz::commit(tx).await?;
    Ok((StatusCode::ACCEPTED, Json(AlertChannelTest { message_id })))
}

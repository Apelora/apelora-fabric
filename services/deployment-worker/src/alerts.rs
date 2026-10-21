// SPDX-License-Identifier: AGPL-3.0-only
//! The alert evaluator (docs/design/alerts.md).
//!
//! Every `APELORA_ALERT_EVALUATE_SECS` (default 30; 0 turns it off) the worker holding the
//! `alert-evaluator` leader lease reads what Apelora already knows (jobs, worker heartbeats,
//! leader leases, the supervisor's actions, replica health, derived application status, Incus,
//! certificates, volume schedules, the outbox, the restore drill's record), evaluates every
//! built-in rule (`apelora_domain::alerts::RULES`, with the operator's changes) and moves each
//! (rule, subject) between pending, firing and resolved with hysteresis.
//!
//! All facts are gathered first; then one transaction, fenced by the lease, writes every state
//! change, one outbox message (topic `alert`) per enabled channel for each alert that fires or
//! resolves outside a silence, and the evaluator's status row. A leader that lost its lease
//! writes nothing. When a rule's source cannot be read, its open alerts are left as they are.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use apelora_domain::alerts::{
    AlertRule, AlertSeverity, Findings, Observation, OpenAlert, RULES, RuleSettings, Transition,
    silence_covers, transition,
};
use apelora_domain::lifecycle::DeploymentStatus;
use apelora_incus_adapter::Provider;
use apelora_persistence::alerts as db;
use apelora_persistence::{Leadership, Pool, StoreError, begin_fenced};
use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::sync::watch;
use uuid::Uuid;

use crate::supervisor::{AbortOnDrop, LEADER_LEASE_SECS, LEADER_RENEW_EVERY, LeaderLease};

/// The leader lease the evaluator runs under.
pub const LEADER_LEASE: &str = "alert-evaluator";

/// How the evaluator runs on this worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub every: Duration,
    /// The restore drill's `restore-drill-last-verified.json`, where this worker can read it.
    pub restore_drill_record: Option<PathBuf>,
}

/// `APELORA_ALERT_EVALUATE_SECS` (default 30, 5–3600, `0` = off) and
/// `APELORA_RESTORE_DRILL_RECORD` (a file path; unset = the restore rule is not configured).
pub fn config_from_env(get: impl Fn(&str) -> Option<String>) -> Result<Option<Config>, String> {
    let every = match get("APELORA_ALERT_EVALUATE_SECS")
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
    {
        None => 30,
        Some(v) => match v.parse::<u64>() {
            Ok(0) => return Ok(None),
            Ok(n) if (5..=3600).contains(&n) => n,
            _ => {
                return Err(format!(
                    "APELORA_ALERT_EVALUATE_SECS={v:?}: use 0 (off) or 5 to 3600 seconds"
                ));
            }
        },
    };
    let record = get("APELORA_RESTORE_DRILL_RECORD")
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    Ok(Some(Config {
        every: Duration::from_secs(every),
        restore_drill_record: record,
    }))
}

// ---- facts ------------------------------------------------------------------------------------

fn clip(s: &str, max: usize) -> String {
    let one_line = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= max {
        one_line
    } else {
        format!("{}…", one_line.chars().take(max).collect::<String>())
    }
}

fn plural(n: f64, one: &str, many: &str) -> String {
    format!("{} {}", n, if n == 1.0 { one } else { many })
}

fn observed(r: Result<Vec<Observation>, StoreError>) -> Findings {
    match r {
        Ok(o) => Findings::Observed(o),
        Err(e) => Findings::Unavailable(format!("the database could not be read: {e}")),
    }
}

async fn job_failures(pool: &Pool) -> Result<Vec<Observation>, StoreError> {
    let rows = db::failed_jobs_by_kind(pool, 15.0 * 60.0).await?;
    Ok(rows
        .into_iter()
        .map(|(kind, n, last)| {
            let n = n as f64;
            let mut summary = format!(
                "{} failed in the last 15 minutes",
                plural(n, &format!("{kind} job"), &format!("{kind} jobs"))
            );
            if let Some(e) = last.filter(|e| !e.trim().is_empty()) {
                summary.push_str(&format!("; the latest: {}", clip(&e, 160)));
            }
            Observation::new(kind, n, summary)
        })
        .collect())
}

async fn queue(pool: &Pool) -> Result<(Vec<Observation>, Vec<Observation>), StoreError> {
    let q = apelora_persistence::monitoring::queue_summary(pool).await?;
    let wait = q.oldest_runnable_secs.floor();
    Ok((
        vec![Observation::new(
            "queue",
            wait,
            format!("the oldest job that could run has waited {wait} s"),
        )],
        vec![Observation::new(
            "queue",
            q.expired_leases as f64,
            format!(
                "{} whose worker stopped renewing the lease",
                plural(q.expired_leases as f64, "running job", "running jobs")
            ),
        )],
    ))
}

async fn unfenced_members(pool: &Pool) -> Result<Vec<Observation>, StoreError> {
    Ok(apelora_persistence::recovery::unfenced(pool)
        .await?
        .into_iter()
        .map(|(member, waiting, secs)| {
            Observation::new(
                &member,
                1.0,
                format!(
                    "member {member} failed {} s ago and is not fenced; {} wait to restart on another server",
                    secs.floor(),
                    plural(f64::from(waiting), "module", "modules")
                ),
            )
        })
        .collect())
}

async fn silent_workers(pool: &Pool) -> Result<Vec<Observation>, StoreError> {
    Ok(apelora_persistence::status::workers(pool)
        .await?
        .into_iter()
        .filter(|w| w.stopped_at.is_none())
        .map(|w| {
            let secs = w.seconds_since_seen.max(0) as f64;
            Observation::new(
                &w.id,
                secs,
                format!(
                    "worker {} on {} last wrote its heartbeat {secs} s ago",
                    w.id, w.host
                ),
            )
        })
        .collect())
}

async fn missing_leaders(pool: &Pool) -> Result<Vec<Observation>, StoreError> {
    let rows = db::leader_leases(pool).await?;
    Ok(rows
        .into_iter()
        .map(|(name, holder, since)| {
            let secs = since.max(0.0);
            Observation::new(
                &name,
                secs,
                if secs > 0.0 {
                    format!("nobody has held {name} for {secs} s (last holder {holder})")
                } else {
                    format!("{name} is held by {holder}")
                },
            )
        })
        .collect())
}

async fn supervisor_actions(pool: &Pool) -> Result<Vec<Observation>, StoreError> {
    let n = db::supervisor_actions(pool, 3600.0).await? as f64;
    Ok(vec![Observation::new(
        "supervisor",
        n,
        format!(
            "the supervisor took {} in the last hour",
            plural(n, "replica action", "replica actions")
        ),
    )])
}

async fn unhealthy_replicas(pool: &Pool) -> Result<Vec<Observation>, StoreError> {
    let rows = db::unhealthy_instances(pool).await?;
    Ok(rows
        .into_iter()
        .map(|(project, instance, detail)| {
            let why = if detail.trim().is_empty() {
                String::new()
            } else {
                format!(": {}", clip(&detail, 160))
            };
            Observation::new(
                format!("{project}/{instance}"),
                1.0,
                format!("{project}/{instance} is unhealthy{why}"),
            )
        })
        .collect())
}

async fn volume_schedules(pool: &Pool) -> Result<(Vec<Observation>, Vec<Observation>), StoreError> {
    let rows = db::volume_schedules(pool).await?;
    let mut failed = Vec::new();
    let mut age = Vec::new();
    for v in rows {
        let (project, name, snapshot_error, backup_error, every, since) = (
            v.project,
            v.incus_name,
            v.snapshot_error,
            v.backup_error,
            v.backup_every_minutes,
            v.backup_age_secs,
        );
        let volume = format!("{project}/{name}");
        for (kind, error) in [("snapshot", snapshot_error), ("backup", backup_error)] {
            if let Some(e) = error {
                failed.push(Observation::new(
                    format!("{volume} {kind}"),
                    1.0,
                    format!(
                        "the last scheduled {kind} of {volume} failed: {}",
                        clip(&e, 160)
                    ),
                ));
            }
        }
        if let Some(minutes) = every.filter(|m| *m > 0) {
            let periods = (since.max(0.0) / (f64::from(minutes) * 60.0) * 10.0).floor() / 10.0;
            age.push(Observation::new(
                volume.clone(),
                periods,
                format!(
                    "the newest backup of {volume} is {periods} schedule periods old (every {minutes} min)"
                ),
            ));
        }
    }
    Ok((failed, age))
}

async fn outbox_age(pool: &Pool) -> Result<Vec<Observation>, StoreError> {
    let b = apelora_persistence::outbox::backlog(pool).await?;
    let age = b.oldest_age_secs.floor();
    Ok(vec![Observation::new(
        "outbox",
        age,
        format!(
            "{} waiting, {} failing; the oldest has waited {age} s",
            plural(b.pending as f64, "message", "messages"),
            b.failing
        ),
    )])
}

fn certificates(provider: &Provider, now: OffsetDateTime) -> Findings {
    let Some(e) = provider.certificate_expiry() else {
        return Findings::NotConfigured(
            "the worker uses the mock provider, which has no certificates".into(),
        );
    };
    let days = |t: i64| ((t - now.unix_timestamp()) as f64 / 86_400.0).floor();
    Findings::Observed(
        [("deploy", e.client), ("server", e.server)]
            .into_iter()
            .map(|(which, at)| {
                let d = days(at);
                let what = if which == "deploy" {
                    "Apelora's deploy certificate (its Incus identity)"
                } else {
                    "the pinned Incus server certificate"
                };
                Observation::new(
                    which,
                    d,
                    if d < 0.0 {
                        format!("{what} has expired")
                    } else {
                        format!("{what} expires in {}", plural(d, "day", "days"))
                    },
                )
            })
            .collect(),
    )
}

/// Days since the last verified restore, from the drill's record.
pub fn restore_drill(path: Option<&std::path::Path>, now: OffsetDateTime) -> Findings {
    let Some(path) = path else {
        return Findings::NotConfigured(
            "set APELORA_RESTORE_DRILL_RECORD on the worker to the drill's restore-drill-last-verified.json".into(),
        );
    };
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Findings::Observed(vec![Observation::new(
                "apelora-database",
                9999.0,
                "no verified restore of Apelora's database is recorded yet",
            )]);
        }
        Err(e) => return Findings::Unavailable(format!("the drill's record cannot be read: {e}")),
    };
    let at = serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|v| v["drilled_at"].as_str().map(str::to_owned))
        .and_then(|s| OffsetDateTime::parse(&s, &Rfc3339).ok());
    match at {
        Some(at) => {
            let days = ((now - at).whole_seconds().max(0) as f64 / 86_400.0).floor();
            Findings::Observed(vec![Observation::new(
                "apelora-database",
                days,
                format!(
                    "the last verified restore of Apelora's database was {} ago",
                    plural(days, "day", "days")
                ),
            )])
        }
        None => Findings::Unavailable(
            "the drill's record has no readable drilled_at (expected format apelora-restore-drill/1)".into(),
        ),
    }
}

/// What every rule's source says now. Incus is read once (members, then instances).
pub async fn gather(
    pool: &Pool,
    provider: &Provider,
    cfg: &Config,
    now: OffsetDateTime,
) -> BTreeMap<&'static str, Findings> {
    let mut out: BTreeMap<&'static str, Findings> = BTreeMap::new();
    out.insert("job-failures", observed(job_failures(pool).await));
    match queue(pool).await {
        Ok((wait, expired)) => {
            out.insert("job-queue-wait", Findings::Observed(wait));
            out.insert("job-leases-expired", Findings::Observed(expired));
        }
        Err(e) => {
            let why = format!("the database could not be read: {e}");
            out.insert("job-queue-wait", Findings::Unavailable(why.clone()));
            out.insert("job-leases-expired", Findings::Unavailable(why));
        }
    }
    out.insert("worker-silent", observed(silent_workers(pool).await));
    out.insert("leader-missing", observed(missing_leaders(pool).await));
    out.insert(
        "supervisor-actions",
        observed(supervisor_actions(pool).await),
    );
    out.insert(
        "replica-unhealthy",
        observed(unhealthy_replicas(pool).await),
    );

    let members = provider.cluster_members().await;
    let instances = match &members {
        Ok(_) => provider.instances().await,
        Err(e) => Err(apelora_incus_adapter::AdapterError::Connectivity(
            e.to_string(),
        )),
    };
    let incus_error = members
        .as_ref()
        .err()
        .map(ToString::to_string)
        .or_else(|| instances.as_ref().err().map(ToString::to_string));
    out.insert(
        "incus-unreachable",
        Findings::Observed(vec![match &incus_error {
            Some(e) => Observation::new(
                "incus",
                1.0,
                format!("Incus cannot be read: {}", clip(e, 200)),
            ),
            None => Observation::new("incus", 0.0, "Incus answers"),
        }]),
    );
    let unavailable = |what: &str| {
        Findings::Unavailable(format!(
            "{what} cannot be read from Incus: {}",
            clip(incus_error.as_deref().unwrap_or("-"), 200)
        ))
    };
    out.insert(
        "member-offline",
        match &members {
            Ok(list) => Findings::Observed(
                list.iter()
                    .filter(|m| m.status != "Online")
                    .map(|m| {
                        let why = if m.message.trim().is_empty() {
                            String::new()
                        } else {
                            format!(" ({})", clip(&m.message, 120))
                        };
                        Observation::new(
                            &m.name,
                            1.0,
                            format!("member {} is {}{why}", m.name, m.status),
                        )
                    })
                    .collect(),
            ),
            Err(_) => unavailable("the member list"),
        },
    );
    out.insert(
        "application-status",
        match &instances {
            Ok(list) => match apelora_persistence::lifecycle::derived_statuses(pool, Some(list)).await {
                Ok(statuses) => Findings::Observed(
                    statuses
                        .into_iter()
                        .filter_map(|(name, status)| {
                            let word = match status {
                                DeploymentStatus::Failed => "failed: its last start, stop or restart failed",
                                DeploymentStatus::Degraded => {
                                    "degraded: fewer instances run than it should have, or some are unhealthy"
                                }
                                _ => return None,
                            };
                            Some(Observation::new(&name, 1.0, format!("application {name} is {word}")))
                        })
                        .collect(),
                ),
                Err(e) => Findings::Unavailable(format!("the database could not be read: {e}")),
            },
            Err(_) => unavailable("the instance list"),
        },
    );
    out.insert("member-unfenced", observed(unfenced_members(pool).await));
    out.insert("certificate-expiry", certificates(provider, now));
    match volume_schedules(pool).await {
        Ok((failed, age)) => {
            out.insert("backup-failed", Findings::Observed(failed));
            out.insert("backup-age", Findings::Observed(age));
        }
        Err(e) => {
            let why = format!("the database could not be read: {e}");
            out.insert("backup-failed", Findings::Unavailable(why.clone()));
            out.insert("backup-age", Findings::Unavailable(why));
        }
    }
    out.insert("outbox-stuck", observed(outbox_age(pool).await));
    out.insert(
        "restore-drill-age",
        restore_drill(cfg.restore_drill_record.as_deref(), now),
    );
    out
}

// ---- one pass ---------------------------------------------------------------------------------

/// What one pass did (for tests and logs).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Pass {
    /// (rule, subject) that fired, resolved, or were opened as pending.
    pub fired: Vec<(String, String)>,
    pub resolved: Vec<(String, String)>,
    pub pending: Vec<(String, String)>,
    /// Outbox messages written.
    pub notifications: usize,
    /// Each rule's source: ok, not-configured, unavailable, disabled.
    pub sources: BTreeMap<String, String>,
    /// Firing alerts after the pass, by severity.
    pub firing: BTreeMap<String, usize>,
}

fn rfc3339(t: Option<OffsetDateTime>) -> Value {
    t.and_then(|t| t.format(&Rfc3339).ok())
        .map_or(Value::Null, Value::String)
}

fn secs(t: OffsetDateTime) -> f64 {
    t.unix_timestamp() as f64 + f64::from(t.nanosecond()) / 1e9
}

/// The notification body for an alert row (the webhook sink adds the message id).
pub fn message(event: &str, rule: &AlertRule, a: &db::AlertRow) -> Value {
    let place = apelora_metrics::place();
    json!({
        "version": 1,
        "event": event,
        "alert": {
            "id": a.id,
            "rule": a.rule,
            "title": rule.title,
            "subject": a.subject,
            "severity": a.severity,
            "summary": a.summary,
            "value": a.value,
            "threshold": a.threshold,
            "unit": rule.unit,
            "startedAt": rfc3339(Some(a.started_at)),
            "firedAt": rfc3339(a.fired_at),
            "resolvedAt": rfc3339(a.resolved_at),
        },
        "installation": { "region": place.region, "site": place.site, "sector": place.sector },
    })
}

/// The settings in force for every rule.
fn effective(rows: &[db::RuleSettingsRow]) -> BTreeMap<&'static str, RuleSettings> {
    RULES
        .iter()
        .map(|r| {
            let s = rows
                .iter()
                .find(|x| x.rule == r.id)
                .map_or(r.defaults(), |x| RuleSettings {
                    enabled: x.enabled,
                    threshold: x.threshold,
                    for_secs: u32::try_from(x.for_secs).unwrap_or(r.for_secs),
                    severity: AlertSeverity::parse(&x.severity).unwrap_or(r.severity),
                });
            (r.id, s)
        })
        .collect()
}

/// One evaluation as the leader (`lease`; `None` in tests): gathers, decides and writes.
pub async fn evaluate_once(
    pool: &Pool,
    provider: &Provider,
    cfg: &Config,
    lease: Option<&Leadership>,
    worker: &str,
) -> Result<Pass, StoreError> {
    let started = Instant::now();
    let now = db::now(pool).await?;
    let findings = gather(pool, provider, cfg, now).await;
    let settings = effective(&db::rule_settings(pool).await?);
    let silences = db::active_silences(pool, now).await?;
    let channels: Vec<(Uuid, AlertSeverity)> = db::channels(pool)
        .await?
        .into_iter()
        .filter(|c| c.enabled)
        .map(|c| {
            (
                c.id,
                AlertSeverity::parse(&c.min_severity).unwrap_or(AlertSeverity::Warning),
            )
        })
        .collect();
    let mut tx = begin_fenced(pool, lease).await?;
    let open = db::open_alerts(&mut *tx).await?;
    let mut pass = Pass::default();
    let silenced = |rule: &str, subject: &str| {
        silences
            .iter()
            .any(|s| silence_covers(&s.rule, s.subject.as_deref(), rule, subject))
    };
    for rule in RULES {
        let s = settings[rule.id];
        let mine: BTreeMap<&str, &db::AlertRow> = open
            .iter()
            .filter(|a| a.rule == rule.id)
            .map(|a| (a.subject.as_str(), a))
            .collect();
        let found = findings
            .get(rule.id)
            .cloned()
            .unwrap_or_else(|| Findings::NotConfigured("no source".into()));
        let source = if s.enabled {
            found.source_state()
        } else {
            "disabled"
        };
        pass.sources.insert(rule.id.to_owned(), source.to_owned());
        let observations: BTreeMap<String, Observation> = match (&found, s.enabled) {
            // Not readable now: keep what is open exactly as it is.
            (Findings::Unavailable(why), true) => {
                tracing::debug!(rule = rule.id, reason = %why, "alert: source unavailable; open alerts kept");
                continue;
            }
            (Findings::Observed(list), true) => list
                .iter()
                .map(|o| (o.subject.clone(), o.clone()))
                .collect(),
            // Turned off, or nothing to watch: whatever is open clears.
            _ => BTreeMap::new(),
        };
        let subjects: BTreeSet<String> = mine
            .keys()
            .map(|k| (*k).to_owned())
            .chain(observations.keys().cloned())
            .collect();
        for subject in subjects {
            let o = observations.get(&subject);
            let holds = s.enabled && o.is_some_and(|o| rule.holds(o.value, s.threshold));
            let current = mine.get(subject.as_str()).copied();
            let state = current.map(|a| OpenAlert {
                firing: a.state == "firing",
                started_at: secs(a.started_at),
                cleared_at: a.cleared_at.map(secs),
            });
            // Disabled rules resolve at once, whatever their duration.
            let wait = if s.enabled { s.for_secs } else { 0 };
            let step = transition(state.as_ref(), holds, secs(now), wait);
            let (value, summary) = o.map_or((0.0, String::new()), |o| (o.value, o.summary.clone()));
            let seen = db::Seen {
                severity: s.severity.as_str(),
                summary: &summary,
                value,
                threshold: s.threshold,
            };
            let row = match (step, current) {
                (Transition::None, _) => None,
                (Transition::OpenPending | Transition::OpenFiring, _) => {
                    let firing = step == Transition::OpenFiring;
                    let row = db::open_alert(
                        &mut tx,
                        &db::NewAlert {
                            rule: rule.id,
                            subject: &subject,
                            severity: s.severity.as_str(),
                            firing,
                            summary: &summary,
                            value,
                            threshold: s.threshold,
                        },
                        now,
                    )
                    .await?;
                    if !firing {
                        pass.pending.push((rule.id.to_owned(), subject.clone()));
                    }
                    Some(row)
                }
                (Transition::Fire | Transition::Refresh, Some(a)) => {
                    Some(db::refresh(&mut tx, a.id, &seen, step == Transition::Fire, now).await?)
                }
                (Transition::Drop, Some(a)) => {
                    db::drop_pending(&mut tx, a.id).await?;
                    None
                }
                (Transition::Clearing, Some(a)) => {
                    db::clearing(&mut tx, a.id, now).await?;
                    None
                }
                (Transition::Resolve, Some(a)) => Some(db::resolve(&mut tx, a.id, now).await?),
                // A transition on an alert that is not open cannot happen; nothing to write.
                (_, None) => None,
            };
            if !step.notifies() {
                continue;
            }
            let Some(row) = row else { continue };
            let event = if step == Transition::Resolve {
                "resolved"
            } else {
                "fired"
            };
            if event == "fired" {
                tracing::warn!(rule = rule.id, subject = %subject, severity = %row.severity, summary = %row.summary, "alert: firing");
                pass.fired.push((rule.id.to_owned(), subject.clone()));
            } else {
                tracing::info!(rule = rule.id, subject = %subject, "alert: resolved");
                pass.resolved.push((rule.id.to_owned(), subject.clone()));
            }
            if silenced(rule.id, &subject) {
                tracing::info!(rule = rule.id, subject = %subject, event, "alert: silenced; no notification sent");
                continue;
            }
            let severity = AlertSeverity::parse(&row.severity).unwrap_or(rule.severity);
            let to: Vec<Uuid> = channels
                .iter()
                .filter(|(_, min)| severity.meets(*min))
                .map(|(id, _)| *id)
                .collect();
            pass.notifications += db::notify(&mut tx, &to, &message(event, rule, &row))
                .await?
                .len();
        }
    }
    db::prune_resolved(&mut tx, db::KEEP_RESOLVED_SECS, now).await?;
    for a in db::open_alerts(&mut *tx).await? {
        if a.state == "firing" {
            *pass.firing.entry(a.severity).or_default() += 1;
        }
    }
    let every = u32::try_from(cfg.every.as_secs()).unwrap_or(u32::MAX);
    db::record_pass(
        &mut *tx,
        now,
        started.elapsed().as_secs_f64(),
        worker,
        every,
        &serde_json::to_value(&pass.sources).unwrap_or_default(),
    )
    .await?;
    tx.commit().await?;
    apelora_metrics::ALERTS_FIRING.replace(AlertSeverity::ALL.iter().map(|s| {
        (
            vec![s.as_str()],
            pass.firing.get(s.as_str()).copied().unwrap_or(0) as f64,
        )
    }));
    Ok(pass)
}

/// Evaluates every `cfg.every` while this worker (`holder`) holds the `alert-evaluator` lease,
/// until the task is dropped. The lease is renewed on its own task.
pub async fn evaluate(pool: Pool, provider: Provider, cfg: Arc<Config>, holder: String) {
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
        every_secs = cfg.every.as_secs(),
        restore_drill_record = cfg.restore_drill_record.is_some(),
        "alert evaluator started"
    );
    loop {
        let current = rx.borrow().clone();
        match current {
            None => apelora_metrics::ALERTS_FIRING.replace([]),
            Some(l) => {
                let started = Instant::now();
                let result = evaluate_once(&pool, &provider, &cfg, Some(&l), &holder).await;
                apelora_metrics::ALERT_EVALUATION.observe(
                    &[if result.is_ok() { "ok" } else { "error" }],
                    started.elapsed().as_secs_f64(),
                );
                match result {
                    Ok(p) => tracing::debug!(
                        fired = p.fired.len(),
                        resolved = p.resolved.len(),
                        notifications = p.notifications,
                        "alert: pass"
                    ),
                    Err(StoreError::Conflict(msg)) => {
                        tracing::warn!(%msg, "alert: pass stopped: the evaluator lease is no longer held, or an alert changed meanwhile")
                    }
                    Err(e) => tracing::error!(error = %e, "alert: pass failed"),
                }
            }
        }
        tokio::time::sleep(cfg.every).await;
    }
}

#[cfg(test)]
mod tests {
    use apelora_incus_adapter::MockIncus;
    use apelora_persistence::{acquire_leadership, enqueue_job};

    use super::*;

    fn cfg() -> Config {
        Config {
            every: Duration::from_secs(30),
            restore_drill_record: None,
        }
    }

    #[test]
    fn the_evaluator_is_configured_by_setting() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == k)
                    .map(|(_, v)| (*v).to_owned())
            }
        };
        assert_eq!(
            config_from_env(env(&[])),
            Ok(Some(Config {
                every: Duration::from_secs(30),
                restore_drill_record: None
            }))
        );
        assert_eq!(
            config_from_env(env(&[("APELORA_ALERT_EVALUATE_SECS", "0")])),
            Ok(None)
        );
        assert!(config_from_env(env(&[("APELORA_ALERT_EVALUATE_SECS", "1")])).is_err());
        assert!(config_from_env(env(&[("APELORA_ALERT_EVALUATE_SECS", "soon")])).is_err());
        let c = config_from_env(env(&[
            ("APELORA_ALERT_EVALUATE_SECS", "10"),
            (
                "APELORA_RESTORE_DRILL_RECORD",
                "/srv/backups/restore-drill-last-verified.json",
            ),
        ]))
        .expect("valid")
        .expect("on");
        assert_eq!(c.every, Duration::from_secs(10));
        assert!(c.restore_drill_record.is_some());
    }

    #[test]
    fn the_restore_drill_record_is_read_honestly() {
        let now = OffsetDateTime::parse("2026-10-21T12:00:00Z", &Rfc3339).expect("time");
        assert!(matches!(
            restore_drill(None, now),
            Findings::NotConfigured(_)
        ));
        let dir = std::env::temp_dir().join(format!("apelora-drill-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("restore-drill-last-verified.json");
        match restore_drill(Some(&path), now) {
            Findings::Observed(o) => assert_eq!(o[0].value, 9999.0, "none recorded yet"),
            other => panic!("{other:?}"),
        }
        std::fs::write(&path, r#"{"format":"apelora-restore-drill/1","drilled_at":"2026-10-01T03:00:00Z","verdict":"pass"}"#).expect("write");
        match restore_drill(Some(&path), now) {
            Findings::Observed(o) => assert_eq!(
                (o[0].subject.as_str(), o[0].value),
                ("apelora-database", 20.0)
            ),
            other => panic!("{other:?}"),
        }
        std::fs::write(&path, "{}").expect("write");
        assert!(matches!(
            restore_drill(Some(&path), now),
            Findings::Unavailable(_)
        ));
        std::fs::remove_dir_all(&dir).expect("clean");
    }

    async fn fail_a_job(pool: &Pool, kind: &str) {
        let id = enqueue_job(pool, kind, None, &json!({}), None)
            .await
            .expect("job")
            .id();
        sqlx::query(
            "UPDATE jobs SET status = 'failed', last_error = 'incus said no' WHERE id = $1",
        )
        .bind(id)
        .execute(pool)
        .await
        .expect("fail");
    }

    async fn channel(pool: &Pool, name: &str, min: &str) -> Uuid {
        db::create_channel(
            pool,
            &db::ChannelInput {
                name,
                url: "https://hooks.example/x",
                secret_ref: "APELORA_ALERT_SECRET_TEST",
                enabled: true,
                min_severity: min,
            },
            "user:1",
        )
        .await
        .expect("channel")
        .id
    }

    async fn alert_messages(pool: &Pool) -> Vec<Value> {
        apelora_persistence::outbox::due(pool, 100)
            .await
            .expect("due")
            .into_iter()
            .filter(|m| m.topic == db::ALERT_TOPIC)
            .map(|m| m.payload)
            .collect()
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn a_failing_job_kind_fires_once_notifies_each_channel_and_resolves(pool: Pool) {
        let provider = Provider::Mock(MockIncus::default());
        let warn = channel(&pool, "ops", "warning").await;
        let _critical_only = channel(&pool, "pager", "critical").await;
        fail_a_job(&pool, "instance.create").await;
        fail_a_job(&pool, "instance.create").await;

        let p = evaluate_once(&pool, &provider, &cfg(), None, "worker-a")
            .await
            .expect("pass");
        assert_eq!(
            p.fired,
            vec![("job-failures".to_owned(), "instance.create".to_owned())]
        );
        assert_eq!(p.sources["job-failures"], "ok");
        assert_eq!(
            p.sources["certificate-expiry"], "not-configured",
            "mock: no certificates"
        );
        assert_eq!(p.sources["restore-drill-age"], "not-configured");
        assert_eq!(p.firing.get("warning"), Some(&1));
        // One message, to the channel whose minimum severity it meets.
        let msgs = alert_messages(&pool).await;
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["channel"], json!(warn.to_string()));
        assert_eq!(msgs[0]["event"], "fired");
        assert_eq!(msgs[0]["alert"]["rule"], "job-failures");
        assert_eq!(msgs[0]["alert"]["value"], 2.0);
        assert!(
            msgs[0]["alert"]["summary"]
                .as_str()
                .expect("summary")
                .contains("incus said no")
        );

        // Deduplicated: the next pass refreshes the same alert, no new message.
        let p = evaluate_once(&pool, &provider, &cfg(), None, "worker-a")
            .await
            .expect("pass");
        assert!(p.fired.is_empty());
        assert_eq!(db::open_alerts(&pool).await.expect("open").len(), 1);
        assert_eq!(alert_messages(&pool).await.len(), 1);

        // The failures age out of the window: resolved (duration 0), and notified.
        sqlx::query("UPDATE jobs SET updated_at = now() - interval '1 hour'")
            .execute(&pool)
            .await
            .expect("age");
        let p = evaluate_once(&pool, &provider, &cfg(), None, "worker-a")
            .await
            .expect("pass");
        assert_eq!(
            p.resolved,
            vec![("job-failures".to_owned(), "instance.create".to_owned())]
        );
        let msgs = alert_messages(&pool).await;
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[1]["event"], "resolved");
        assert!(db::open_alerts(&pool).await.expect("open").is_empty());
        assert_eq!(
            db::history(&pool, None, 10).await.expect("history").len(),
            1
        );
        let e = db::evaluator(&pool).await.expect("read").expect("row");
        assert_eq!(e.worker, "worker-a");
        assert_eq!(e.sources["job-failures"], "ok");
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn a_condition_must_hold_for_the_rule_s_duration_and_silences_hold_back_messages(
        pool: Pool,
    ) {
        let provider = Provider::Mock(MockIncus::default());
        channel(&pool, "ops", "info").await;
        // The supervisor rule needs 4 actions in an hour; give it a duration of an hour.
        db::put_rule_settings(
            &pool,
            "supervisor-actions",
            true,
            2.0,
            3600,
            "critical",
            "user:1",
        )
        .await
        .expect("settings");
        for i in 0..3 {
            apelora_persistence::audit(
                &pool,
                "supervisor",
                "replica.recreate",
                &format!("apelora/web-0{i}"),
                &json!({}),
            )
            .await
            .expect("audit");
        }
        let p = evaluate_once(&pool, &provider, &cfg(), None, "w")
            .await
            .expect("pass");
        assert_eq!(
            p.pending,
            vec![("supervisor-actions".to_owned(), "supervisor".to_owned())]
        );
        assert!(p.fired.is_empty(), "not before the duration");
        // An hour later (the alert started an hour ago), it fires with the configured severity.
        sqlx::query("UPDATE alerts SET started_at = started_at - interval '1 hour'")
            .execute(&pool)
            .await
            .expect("age");
        // A silence for the rule holds back the message; the alert still fires.
        db::create_silence(
            &pool,
            &db::NewSilence {
                rule: "supervisor-actions",
                subject: None,
                minutes: 60,
                note: "known",
                by: "user:1",
                by_name: "ada",
            },
        )
        .await
        .expect("silence");
        let p = evaluate_once(&pool, &provider, &cfg(), None, "w")
            .await
            .expect("pass");
        assert_eq!(p.fired.len(), 1);
        assert_eq!(p.firing.get("critical"), Some(&1));
        assert_eq!(p.notifications, 0);
        assert!(alert_messages(&pool).await.is_empty());

        // Turning the rule off resolves it on the next pass.
        db::put_rule_settings(
            &pool,
            "supervisor-actions",
            false,
            2.0,
            3600,
            "critical",
            "user:1",
        )
        .await
        .expect("off");
        let p = evaluate_once(&pool, &provider, &cfg(), None, "w")
            .await
            .expect("pass");
        assert_eq!(p.resolved.len(), 1);
        assert_eq!(p.sources["supervisor-actions"], "disabled");
    }

    fn set_member_status(mock: &MockIncus, name: &str, status: &str) {
        let mut members = mock.members.lock().unwrap_or_else(|e| e.into_inner());
        for m in members.iter_mut().filter(|m| m.name == name) {
            m.status = status.into();
        }
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn incus_down_fires_its_own_rule_and_keeps_the_others_as_they_were(pool: Pool) {
        let mock = MockIncus::default();
        let provider = Provider::Mock(mock.clone());
        let down = Provider::Incus(apelora_incus_adapter::IncusClient::plain_for_tests(
            "http://127.0.0.1:1",
        ));
        let member = mock.members.lock().expect("members")[1].name.clone();
        // A member offline: pending (120 s), aged so it fires.
        set_member_status(&mock, &member, "Offline");
        evaluate_once(&pool, &provider, &cfg(), None, "w")
            .await
            .expect("pass");
        sqlx::query("UPDATE alerts SET started_at = started_at - interval '10 minutes'")
            .execute(&pool)
            .await
            .expect("age");
        let p = evaluate_once(&pool, &provider, &cfg(), None, "w")
            .await
            .expect("pass");
        assert_eq!(p.fired, vec![("member-offline".to_owned(), member.clone())]);
        // Incus stops answering: its rule opens, the member alert is kept as it is.
        let p = evaluate_once(&pool, &down, &cfg(), None, "w")
            .await
            .expect("pass");
        assert_eq!(p.sources["member-offline"], "unavailable");
        assert_eq!(p.sources["application-status"], "unavailable");
        assert_eq!(
            p.pending,
            vec![("incus-unreachable".to_owned(), "incus".to_owned())]
        );
        let open = db::open_alerts(&pool).await.expect("open");
        assert!(
            open.iter()
                .any(|a| a.rule == "member-offline" && a.state == "firing")
        );
        // Back: the member is online again; it resolves after staying clear for its duration.
        set_member_status(&mock, &member, "Online");
        let p = evaluate_once(&pool, &provider, &cfg(), None, "w")
            .await
            .expect("pass");
        assert!(p.resolved.is_empty(), "clearing, not yet resolved");
        sqlx::query("UPDATE alerts SET cleared_at = cleared_at - interval '10 minutes' WHERE cleared_at IS NOT NULL")
            .execute(&pool)
            .await
            .expect("age");
        let p = evaluate_once(&pool, &provider, &cfg(), None, "w")
            .await
            .expect("pass");
        assert_eq!(
            p.resolved,
            vec![("member-offline".to_owned(), member.clone())]
        );
        // The pending Incus alert never fired: dropped, not in the history.
        assert!(db::open_alerts(&pool).await.expect("open").is_empty());
        assert_eq!(
            db::history(&pool, None, 10).await.expect("history").len(),
            1
        );
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn an_evaluator_that_lost_its_lease_writes_nothing(pool: Pool) {
        let provider = Provider::Mock(MockIncus::default());
        let held = acquire_leadership(&pool, LEADER_LEASE, "worker-a", 60.0)
            .await
            .expect("lease")
            .expect("free");
        let stale = Leadership {
            generation: held.generation + 1,
            ..held.clone()
        };
        fail_a_job(&pool, "network.sync").await;
        let e = evaluate_once(&pool, &provider, &cfg(), Some(&stale), "worker-b").await;
        assert!(matches!(e, Err(StoreError::Conflict(_))), "{e:?}");
        assert!(db::open_alerts(&pool).await.expect("open").is_empty());
        assert!(db::evaluator(&pool).await.expect("read").is_none());
        let p = evaluate_once(&pool, &provider, &cfg(), Some(&held), "worker-a")
            .await
            .expect("pass");
        assert_eq!(p.fired.len(), 1);
    }
}

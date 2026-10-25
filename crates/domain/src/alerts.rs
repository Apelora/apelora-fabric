// SPDX-License-Identifier: AGPL-3.0-only
//! Built-in alert rules and how an alert moves between states (docs/design/alerts.md). Pure:
//! the worker supplies observations and the time; nothing here reads a database or Incus.
//!
//! Rules are code; an operator changes only their parameters (on or off, threshold, duration,
//! severity). Each rule watches subjects (a job kind, a worker, a member, a volume…) and reports
//! one value per subject. The condition holds when the value reaches the threshold; it must hold
//! for the rule's duration before the alert fires, and stay clear for the same duration before
//! it resolves (hysteresis).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum AlertSeverity {
    Info,
    Warning,
    Critical,
}

impl AlertSeverity {
    pub const ALL: [AlertSeverity; 3] = [
        AlertSeverity::Critical,
        AlertSeverity::Warning,
        AlertSeverity::Info,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            AlertSeverity::Info => "info",
            AlertSeverity::Warning => "warning",
            AlertSeverity::Critical => "critical",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.as_str() == s)
    }

    /// Is an alert of this severity sent to a channel that wants `min` and above?
    pub fn meets(self, min: AlertSeverity) -> bool {
        self >= min
    }
}

/// How a rule compares its value with the threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum AlertComparison {
    /// Holds when `value >= threshold`.
    AtLeast,
    /// Holds when `value <= threshold`.
    AtMost,
}

/// One built-in rule and its defaults.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AlertRule {
    /// Stable id: lowercase letters, digits and hyphens.
    pub id: &'static str,
    pub title: &'static str,
    /// What it watches and why, in a sentence or two.
    pub description: &'static str,
    /// What the value counts (shown next to the threshold).
    pub unit: &'static str,
    pub comparison: AlertComparison,
    pub threshold: f64,
    /// Smallest and largest threshold an operator may set.
    pub threshold_min: f64,
    pub threshold_max: f64,
    /// `false` for rules whose value is 1 while a state lasts (the threshold is fixed at 1).
    pub threshold_editable: bool,
    /// Seconds the condition must hold before firing (and stay clear before resolving).
    pub for_secs: u32,
    pub severity: AlertSeverity,
    /// Where the rule's facts come from (shown in the Console).
    pub source: &'static str,
    /// Where the operator runbooks explain this alert: a page and section, e.g.
    /// `alerts.md#jobs-are-failing`.
    pub runbook: &'static str,
}

/// Longest duration a rule may wait (a day).
pub const MAX_FOR_SECS: u32 = 86_400;

#[allow(clippy::too_many_arguments)]
const fn rule(
    id: &'static str,
    title: &'static str,
    description: &'static str,
    unit: &'static str,
    (threshold, threshold_min, threshold_max): (f64, f64, f64),
    for_secs: u32,
    severity: AlertSeverity,
    source: &'static str,
    runbook: &'static str,
) -> AlertRule {
    AlertRule {
        id,
        title,
        description,
        unit,
        comparison: AlertComparison::AtLeast,
        threshold,
        threshold_min,
        threshold_max,
        threshold_editable: true,
        for_secs,
        severity,
        source,
        runbook,
    }
}

/// A rule whose value is 1 while a state lasts: only its duration and severity are tuned.
#[allow(clippy::too_many_arguments)]
const fn state_rule(
    id: &'static str,
    title: &'static str,
    description: &'static str,
    for_secs: u32,
    severity: AlertSeverity,
    source: &'static str,
    runbook: &'static str,
) -> AlertRule {
    AlertRule {
        threshold_editable: false,
        ..rule(
            id,
            title,
            description,
            "while it lasts",
            (1.0, 1.0, 1.0),
            for_secs,
            severity,
            source,
            runbook,
        )
    }
}

use AlertSeverity::{Critical, Warning};

/// Every built-in rule, in the order the Console lists them.
pub const RULES: &[AlertRule] = &[
    rule(
        "job-failures",
        "Jobs are failing",
        "Jobs of one kind that ended failed in the last 15 minutes. A failed job did not reach its goal after its retries.",
        "failed jobs in 15 minutes",
        (1.0, 1.0, 1000.0),
        0,
        Warning,
        "Job queue",
        "alerts.md#jobs-are-failing",
    ),
    rule(
        "job-queue-wait",
        "Jobs are waiting too long",
        "How long the oldest job that could run has waited. Long waits mean no worker is claiming jobs, or workers are busy.",
        "seconds",
        (300.0, 10.0, 86_400.0),
        60,
        Warning,
        "Job queue",
        "alerts.md#jobs-are-waiting-too-long",
    ),
    rule(
        "job-leases-expired",
        "Running jobs lost their worker",
        "Running jobs whose worker stopped renewing the lease. Another worker takes them over; if none does, they stay stuck.",
        "jobs",
        (1.0, 1.0, 1000.0),
        120,
        Warning,
        "Job queue",
        "alerts.md#running-jobs-lost-their-worker",
    ),
    rule(
        "worker-silent",
        "A worker stopped reporting",
        "A deployment worker that did not stop cleanly and has not written its heartbeat for this long (workers write one every 15 seconds).",
        "seconds since its heartbeat",
        (60.0, 30.0, 86_400.0),
        0,
        Critical,
        "Worker heartbeats",
        "alerts.md#a-worker-stopped-reporting",
    ),
    rule(
        "leader-missing",
        "Nobody holds a leader role",
        "A role only one worker plays (replica supervisor, outbox relay, alert evaluator) has had no holder for this long.",
        "seconds without a holder",
        (90.0, 30.0, 86_400.0),
        0,
        Warning,
        "Leader leases",
        "alerts.md#nobody-holds-a-leader-role",
    ),
    rule(
        "supervisor-actions",
        "The supervisor is acting often",
        "Replica actions (recreate, restart, substitute, retire) the supervisor took in the last hour. Many in a row usually mean one cause that keeps coming back.",
        "actions in an hour",
        (4.0, 1.0, 1000.0),
        0,
        Warning,
        "Supervisor audit rows",
        "alerts.md#the-supervisor-is-acting-often",
    ),
    state_rule(
        "replica-unhealthy",
        "A replica is unhealthy",
        "An instance the supervisor rates unhealthy (its port check keeps failing).",
        300,
        Warning,
        "Instance health",
        "alerts.md#a-replica-is-unhealthy",
    ),
    state_rule(
        "application-status",
        "An application is failed or degraded",
        "An application whose derived status is failed (its last start, stop or restart failed) or degraded (fewer instances run than it should have).",
        600,
        Warning,
        "Applications",
        "alerts.md#an-application-is-failed-or-degraded",
    ),
    state_rule(
        "incus-unreachable",
        "Incus cannot be reached",
        "The worker cannot read the member or instance list from Incus. Deployments, moves and recovery wait until it can.",
        60,
        Critical,
        "Incus",
        "alerts.md#incus-cannot-be-reached",
    ),
    state_rule(
        "member-offline",
        "A cluster member is not online",
        "An Incus cluster member whose status is not Online. Two of three offline puts the cluster's quorum at risk.",
        120,
        Critical,
        "Incus members",
        "alerts.md#a-cluster-member-is-not-online",
    ),
    state_rule(
        "member-unfenced",
        "A failed server is waiting for fencing",
        "A server Incus reports Offline holds modules on shared storage that restart on another server only once it is fenced: its storage lease expired, or a person confirmed it is off. Normally about 140 s after it stopped.",
        300,
        Critical,
        "Failed servers",
        "alerts.md#a-failed-server-is-waiting-for-fencing",
    ),
    AlertRule {
        comparison: AlertComparison::AtMost,
        ..rule(
            "certificate-expiry",
            "A certificate expires soon",
            "Days left before the certificate the worker uses to reach Incus, or the Incus server certificate it trusts, expires.",
            "days left",
            (30.0, 1.0, 365.0),
            0,
            Warning,
            "Worker's Incus configuration",
            "alerts.md#a-certificate-expires-soon",
        )
    },
    state_rule(
        "backup-failed",
        "A scheduled snapshot or backup failed",
        "The last scheduled snapshot or backup of a volume failed (or no backup pool is configured).",
        0,
        Warning,
        "Volume schedules",
        "alerts.md#a-scheduled-snapshot-or-backup-failed",
    ),
    rule(
        "backup-age",
        "A backup is overdue",
        "Age of a scheduled volume's newest backup, in schedule periods (2 means two backups were missed in a row).",
        "schedule periods",
        (2.0, 1.0, 100.0),
        0,
        Warning,
        "Volume schedules",
        "alerts.md#a-backup-is-overdue",
    ),
    rule(
        "outbox-stuck",
        "Messages are not being delivered",
        "How long the oldest undelivered outbox message (audit copies, alert notifications) has waited.",
        "seconds",
        (600.0, 60.0, 86_400.0),
        0,
        Warning,
        "Outbox",
        "alerts.md#messages-are-not-being-delivered",
    ),
    rule(
        "restore-drill-age",
        "Apelora's database restore is not proven",
        "Days since a restore of Apelora's own database was last verified by the drill. Read from the drill's record where the worker can see it (APELORA_RESTORE_DRILL_RECORD).",
        "days",
        (35.0, 1.0, 365.0),
        0,
        Warning,
        "Restore drill record",
        "alerts.md#apeloras-database-restore-is-not-proven",
    ),
];

pub fn alert_rule(id: &str) -> Option<&'static AlertRule> {
    RULES.iter().find(|r| r.id == id)
}

/// A rule's parameters as in force: the defaults, or what an operator changed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RuleSettings {
    pub enabled: bool,
    pub threshold: f64,
    pub for_secs: u32,
    pub severity: AlertSeverity,
}

impl AlertRule {
    pub fn defaults(&self) -> RuleSettings {
        RuleSettings {
            enabled: true,
            threshold: self.threshold,
            for_secs: self.for_secs,
            severity: self.severity,
        }
    }

    /// Does `value` meet the condition at `threshold`?
    pub fn holds(&self, value: f64, threshold: f64) -> bool {
        match self.comparison {
            AlertComparison::AtLeast => value >= threshold,
            AlertComparison::AtMost => value <= threshold,
        }
    }

    /// Checks parameters an operator wants to set; the reason when they are refused.
    pub fn check(&self, s: &RuleSettings) -> Result<(), String> {
        if !s.threshold.is_finite() {
            return Err("the threshold must be a number".into());
        }
        if !self.threshold_editable && s.threshold != self.threshold {
            return Err(format!(
                "{} has no threshold to change (it fires while the state lasts)",
                self.id
            ));
        }
        if s.threshold < self.threshold_min || s.threshold > self.threshold_max {
            return Err(format!(
                "the threshold of {} must be between {} and {} {}",
                self.id, self.threshold_min, self.threshold_max, self.unit
            ));
        }
        if s.for_secs > MAX_FOR_SECS {
            return Err(format!(
                "the duration must be at most {MAX_FOR_SECS} seconds (a day)"
            ));
        }
        Ok(())
    }
}

/// What a rule found about one subject in one pass.
#[derive(Debug, Clone, PartialEq)]
pub struct Observation {
    /// What it is about, e.g. `instance.create`, `worker-7`, `apelora/web-01`.
    pub subject: String,
    pub value: f64,
    /// One sentence for people (no secrets).
    pub summary: String,
}

impl Observation {
    pub fn new(subject: impl Into<String>, value: f64, summary: impl Into<String>) -> Self {
        Observation {
            subject: subject.into(),
            value,
            summary: summary.into(),
        }
    }
}

/// What a rule's source said in one pass.
#[derive(Debug, Clone, PartialEq)]
pub enum Findings {
    /// Read: one observation per subject watched (subjects not listed are clear).
    Observed(Vec<Observation>),
    /// The source is not set up on this installation; the rule cannot fire.
    NotConfigured(String),
    /// The source could not be read now; open alerts stay as they are.
    Unavailable(String),
}

impl Findings {
    /// The word the API and the Console show for the source.
    pub fn source_state(&self) -> &'static str {
        match self {
            Findings::Observed(_) => "ok",
            Findings::NotConfigured(_) => "not-configured",
            Findings::Unavailable(_) => "unavailable",
        }
    }
}

/// An open alert as the evaluator last left it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OpenAlert {
    pub firing: bool,
    /// Unix seconds: when the condition was first seen.
    pub started_at: f64,
    /// Unix seconds: when it was first seen clear while firing.
    pub cleared_at: Option<f64>,
}

/// What to do with one (rule, subject) in this pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    /// Nothing open and nothing to open.
    None,
    /// Open a pending alert (the condition must hold for the duration first).
    OpenPending,
    /// Open a firing alert at once (duration 0).
    OpenFiring,
    /// Pending → firing.
    Fire,
    /// Still pending, or still firing: refresh value and summary (and forget `cleared_at`).
    Refresh,
    /// A pending alert whose condition cleared before it fired: forget it.
    Drop,
    /// Firing, clear for less than the duration: note when it cleared (or keep that note).
    Clearing,
    /// Firing, clear for the duration: resolve.
    Resolve,
}

impl Transition {
    /// Does this transition notify the channels?
    pub fn notifies(self) -> bool {
        matches!(
            self,
            Transition::OpenFiring | Transition::Fire | Transition::Resolve
        )
    }
}

/// The transition for one (rule, subject): `open` as last left, whether the condition `holds`
/// now, the time `now` and the rule's duration `for_secs` (Unix seconds and seconds).
pub fn transition(open: Option<&OpenAlert>, holds: bool, now: f64, for_secs: u32) -> Transition {
    let wait = f64::from(for_secs);
    match (open, holds) {
        (None, false) => Transition::None,
        (None, true) if for_secs == 0 => Transition::OpenFiring,
        (None, true) => Transition::OpenPending,
        (Some(a), true) if !a.firing && now - a.started_at >= wait => Transition::Fire,
        (Some(_), true) => Transition::Refresh,
        (Some(a), false) if !a.firing => Transition::Drop,
        (Some(a), false) => {
            let cleared = a.cleared_at.unwrap_or(now);
            if now - cleared >= wait {
                Transition::Resolve
            } else {
                Transition::Clearing
            }
        }
    }
}

/// Is a silence for `rule` (`subject` `None` = all of the rule's subjects) covering
/// `(alert_rule, alert_subject)`?
pub fn silence_covers(
    rule: &str,
    subject: Option<&str>,
    alert_rule: &str,
    alert_subject: &str,
) -> bool {
    rule == alert_rule && subject.is_none_or(|s| s == alert_subject)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_are_unique_well_formed_and_their_defaults_pass_their_own_checks() {
        let mut seen = std::collections::HashSet::new();
        for r in RULES {
            assert!(seen.insert(r.id), "duplicate {}", r.id);
            assert!(
                r.id.bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-'),
                "{}",
                r.id
            );
            assert!(!r.title.is_empty() && !r.description.is_empty() && !r.unit.is_empty());
            assert!(r.runbook.starts_with("alerts.md#"), "{}", r.id);
            assert_eq!(r.check(&r.defaults()), Ok(()), "{}", r.id);
            assert!(r.threshold_min <= r.threshold && r.threshold <= r.threshold_max);
        }
        assert_eq!(RULES.len(), 16);
        assert!(alert_rule("job-failures").is_some());
        assert!(alert_rule("nope").is_none());
    }

    #[test]
    fn thresholds_compare_in_the_rule_s_direction_and_are_bounded() {
        let wait = alert_rule("job-queue-wait").expect("rule");
        assert!(wait.holds(300.0, 300.0) && !wait.holds(299.0, 300.0));
        let cert = alert_rule("certificate-expiry").expect("rule");
        assert!(cert.holds(29.0, 30.0) && cert.holds(30.0, 30.0) && !cert.holds(31.0, 30.0));
        let mut s = wait.defaults();
        s.threshold = 5.0;
        assert!(wait.check(&s).is_err(), "below its minimum");
        s.threshold = f64::NAN;
        assert!(wait.check(&s).is_err());
        s.threshold = 600.0;
        s.for_secs = MAX_FOR_SECS + 1;
        assert!(wait.check(&s).is_err());
        let state = alert_rule("member-offline").expect("rule");
        let mut s = state.defaults();
        s.threshold = 2.0;
        assert!(
            state.check(&s).is_err(),
            "a state rule's threshold is fixed"
        );
        s.threshold = 1.0;
        s.for_secs = 0;
        assert_eq!(state.check(&s), Ok(()));
    }

    #[test]
    fn severities_order_and_parse() {
        assert!(AlertSeverity::Critical.meets(AlertSeverity::Warning));
        assert!(AlertSeverity::Warning.meets(AlertSeverity::Warning));
        assert!(!AlertSeverity::Info.meets(AlertSeverity::Warning));
        for s in AlertSeverity::ALL {
            assert_eq!(AlertSeverity::parse(s.as_str()), Some(s));
        }
        assert_eq!(AlertSeverity::parse("page"), None);
    }

    fn pending(at: f64) -> OpenAlert {
        OpenAlert {
            firing: false,
            started_at: at,
            cleared_at: None,
        }
    }
    fn firing(at: f64, cleared: Option<f64>) -> OpenAlert {
        OpenAlert {
            firing: true,
            started_at: at,
            cleared_at: cleared,
        }
    }

    #[test]
    fn a_condition_fires_only_after_holding_for_the_duration() {
        use Transition as T;
        assert_eq!(transition(None, false, 100.0, 60), T::None);
        assert_eq!(transition(None, true, 100.0, 60), T::OpenPending);
        assert_eq!(transition(None, true, 100.0, 0), T::OpenFiring);
        assert_eq!(
            transition(Some(&pending(100.0)), true, 159.0, 60),
            T::Refresh
        );
        assert_eq!(transition(Some(&pending(100.0)), true, 160.0, 60), T::Fire);
        // It cleared before firing: forgotten, never notified.
        assert_eq!(transition(Some(&pending(100.0)), false, 130.0, 60), T::Drop);
    }

    #[test]
    fn a_firing_alert_resolves_only_after_staying_clear_for_the_duration() {
        use Transition as T;
        assert_eq!(
            transition(Some(&firing(0.0, None)), true, 500.0, 60),
            T::Refresh
        );
        // First clear pass: note it.
        assert_eq!(
            transition(Some(&firing(0.0, None)), false, 500.0, 60),
            T::Clearing
        );
        // Still clear, not long enough.
        assert_eq!(
            transition(Some(&firing(0.0, Some(500.0))), false, 559.0, 60),
            T::Clearing
        );
        assert_eq!(
            transition(Some(&firing(0.0, Some(500.0))), false, 560.0, 60),
            T::Resolve
        );
        // Holding again in between resets the clock (T::Refresh forgets cleared_at).
        assert_eq!(
            transition(Some(&firing(0.0, Some(500.0))), true, 530.0, 60),
            T::Refresh
        );
        // Duration 0: resolves on the first clear pass.
        assert_eq!(
            transition(Some(&firing(0.0, None)), false, 10.0, 0),
            T::Resolve
        );
    }

    #[test]
    fn only_firing_and_resolving_notify() {
        use Transition as T;
        let notifying: Vec<Transition> = [
            T::None,
            T::OpenPending,
            T::OpenFiring,
            T::Fire,
            T::Refresh,
            T::Drop,
            T::Clearing,
            T::Resolve,
        ]
        .into_iter()
        .filter(|t| t.notifies())
        .collect();
        assert_eq!(notifying, vec![T::OpenFiring, T::Fire, T::Resolve]);
    }

    #[test]
    fn silences_cover_one_subject_or_the_whole_rule() {
        assert!(silence_covers(
            "job-failures",
            None,
            "job-failures",
            "instance.create"
        ));
        assert!(silence_covers(
            "job-failures",
            Some("instance.create"),
            "job-failures",
            "instance.create"
        ));
        assert!(!silence_covers(
            "job-failures",
            Some("instance.delete"),
            "job-failures",
            "instance.create"
        ));
        assert!(!silence_covers("member-offline", None, "job-failures", "x"));
    }
}

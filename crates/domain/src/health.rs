// SPDX-License-Identifier: AGPL-3.0-only
//! Instance health with hysteresis (Phase 08). Pure: the caller observes, at a time it chooses,
//! whether a running instance passes its check; this decides whether the instance is healthy,
//! and when a failing stateless replica should be restarted.
//!
//! The check is what Incus offers without reaching into the instance: an OVN load balancer
//! probes the service port of each backend (a TCP connection, not an application request).
//! Instances without a service address are not checked.

use serde::{Deserialize, Serialize};

/// One observation of a running instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    Passing,
    Failing,
    /// No check result (no health check, or the check is not installed yet).
    Unknown,
}

/// The decided health of an instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum HealthState {
    /// Not decided yet, or no check reports on it.
    Unknown,
    Healthy,
    Unhealthy,
}

impl HealthState {
    pub fn as_str(self) -> &'static str {
        match self {
            HealthState::Unknown => "unknown",
            HealthState::Healthy => "healthy",
            HealthState::Unhealthy => "unhealthy",
        }
    }

    pub fn parse(s: &str) -> HealthState {
        match s {
            "healthy" => HealthState::Healthy,
            "unhealthy" => HealthState::Unhealthy,
            _ => HealthState::Unknown,
        }
    }
}

/// How long a result must hold before the decision changes, and how restarts are paced.
/// Times are in seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HealthPolicy {
    /// Failing this long turns an instance unhealthy.
    pub fail_after: f64,
    /// Passing this long turns it healthy again.
    pub pass_after: f64,
    /// Unhealthy this long gets a stateless replica restarted.
    pub restart_after: f64,
    /// At most one restart per instance in this window.
    pub restart_cooldown: f64,
}

impl Default for HealthPolicy {
    fn default() -> Self {
        HealthPolicy {
            fail_after: 20.0,
            pass_after: 20.0,
            restart_after: 60.0,
            restart_cooldown: 300.0,
        }
    }
}

impl HealthPolicy {
    /// Decides on the first observation and restarts at once (tests, drills).
    pub fn immediate() -> Self {
        HealthPolicy {
            fail_after: 0.0,
            pass_after: 0.0,
            restart_after: 0.0,
            restart_cooldown: 0.0,
        }
    }
}

/// The hysteresis state of one instance.
#[derive(Debug, Clone, PartialEq)]
pub struct Tracker {
    pub health: HealthState,
    /// When `health` was decided.
    pub since: f64,
    /// The result currently being observed and since when, while it differs from `health`.
    streak: Option<(Probe, f64)>,
    last_restart: Option<f64>,
}

impl Tracker {
    pub fn new(now: f64) -> Self {
        Tracker {
            health: HealthState::Unknown,
            since: now,
            streak: None,
            last_restart: None,
        }
    }

    /// Records an observation; returns the new health when it changed.
    pub fn observe(
        &mut self,
        probe: Probe,
        now: f64,
        policy: &HealthPolicy,
    ) -> Option<HealthState> {
        let (target, hold) = match probe {
            // No result keeps the decision, and does not count towards a change.
            Probe::Unknown => return None,
            Probe::Passing => (HealthState::Healthy, policy.pass_after),
            Probe::Failing => (HealthState::Unhealthy, policy.fail_after),
        };
        if self.health == target {
            self.streak = None;
            return None;
        }
        let started = match self.streak {
            Some((p, t)) if p == probe => t,
            _ => {
                self.streak = Some((probe, now));
                now
            }
        };
        if now - started >= hold {
            self.health = target;
            self.since = now;
            self.streak = None;
            Some(target)
        } else {
            None
        }
    }

    /// A stateless replica that has stayed unhealthy long enough, and was not restarted within
    /// the cooldown, should be restarted now.
    pub fn should_restart(&self, now: f64, policy: &HealthPolicy) -> bool {
        self.health == HealthState::Unhealthy
            && now - self.since >= policy.restart_after
            && self
                .last_restart
                .is_none_or(|t| now - t >= policy.restart_cooldown)
    }

    pub fn restarted(&mut self, now: f64) {
        self.last_restart = Some(now);
    }

    /// When this tracker last restarted the instance.
    pub fn last_restart(&self) -> Option<f64> {
        self.last_restart
    }
}

/// A load balancer backend's check status as Incus reports it, as a probe.
pub fn probe_of(status: &str) -> Probe {
    match status {
        "online" => Probe::Passing,
        "offline" => Probe::Failing,
        _ => Probe::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: HealthPolicy = HealthPolicy {
        fail_after: 20.0,
        pass_after: 20.0,
        restart_after: 60.0,
        restart_cooldown: 300.0,
    };

    #[test]
    fn a_short_blip_changes_nothing() {
        let mut t = Tracker::new(0.0);
        assert_eq!(t.observe(Probe::Passing, 0.0, &P), None);
        assert_eq!(
            t.observe(Probe::Passing, 20.0, &P),
            Some(HealthState::Healthy)
        );
        // Failing for 10 s, then passing again: still healthy.
        assert_eq!(t.observe(Probe::Failing, 30.0, &P), None);
        assert_eq!(t.observe(Probe::Failing, 40.0, &P), None);
        assert_eq!(t.observe(Probe::Passing, 45.0, &P), None);
        assert_eq!(t.observe(Probe::Failing, 50.0, &P), None);
        assert_eq!(t.health, HealthState::Healthy);
    }

    #[test]
    fn sustained_failure_turns_unhealthy_then_recovery_needs_a_sustained_pass() {
        let mut t = Tracker::new(0.0);
        t.observe(Probe::Passing, 0.0, &P);
        t.observe(Probe::Passing, 20.0, &P);
        assert_eq!(t.observe(Probe::Failing, 30.0, &P), None);
        assert_eq!(
            t.observe(Probe::Failing, 50.0, &P),
            Some(HealthState::Unhealthy)
        );
        assert_eq!(t.since, 50.0);
        // Unknown neither confirms nor clears it.
        assert_eq!(t.observe(Probe::Unknown, 60.0, &P), None);
        assert_eq!(t.observe(Probe::Passing, 70.0, &P), None);
        assert_eq!(t.observe(Probe::Failing, 80.0, &P), None);
        assert_eq!(t.observe(Probe::Passing, 85.0, &P), None);
        assert_eq!(
            t.observe(Probe::Passing, 105.0, &P),
            Some(HealthState::Healthy)
        );
    }

    #[test]
    fn restarts_wait_for_the_delay_and_respect_the_cooldown() {
        let mut t = Tracker::new(0.0);
        t.observe(Probe::Failing, 0.0, &P);
        t.observe(Probe::Failing, 20.0, &P);
        assert_eq!(t.health, HealthState::Unhealthy);
        assert!(!t.should_restart(60.0, &P));
        assert!(t.should_restart(80.0, &P));
        t.restarted(80.0);
        // Still failing after the restart: not again within the cooldown.
        assert!(!t.should_restart(200.0, &P));
        assert!(t.should_restart(380.0, &P));
        // Healthy instances are never restarted.
        let mut ok = Tracker::new(0.0);
        ok.observe(Probe::Passing, 0.0, &P);
        ok.observe(Probe::Passing, 20.0, &P);
        assert!(!ok.should_restart(1000.0, &P));
    }

    #[test]
    fn the_immediate_policy_decides_on_one_observation() {
        let p = HealthPolicy::immediate();
        let mut t = Tracker::new(0.0);
        assert_eq!(
            t.observe(Probe::Failing, 0.0, &p),
            Some(HealthState::Unhealthy)
        );
        assert!(t.should_restart(0.0, &p));
        assert_eq!(probe_of("online"), Probe::Passing);
        assert_eq!(probe_of("offline"), Probe::Failing);
        assert_eq!(probe_of("unknown"), Probe::Unknown);
    }
}

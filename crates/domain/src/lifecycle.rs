// SPDX-License-Identifier: AGPL-3.0-only
//! Application lifecycle: what state a deployment is in, and how start, stop and restart are
//! ordered. Pure: the caller supplies what it observed and when; nothing here talks to Incus.
//!
//! **Status is derived, never stored or guessed.** It comes from four sources the caller
//! provides: the recorded desired state, the instances Incus reports, the supervisor's health
//! evidence, and the latest operation. When the observation itself failed the answer is
//! `Unknown`, not the last good state.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::network::Connection;
use crate::plan::{Availability, DesiredAppliance};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum DeploymentStatus {
    /// Designed, never deployed.
    NotDeployed,
    Starting,
    Running,
    /// Some instances are not running or are failing their health check.
    Degraded,
    Stopping,
    Stopped,
    /// The most recent start, stop or restart failed and the application has not been brought
    /// to a good state since.
    Failed,
    /// The control plane cannot observe it (Incus unreachable or the answer is incomplete).
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum OperationKind {
    Start,
    Stop,
    Restart,
}

impl OperationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            OperationKind::Start => "start",
            OperationKind::Stop => "stop",
            OperationKind::Restart => "restart",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        [
            OperationKind::Start,
            OperationKind::Stop,
            OperationKind::Restart,
        ]
        .into_iter()
        .find(|k| k.as_str() == s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum OperationState {
    Pending,
    Running,
    Succeeded,
    /// Some steps succeeded and one failed; nothing further was attempted.
    Failed,
}

impl OperationState {
    pub fn as_str(self) -> &'static str {
        match self {
            OperationState::Pending => "pending",
            OperationState::Running => "running",
            OperationState::Succeeded => "succeeded",
            OperationState::Failed => "failed",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        [
            OperationState::Pending,
            OperationState::Running,
            OperationState::Succeeded,
            OperationState::Failed,
        ]
        .into_iter()
        .find(|k| k.as_str() == s)
    }
    pub fn is_active(self) -> bool {
        matches!(self, OperationState::Pending | OperationState::Running)
    }
}

/// One instance as observed, for status purposes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observed {
    pub appliance: String,
    pub instance: String,
    pub status: String,
    /// `unhealthy` from the supervisor's decision; anything else is not a failure.
    pub unhealthy: bool,
    /// Incus cannot report this instance (its member is unreachable).
    pub unreachable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Activity {
    pub kind: OperationKind,
    pub state: OperationState,
}

/// What the status is derived from.
#[derive(Debug, Clone)]
pub struct StatusInput<'a> {
    /// Instances the deployment should have (appliance id, replicas); empty = never deployed.
    pub desired: &'a [(String, u8)],
    /// `None` when Incus could not be read at all.
    pub observed: Option<&'a [Observed]>,
    pub latest_operation: Option<Activity>,
}

pub fn derive_status(i: &StatusInput<'_>) -> DeploymentStatus {
    if i.desired.is_empty() {
        return DeploymentStatus::NotDeployed;
    }
    // An operation in flight decides the label, but only if we can still see the instances.
    let Some(observed) = i.observed else {
        return DeploymentStatus::Unknown;
    };
    if let Some(a) = i.latest_operation
        && a.state.is_active()
    {
        return match a.kind {
            OperationKind::Stop => DeploymentStatus::Stopping,
            OperationKind::Start | OperationKind::Restart => DeploymentStatus::Starting,
        };
    }
    if observed.iter().any(|o| o.unreachable) {
        return DeploymentStatus::Unknown;
    }
    let want: u32 = i.desired.iter().map(|(_, n)| u32::from(*n)).sum();
    let running = observed.iter().filter(|o| o.status == "Running").count() as u32;
    let unhealthy = observed
        .iter()
        .filter(|o| o.status == "Running" && o.unhealthy)
        .count();
    let missing_appliance = i
        .desired
        .iter()
        .any(|(a, _)| !observed.iter().any(|o| &o.appliance == a));
    let good = running >= want && unhealthy == 0 && !missing_appliance;
    if good {
        return DeploymentStatus::Running;
    }
    if i.latest_operation
        .is_some_and(|a| a.state == OperationState::Failed)
    {
        return DeploymentStatus::Failed;
    }
    if running == 0 && !observed.is_empty() {
        return DeploymentStatus::Stopped;
    }
    if running == 0 && observed.is_empty() {
        // Deployed according to the records, but nothing exists in Incus.
        return DeploymentStatus::Degraded;
    }
    DeploymentStatus::Degraded
}

/// Layers of appliances for an operation, in the order to act on them. `providers_first`
/// (starting) puts what others depend on first; stopping reverses it. A connection `from → to`
/// means `from` depends on `to`. Appliances in a cycle, and unconnected ones, are placed so
/// every appliance appears exactly once.
pub fn layers(
    appliances: &[String],
    connections: &[Connection],
    providers_first: bool,
) -> Vec<Vec<String>> {
    let known: BTreeSet<&str> = appliances.iter().map(String::as_str).collect();
    // depends_on[a] = what a needs running before it.
    let mut depends_on: BTreeMap<&str, BTreeSet<&str>> = appliances
        .iter()
        .map(|a| (a.as_str(), BTreeSet::new()))
        .collect();
    for c in connections {
        if c.from != c.to && known.contains(c.from.as_str()) && known.contains(c.to.as_str()) {
            depends_on
                .entry(c.from.as_str())
                .or_default()
                .insert(c.to.as_str());
        }
    }
    let mut placed: BTreeSet<&str> = BTreeSet::new();
    let mut out: Vec<Vec<String>> = Vec::new();
    while placed.len() < known.len() {
        let ready: Vec<&str> = depends_on
            .iter()
            .filter(|(a, deps)| !placed.contains(*a) && deps.iter().all(|d| placed.contains(d)))
            .map(|(a, _)| *a)
            .collect();
        // A cycle: nothing is ready, so release the rest together rather than loop forever.
        let layer: Vec<&str> = if ready.is_empty() {
            depends_on
                .keys()
                .copied()
                .filter(|a| !placed.contains(a))
                .collect()
        } else {
            ready
        };
        placed.extend(layer.iter().copied());
        out.push(layer.into_iter().map(str::to_owned).collect());
    }
    if !providers_first {
        out.reverse();
    }
    out
}

/// Appliances whose interruption needs the caller's explicit confirmation: anything that is not
/// stateless holds a single instance (or data) that clients lose while it is stopped.
pub fn needs_confirmation(desired: &[DesiredAppliance]) -> Vec<(String, Availability)> {
    desired
        .iter()
        .filter(|d| d.availability != Availability::Stateless)
        .map(|d| (d.appliance.clone(), d.availability))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn(from: &str, to: &str) -> Connection {
        Connection {
            from: from.into(),
            to: to.into(),
            transport: "tcp".into(),
            port: Some(80),
        }
    }
    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }
    fn obs(appliance: &str, status: &str) -> Observed {
        Observed {
            appliance: appliance.into(),
            instance: format!("app-{appliance}"),
            status: status.into(),
            unhealthy: false,
            unreachable: false,
        }
    }
    fn st(
        desired: &[(&str, u8)],
        observed: Option<&[Observed]>,
        op: Option<Activity>,
    ) -> DeploymentStatus {
        let d: Vec<(String, u8)> = desired.iter().map(|(a, n)| ((*a).to_owned(), *n)).collect();
        derive_status(&StatusInput {
            desired: &d,
            observed,
            latest_operation: op,
        })
    }

    #[test]
    fn providers_start_before_their_consumers_and_stop_after_them() {
        let a = names(&["web", "api", "db", "cache"]);
        let c = [conn("web", "api"), conn("api", "db"), conn("api", "cache")];
        assert_eq!(
            layers(&a, &c, true),
            vec![names(&["cache", "db"]), names(&["api"]), names(&["web"])]
        );
        assert_eq!(
            layers(&a, &c, false),
            vec![names(&["web"]), names(&["api"]), names(&["cache", "db"])]
        );
    }

    #[test]
    fn every_appliance_appears_once_even_with_cycles_unknown_names_and_no_connections() {
        let a = names(&["a", "b", "c", "lone"]);
        let c = [
            conn("a", "b"),
            conn("b", "a"),
            conn("c", "ghost"),
            conn("c", "c"),
        ];
        let l = layers(&a, &c, true);
        let flat: Vec<&String> = l.iter().flatten().collect();
        assert_eq!(flat.len(), 4);
        assert_eq!(flat.iter().collect::<BTreeSet<_>>().len(), 4);
        assert_eq!(
            layers(&names(&["x", "y"]), &[], true),
            vec![names(&["x", "y"])]
        );
        assert!(layers(&[], &[], true).is_empty());
    }

    #[test]
    fn never_deployed_stays_not_deployed_whatever_else_is_known() {
        assert_eq!(st(&[], Some(&[]), None), DeploymentStatus::NotDeployed);
        assert_eq!(st(&[], None, None), DeploymentStatus::NotDeployed);
    }

    #[test]
    fn an_unreadable_inventory_is_unknown_never_healthy() {
        assert_eq!(st(&[("web", 1)], None, None), DeploymentStatus::Unknown);
        let mut o = obs("web", "Running");
        o.unreachable = true;
        assert_eq!(
            st(&[("web", 1)], Some(&[o]), None),
            DeploymentStatus::Unknown,
            "an instance on an unreachable member is not known to be running"
        );
    }

    #[test]
    fn running_needs_every_replica_up_and_healthy() {
        let two = [
            obs("web", "Running"),
            Observed {
                instance: "app-web-r2".into(),
                ..obs("web", "Running")
            },
        ];
        assert_eq!(
            st(&[("web", 2)], Some(&two), None),
            DeploymentStatus::Running
        );
        assert_eq!(
            st(&[("web", 3)], Some(&two), None),
            DeploymentStatus::Degraded,
            "a replica is missing"
        );
        let mut sick = two.clone();
        sick[0].unhealthy = true;
        assert_eq!(
            st(&[("web", 2)], Some(&sick), None),
            DeploymentStatus::Degraded
        );
        let mixed = [obs("web", "Running"), obs("db", "Stopped")];
        assert_eq!(
            st(&[("web", 1), ("db", 1)], Some(&mixed), None),
            DeploymentStatus::Degraded
        );
        assert_eq!(
            st(
                &[("web", 1), ("db", 1)],
                Some(&[obs("web", "Running")]),
                None
            ),
            DeploymentStatus::Degraded,
            "db does not exist"
        );
    }

    #[test]
    fn all_stopped_is_stopped_and_vanished_instances_are_degraded() {
        assert_eq!(
            st(&[("web", 1)], Some(&[obs("web", "Stopped")]), None),
            DeploymentStatus::Stopped
        );
        assert_eq!(
            st(&[("web", 1)], Some(&[]), None),
            DeploymentStatus::Degraded
        );
    }

    #[test]
    fn an_operation_in_flight_decides_the_label_and_a_failed_one_sticks_until_recovery() {
        let run = [obs("web", "Running")];
        let act = |kind, state| Some(Activity { kind, state });
        assert_eq!(
            st(
                &[("web", 1)],
                Some(&run),
                act(OperationKind::Stop, OperationState::Running)
            ),
            DeploymentStatus::Stopping
        );
        assert_eq!(
            st(
                &[("web", 1)],
                Some(&run),
                act(OperationKind::Start, OperationState::Pending)
            ),
            DeploymentStatus::Starting
        );
        assert_eq!(
            st(
                &[("web", 1)],
                Some(&run),
                act(OperationKind::Restart, OperationState::Running)
            ),
            DeploymentStatus::Starting
        );
        let stopped = [obs("web", "Stopped")];
        assert_eq!(
            st(
                &[("web", 1)],
                Some(&stopped),
                act(OperationKind::Start, OperationState::Failed)
            ),
            DeploymentStatus::Failed
        );
        assert_eq!(
            st(
                &[("web", 1)],
                Some(&run),
                act(OperationKind::Start, OperationState::Failed)
            ),
            DeploymentStatus::Running,
            "recovered by other means"
        );
        assert_eq!(
            st(
                &[("web", 1)],
                None,
                act(OperationKind::Stop, OperationState::Running)
            ),
            DeploymentStatus::Unknown,
            "cannot confirm"
        );
        assert_eq!(
            st(
                &[("web", 1)],
                Some(&stopped),
                act(OperationKind::Stop, OperationState::Succeeded)
            ),
            DeploymentStatus::Stopped
        );
    }

    #[test]
    fn only_non_stateless_appliances_need_confirmation_to_stop() {
        use crate::InstanceSpec;
        let spec: InstanceSpec = serde_json::from_value(serde_json::json!({ "project": "p", "name": "n", "kind": "container", "source": { "protocol": "oci", "server": "https://r", "alias": "x" }, "start": true })).expect("spec");
        let d = |name: &str, availability| DesiredAppliance {
            appliance: name.into(),
            spec: spec.clone(),
            replicas: 1,
            availability,
            internet: Default::default(),
        };
        let list = [
            d("web", Availability::Stateless),
            d("db", Availability::Stateful),
            d("cache", Availability::Singleton),
        ];
        let n: Vec<String> = needs_confirmation(&list)
            .into_iter()
            .map(|(a, _)| a)
            .collect();
        assert_eq!(n, ["db", "cache"]);
    }

    #[test]
    fn kinds_and_states_round_trip() {
        for k in [
            OperationKind::Start,
            OperationKind::Stop,
            OperationKind::Restart,
        ] {
            assert_eq!(OperationKind::parse(k.as_str()), Some(k));
        }
        for s in [
            OperationState::Pending,
            OperationState::Running,
            OperationState::Succeeded,
            OperationState::Failed,
        ] {
            assert_eq!(OperationState::parse(s.as_str()), Some(s));
        }
        assert!(OperationState::Running.is_active() && !OperationState::Failed.is_active());
    }
}

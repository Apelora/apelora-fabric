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
use crate::plan::{Availability, DesiredModule};

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
    /// A migration to another server is in progress.
    Migrating,
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
    /// Immediate power-off of every instance, in dependency order. Nothing waits for the guests
    /// to shut down, so unsaved data in memory is lost. Operations recorded before `Shutdown`
    /// existed also say `stop`; they were graceful.
    Stop,
    /// Graceful shutdown of every instance, in dependency order: each guest is asked to stop
    /// and given time to (what `Stop` meant before ADR 0015).
    Shutdown,
    Restart,
    /// Moves every instance to another server, keeping what can run while it moves.
    Migrate,
}

impl OperationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            OperationKind::Start => "start",
            OperationKind::Stop => "stop",
            OperationKind::Shutdown => "shutdown",
            OperationKind::Restart => "restart",
            OperationKind::Migrate => "migrate",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        [
            OperationKind::Start,
            OperationKind::Stop,
            OperationKind::Shutdown,
            OperationKind::Restart,
            OperationKind::Migrate,
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
    /// Cancelled on request at a phase boundary: the phase that was running finished, later
    /// phases were not started.
    Cancelled,
}

impl OperationState {
    pub fn as_str(self) -> &'static str {
        match self {
            OperationState::Pending => "pending",
            OperationState::Running => "running",
            OperationState::Succeeded => "succeeded",
            OperationState::Failed => "failed",
            OperationState::Cancelled => "cancelled",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        [
            OperationState::Pending,
            OperationState::Running,
            OperationState::Succeeded,
            OperationState::Failed,
            OperationState::Cancelled,
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
    pub module: String,
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
    /// Instances the deployment should have (module id, replicas); empty = never deployed.
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
            OperationKind::Stop | OperationKind::Shutdown => DeploymentStatus::Stopping,
            OperationKind::Start | OperationKind::Restart => DeploymentStatus::Starting,
            OperationKind::Migrate => DeploymentStatus::Migrating,
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
    let missing_module = i
        .desired
        .iter()
        .any(|(a, _)| !observed.iter().any(|o| &o.module == a));
    let good = running >= want && unhealthy == 0 && !missing_module;
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
    // Includes "deployed according to the records, but nothing exists in Incus".
    DeploymentStatus::Degraded
}

/// Instances each module of a deployment should have, as [`StatusInput::desired`] takes them.
pub fn replicas_of(desired: &[DesiredModule]) -> Vec<(String, u8)> {
    desired
        .iter()
        .map(|d| (d.module.clone(), d.replicas))
        .collect()
}

/// The instances Incus reports for the application `name` in `project` (managed, labelled for
/// it), with the supervisor's verdict from `unhealthy(instance name)`.
pub fn observed_for(
    name: &str,
    project: &str,
    instances: &[crate::Instance],
    unhealthy: &dyn Fn(&str) -> bool,
) -> Vec<Observed> {
    instances
        .iter()
        .filter(|i| {
            i.managed
                && i.project == project
                && i.owner.as_ref().is_some_and(|o| o.application == name)
        })
        .map(|i| Observed {
            module: i
                .owner
                .as_ref()
                .map(|o| o.module.clone())
                .unwrap_or_default(),
            instance: i.name.clone(),
            status: i.status.clone(),
            unhealthy: unhealthy(&i.name),
            unreachable: i.unreachable,
        })
        .collect()
}

/// Layers of modules for an operation, in the order to act on them. `providers_first`
/// (starting) puts what others depend on first; stopping reverses it. A connection `from → to`
/// means `from` depends on `to`. Modules in a cycle, and unconnected ones, are placed so
/// every module appears exactly once.
pub fn layers(
    modules: &[String],
    connections: &[Connection],
    providers_first: bool,
) -> Vec<Vec<String>> {
    let known: BTreeSet<&str> = modules.iter().map(String::as_str).collect();
    // depends_on[a] = what a needs running before it.
    let mut depends_on: BTreeMap<&str, BTreeSet<&str>> = modules
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
        let before = placed.len();
        placed.extend(layer.iter().copied());
        out.push(layer.into_iter().map(str::to_owned).collect());
        // A pass that places nothing must stop. Otherwise `<` becoming `<=`, or the
        // cycle release selecting modules already placed, spins until the test times out.
        if placed.len() == before {
            break;
        }
    }
    if !providers_first {
        out.reverse();
    }
    out
}

/// Modules whose interruption needs the caller's explicit confirmation: anything that is not
/// stateless holds a single instance (or data) that clients lose while it is stopped.
pub fn needs_confirmation(desired: &[DesiredModule]) -> Vec<(String, Availability)> {
    desired
        .iter()
        .filter(|d| d.availability != Availability::Stateless)
        .map(|d| (d.module.clone(), d.availability))
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
    fn obs(module: &str, status: &str) -> Observed {
        Observed {
            module: module.into(),
            instance: format!("app-{module}"),
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

    fn inst(
        name: &str,
        project: &str,
        owner: Option<(&str, &str)>,
        managed: bool,
        status: &str,
    ) -> crate::Instance {
        crate::Instance {
            name: name.into(),
            project: project.into(),
            kind: crate::InstanceKind::Container,
            status: status.into(),
            location: "m1".into(),
            architecture: "x86_64".into(),
            description: String::new(),
            managed,
            owner: owner.map(|(app, module)| crate::Owner {
                application: app.into(),
                module: module.into(),
                replica: 1,
            }),
            evacuate: None,
            unreachable: false,
            limits: Default::default(),
        }
    }

    #[test]
    fn the_observed_instances_are_only_this_applications_managed_ones_in_its_project() {
        let all = vec![
            inst(
                "shop-web-01",
                "p",
                Some(("shop", "web-01")),
                true,
                "Running",
            ),
            inst("shop-db-01", "p", Some(("shop", "db-01")), true, "Stopped"),
            inst(
                "other-web-01",
                "p",
                Some(("other", "web-01")),
                true,
                "Running",
            ),
            inst(
                "shop-web-01",
                "elsewhere",
                Some(("shop", "web-01")),
                true,
                "Running",
            ),
            inst("shop-x", "p", Some(("shop", "x")), false, "Running"),
            inst("stray", "p", None, true, "Running"),
        ];
        let got = observed_for("shop", "p", &all, &|n| n == "shop-db-01");
        assert_eq!(got.len(), 2, "{got:?}");
        let web = got.iter().find(|o| o.module == "web-01").expect("web");
        assert_eq!(
            (
                web.instance.as_str(),
                web.status.as_str(),
                web.unhealthy,
                web.unreachable
            ),
            ("shop-web-01", "Running", false, false)
        );
        let db = got.iter().find(|o| o.module == "db-01").expect("db");
        assert_eq!(
            (db.status.as_str(), db.unhealthy),
            ("Stopped", true),
            "the supervisor's verdict is carried"
        );
        assert!(observed_for("shop", "nowhere", &all, &|_| false).is_empty());
        assert!(observed_for("nobody", "p", &all, &|_| false).is_empty());
    }

    #[test]
    fn replicas_are_listed_per_module_in_order() {
        let d = |module: &str, replicas: u8| DesiredModule {
            module: module.into(),
            spec: crate::InstanceSpec {
                project: String::new(),
                name: String::new(),
                kind: crate::InstanceKind::Container,
                source: crate::ImageSource {
                    protocol: crate::ImageProtocol::Simplestreams,
                    server: "https://images.example".into(),
                    alias: "debian/13".into(),
                    fingerprint: None,
                },
                profiles: vec![],
                config: Default::default(),
                start: true,
                architectures: vec![],
                owner: None,
                network: None,
                volumes: vec![],
                root_pool: None,
            },
            replicas,
            availability: Availability::Stateless,
            internet: Default::default(),
            guest_os: Default::default(),
            readiness: None,
            dns_resolver: false,
        };
        assert_eq!(
            replicas_of(&[d("web-01", 3), d("api-01", 1)]),
            vec![("web-01".to_owned(), 3), ("api-01".to_owned(), 1)]
        );
        assert!(replicas_of(&[]).is_empty());
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
    fn a_self_connection_or_an_unknown_endpoint_never_changes_the_order() {
        let a = names(&["a", "b"]);
        // b needs a; a "needs" itself and a ghost. Only the real dependency counts.
        let c = [
            conn("b", "a"),
            conn("a", "a"),
            conn("ghost", "a"),
            conn("b", "ghost"),
        ];
        assert_eq!(layers(&a, &c, true), vec![names(&["a"]), names(&["b"])]);
    }

    #[test]
    fn every_module_appears_once_even_with_cycles_unknown_names_and_no_connections() {
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
        // A graceful shutdown, an immediate stop and a migration each have their own label.
        assert_eq!(
            st(
                &[("web", 1)],
                Some(&run),
                act(OperationKind::Shutdown, OperationState::Running)
            ),
            DeploymentStatus::Stopping
        );
        assert_eq!(
            st(
                &[("web", 1)],
                Some(&run),
                act(OperationKind::Migrate, OperationState::Running)
            ),
            DeploymentStatus::Migrating
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
    fn every_operation_kind_round_trips_through_its_name() {
        for k in [
            OperationKind::Start,
            OperationKind::Stop,
            OperationKind::Shutdown,
            OperationKind::Restart,
            OperationKind::Migrate,
        ] {
            assert_eq!(OperationKind::parse(k.as_str()), Some(k));
        }
        assert_eq!(OperationKind::parse("reboot"), None);
    }

    #[test]
    fn only_non_stateless_modules_need_confirmation_to_stop() {
        use crate::InstanceSpec;
        let spec: InstanceSpec = serde_json::from_value(serde_json::json!({ "project": "p", "name": "n", "kind": "container", "source": { "protocol": "oci", "server": "https://r", "alias": "x" }, "start": true })).expect("spec");
        let d = |name: &str, availability| DesiredModule {
            module: name.into(),
            spec: spec.clone(),
            replicas: 1,
            availability,
            internet: Default::default(),
            guest_os: Default::default(),
            readiness: None,
            dns_resolver: false,
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

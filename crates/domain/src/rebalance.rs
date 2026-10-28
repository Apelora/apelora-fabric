// SPDX-License-Identifier: AGPL-3.0-only
//! Rebalancing (mobility slice C): put a stateless module's replicas on different members
//! again after they landed together. Incus's own `cluster.rebalance.interval` stays at 0
//! (it moves instances without a dry run). Apelora decides, one replica at a time.
//!
//! The plan is pure. The supervisor queues the move only when `APELORA_REBALANCE=1`.

use serde::{Deserialize, Serialize};

use crate::enrolment::HELD;
use crate::plan::{Availability, DesiredModule, replica_name};
use crate::{ClusterMember, Instance, InstanceKind};

/// One replica to move so a module is spread again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct RebalanceMove {
    pub project: String,
    pub application: String,
    pub module: String,
    pub instance: String,
    pub from: String,
    pub target: String,
    /// A virtual machine moves while it runs. A container stops briefly, then starts.
    pub live: bool,
    pub reason: String,
}

/// A deployed stateless module the plan may spread.
pub struct SpreadModule<'a> {
    pub application: &'a str,
    pub project: &'a str,
    pub module: &'a DesiredModule,
}

/// The moves that would restore a spread, one per module, the most crowded module first.
/// Stateless modules with a single replica are left where they are. An instance whose
/// member is not Online is left to recovery or to the evacuation already in progress.
pub fn plan(
    members: &[ClusterMember],
    instances: &[Instance],
    modules: &[SpreadModule<'_>],
) -> Vec<RebalanceMove> {
    let mut moves = Vec::new();
    for m in modules {
        if m.module.availability != Availability::Stateless || m.module.replicas < 2 {
            continue;
        }
        let names: Vec<String> = (1..=m.module.replicas)
            .map(|r| replica_name(m.application, &m.module.module, r))
            .collect();
        let here: Vec<&Instance> = instances
            .iter()
            .filter(|i| {
                i.project == m.project
                    && names.iter().any(|n| n == &i.name)
                    && i.status == "Running"
                    && members
                        .iter()
                        .any(|x| x.name == i.location && x.status == "Online")
            })
            .collect();
        if here.len() < 2 {
            continue;
        }
        let eligible: Vec<&ClusterMember> = members
            .iter()
            .filter(|x| {
                x.status == "Online"
                    && x.scheduler != HELD
                    && here.iter().any(|i| i.architecture == x.architecture)
            })
            .collect();
        if eligible.len() < 2 {
            continue;
        }
        let count = |name: &str| here.iter().filter(|i| i.location == name).count();
        let Some(from) = eligible
            .iter()
            .filter(|x| count(&x.name) >= 2)
            .max_by_key(|x| (count(&x.name), x.name.clone()))
        else {
            continue;
        };
        let Some(target) = eligible
            .iter()
            .filter(|x| x.name != from.name && x.architecture == from.architecture)
            .min_by_key(|x| (count(&x.name), x.name.clone()))
        else {
            continue;
        };
        if count(&target.name) >= count(&from.name) {
            continue;
        }
        let Some(instance) = here
            .iter()
            .filter(|i| i.location == from.name)
            .max_by_key(|i| i.name.clone())
        else {
            continue;
        };
        let live = instance.kind == InstanceKind::VirtualMachine;
        let how = if live {
            "It keeps running while it moves."
        } else {
            "It stops briefly while it moves, then starts on the other server."
        };
        moves.push(RebalanceMove {
            project: m.project.to_owned(),
            application: m.application.to_owned(),
            module: m.module.module.clone(),
            instance: instance.name.clone(),
            from: from.name.clone(),
            target: target.name.clone(),
            live,
            reason: format!(
                "{count_from} of {module} are on {from} and {count_to} on {target}. Moving {name} restores the spread. {how}",
                count_from = count(&from.name),
                module = m.module.module,
                from = from.name,
                count_to = count(&target.name),
                target = target.name,
                name = instance.name,
                how = how,
            ),
        });
    }
    moves.sort_by(|a, b| {
        (&a.application, &a.module, &a.instance).cmp(&(&b.application, &b.module, &b.instance))
    });
    moves
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::DesiredModule;
    use crate::{ImageProtocol, ImageSource, InstanceKind, InstanceSpec, Owner};

    fn member(name: &str) -> ClusterMember {
        ClusterMember {
            name: name.into(),
            url: String::new(),
            roles: vec![],
            status: "Online".into(),
            message: String::new(),
            architecture: "aarch64".into(),
            failure_domain: String::new(),
            database: false,
            groups: vec![],
            scheduler: "all".into(),
        }
    }

    fn module(name: &str, replicas: u8) -> DesiredModule {
        DesiredModule {
            module: name.into(),
            spec: InstanceSpec {
                project: String::new(),
                name: String::new(),
                kind: InstanceKind::Container,
                source: ImageSource {
                    protocol: ImageProtocol::Simplestreams,
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
        }
    }

    fn instance(name: &str, location: &str) -> Instance {
        Instance {
            name: name.into(),
            project: "apelora".into(),
            kind: InstanceKind::Container,
            status: "Running".into(),
            location: location.into(),
            architecture: "aarch64".into(),
            description: String::new(),
            managed: true,
            owner: Some(Owner {
                application: "shop".into(),
                module: "web-01".into(),
                replica: 1,
            }),
            evacuate: None,
            unreachable: false,
            built_from: None,
            evacuated_from: None,
            limits: Default::default(),
        }
    }

    #[test]
    fn two_replicas_on_one_server_move_one_to_an_empty_server() {
        let web = module("web-01", 2);
        let mods = [SpreadModule {
            application: "shop",
            project: "apelora",
            module: &web,
        }];
        let members = vec![member("m1"), member("m2")];
        let instances = vec![
            instance("shop-web-01", "m1"),
            instance("shop-web-01-r2", "m1"),
        ];
        let moves = plan(&members, &instances, &mods);
        assert_eq!(moves.len(), 1);
        assert_eq!(moves[0].instance, "shop-web-01-r2");
        assert_eq!(moves[0].from, "m1");
        assert_eq!(moves[0].target, "m2");
        assert!(!moves[0].live);
    }

    #[test]
    fn replicas_already_apart_and_a_single_replica_stay() {
        let web = module("web-01", 2);
        let one = module("cache-01", 1);
        let mods = [
            SpreadModule {
                application: "shop",
                project: "apelora",
                module: &web,
            },
            SpreadModule {
                application: "shop",
                project: "apelora",
                module: &one,
            },
        ];
        let members = vec![member("m1"), member("m2")];
        let instances = vec![
            instance("shop-web-01", "m1"),
            instance("shop-web-01-r2", "m2"),
            instance("shop-cache-01", "m1"),
        ];
        assert!(plan(&members, &instances, &mods).is_empty());
    }

    #[test]
    fn a_virtual_machine_is_planned_to_move_while_it_runs() {
        let web = module("web-01", 2);
        let mods = [SpreadModule {
            application: "shop",
            project: "apelora",
            module: &web,
        }];
        let mut a = instance("shop-web-01", "m1");
        let mut b = instance("shop-web-01-r2", "m1");
        a.kind = InstanceKind::VirtualMachine;
        b.kind = InstanceKind::VirtualMachine;
        let moves = plan(&[member("m1"), member("m2")], &[a, b], &mods);
        assert!(moves[0].live);
    }
}

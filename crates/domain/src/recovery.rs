// SPDX-License-Identifier: AGPL-3.0-only
//! What happens to each instance of a failed member (HA slice B, ADR 0013): restart it on
//! another member once the member is fenced, give it a substitute (stateless replicas), or let
//! it wait for its member (data on a member-local pool) or a person (not Apelora's). Pure.

use serde::{Deserialize, Serialize};

use crate::evacuation::Deployed;
use crate::plan::{Availability, DesiredModule, replica_name};
use crate::storage::is_shared_driver;
use crate::{ClusterMember, Instance, StoragePool};

/// What becomes of one instance of a failed member.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum RecoveryAction {
    /// Restarts on `target` (the member is fenced).
    Restart,
    /// Will restart once the member is fenced.
    AfterFencing,
    /// A stateless replica: the replica supervisor starts a substitute elsewhere.
    Substitute,
    /// Stays where it is until its member returns (or a person restores it elsewhere).
    Wait,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct RecoveryItem {
    pub project: String,
    pub instance: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub application: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    pub action: RecoveryAction,
    /// The member it restarts on (as chosen now; the job chooses again when it runs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub reason: String,
}

/// The deployment record of an instance: its application, project and module.
pub struct Recorded<'a> {
    pub application: &'a str,
    pub project: &'a str,
    pub module: &'a DesiredModule,
}

/// The record an instance was deployed from, matched by project and name (an instance on an
/// offline member has no readable labels).
pub fn record_of<'a>(
    deployed: &'a [Deployed<'a>],
    project: &str,
    name: &str,
) -> Option<Recorded<'a>> {
    deployed.iter().find_map(|d| {
        (d.project == project)
            .then(|| {
                d.desired.iter().find(|m| {
                    (1..=m.replicas.max(1))
                        .any(|r| replica_name(d.application, &m.module, r) == name)
                })
            })
            .flatten()
            .map(|module| Recorded {
                application: d.application,
                project: d.project,
                module,
            })
    })
}

/// Why `module` cannot follow its instance to another member, or `None` when it can: its root
/// disk and every volume are on a shared pool.
pub fn blocked_by_storage(module: &DesiredModule, pools: &[StoragePool]) -> Option<String> {
    let shared = |p: &str| {
        pools
            .iter()
            .any(|x| x.name == p && is_shared_driver(&x.driver))
    };
    match &module.spec.root_pool {
        None => {
            return Some(
                "its root disk is on the member-local pool of the project's profile".into(),
            );
        }
        Some(p) if !shared(p) => {
            return Some(format!("its root disk is on member-local pool {p}"));
        }
        Some(_) => {}
    }
    module
        .spec
        .volumes
        .iter()
        .find_map(|v| match v.pool.as_deref() {
            Some(p) if v.class.shared() && shared(p) => None,
            Some(p) => Some(format!("volume {} is on member-local pool {p}", v.name)),
            None => Some(format!("volume {} has no pool recorded", v.name)),
        })
}

/// The member an instance of `application`/`module` restarts on: Online, not held out of
/// placement, the failed member's architecture; fewest instances of the same module, then
/// fewest instances, then by name. `None` when no member qualifies.
pub fn choose_target(
    failed: &ClusterMember,
    members: &[ClusterMember],
    instances: &[Instance],
    application: &str,
    module: &str,
) -> Option<String> {
    members
        .iter()
        .filter(|m| {
            m.name != failed.name
                && m.status == "Online"
                && m.scheduler != crate::enrolment::HELD
                && (failed.architecture.is_empty() || m.architecture == failed.architecture)
        })
        .min_by_key(|m| {
            let here: Vec<&Instance> = instances.iter().filter(|i| i.location == m.name).collect();
            let siblings = here
                .iter()
                .filter(|i| {
                    i.owner
                        .as_ref()
                        .is_some_and(|o| o.application == application && o.module == module)
                })
                .count();
            (siblings, here.len(), m.name.clone())
        })
        .map(|m| m.name.clone())
}

/// Every instance Incus lists on `failed`, with what becomes of it.
pub fn plan_recovery(
    failed: &ClusterMember,
    fenced: bool,
    members: &[ClusterMember],
    instances: &[Instance],
    deployed: &[Deployed<'_>],
    pools: &[StoragePool],
) -> Vec<RecoveryItem> {
    let mut items: Vec<RecoveryItem> = instances
        .iter()
        .filter(|i| i.location == failed.name)
        .map(|i| {
            let item = |application: Option<&str>,
                        module: Option<&str>,
                        action,
                        target,
                        reason: String| RecoveryItem {
                project: i.project.clone(),
                instance: i.name.clone(),
                application: application.map(str::to_owned),
                module: module.map(str::to_owned),
                action,
                target,
                reason,
            };
            let Some(r) = record_of(deployed, &i.project, &i.name) else {
                return item(
                    None,
                    None,
                    RecoveryAction::Wait,
                    None,
                    "not deployed by Apelora".into(),
                );
            };
            let (app, module) = (Some(r.application), Some(r.module.module.as_str()));
            if r.module.availability == Availability::Stateless {
                return item(
                    app,
                    module,
                    RecoveryAction::Substitute,
                    None,
                    "a stateless replica".into(),
                );
            }
            if let Some(why) = blocked_by_storage(r.module, pools) {
                return item(app, module, RecoveryAction::Wait, None, why);
            }
            match choose_target(failed, members, instances, r.application, &r.module.module) {
                None => item(
                    app,
                    module,
                    RecoveryAction::Wait,
                    None,
                    format!(
                        "no online server with architecture {} can take it",
                        failed.architecture
                    ),
                ),
                Some(t) if fenced => item(
                    app,
                    module,
                    RecoveryAction::Restart,
                    Some(t.clone()),
                    "all its disks are on the shared pool".into(),
                ),
                Some(t) => item(
                    app,
                    module,
                    RecoveryAction::AfterFencing,
                    Some(t),
                    "all its disks are on the shared pool".into(),
                ),
            }
        })
        .collect();
    items.sort_by(|a, b| (&a.project, &a.instance).cmp(&(&b.project, &b.instance)));
    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{VolumeClass, VolumeContent, VolumeMount};
    use crate::{ImageProtocol, ImageSource, InstanceKind, InstanceSpec, Owner};

    fn member(name: &str, status: &str) -> ClusterMember {
        ClusterMember {
            name: name.into(),
            url: String::new(),
            roles: vec![],
            status: status.into(),
            message: String::new(),
            architecture: "aarch64".into(),
            failure_domain: "default".into(),
            database: false,
            groups: vec![],
            scheduler: "all".into(),
        }
    }

    fn pool(name: &str, driver: &str) -> StoragePool {
        StoragePool {
            name: name.into(),
            driver: driver.into(),
            status: "Created".into(),
            description: String::new(),
            locations: vec![],
        }
    }

    fn module(
        name: &str,
        availability: Availability,
        root: Option<&str>,
        volume: Option<(&str, VolumeClass)>,
    ) -> DesiredModule {
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
                volumes: volume
                    .map(|(p, class)| VolumeMount {
                        name: "data".into(),
                        size_mib: 256,
                        content: VolumeContent::Filesystem,
                        class,
                        path: Some("/data".into()),
                        pool: Some(p.into()),
                    })
                    .into_iter()
                    .collect(),
                root_pool: root.map(str::to_owned),
            },
            replicas: 1,
            availability,
            internet: Default::default(),
            guest_os: Default::default(),
            readiness: None,
            dns_resolver: false,
        }
    }

    fn instance(name: &str, location: &str, owner: Option<(&str, &str)>) -> Instance {
        Instance {
            name: name.into(),
            project: "apelora".into(),
            kind: InstanceKind::Container,
            status: if location == "m2" { "Error" } else { "Running" }.into(),
            location: location.into(),
            architecture: "aarch64".into(),
            description: String::new(),
            managed: owner.is_some(),
            owner: owner.map(|(a, m)| Owner {
                application: a.into(),
                module: m.into(),
                replica: 1,
            }),
            evacuate: None,
            unreachable: location == "m2",
            built_from: None,
            limits: Default::default(),
        }
    }

    #[test]
    fn a_vm_on_the_shared_pool_restarts_after_fencing() {
        let members = vec![member("m1", "Online"), member("m2", "Offline")];
        let mut vm = module(
            "db",
            Availability::Stateful,
            Some("shared"),
            Some(("shared", VolumeClass::DataReplicated)),
        );
        vm.spec.kind = InstanceKind::VirtualMachine;
        let desired = vec![vm];
        let deployed = [Deployed {
            application: "shop",
            project: "apelora",
            desired: &desired,
        }];
        let mut inst = instance("shop-db", "m2", None);
        inst.kind = InstanceKind::VirtualMachine;
        let plan = plan_recovery(
            &members[1],
            true,
            &members,
            &[inst],
            &deployed,
            &[pool("shared", "lvmcluster")],
        );
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].action, RecoveryAction::Restart);
        assert_eq!(plan[0].target.as_deref(), Some("m1"));
        assert!(plan[0].reason.contains("shared pool"), "{}", plan[0].reason);
    }

    #[test]
    fn shared_storage_restarts_after_fencing_and_local_data_waits() {
        let members = vec![
            member("m1", "Online"),
            member("m2", "Offline"),
            member("m3", "Online"),
        ];
        let desired = vec![
            module(
                "db",
                Availability::Stateful,
                Some("shared"),
                Some(("shared", VolumeClass::DataReplicated)),
            ),
            module(
                "files",
                Availability::Stateful,
                None,
                Some(("local", VolumeClass::DataLocal)),
            ),
            module("web", Availability::Stateless, None, None),
            module("cache", Availability::Singleton, Some("local"), None),
        ];
        let deployed = [Deployed {
            application: "shop",
            project: "apelora",
            desired: &desired,
        }];
        let pools = [pool("local", "btrfs"), pool("shared", "lvmcluster")];
        let instances = vec![
            instance("shop-db", "m2", None),
            instance("shop-files", "m2", None),
            instance("shop-web", "m2", None),
            instance("shop-cache", "m2", None),
            instance("other", "m2", None),
            // m1 already runs another db of the same module: m3 is preferred.
            instance("shop-db-x", "m1", Some(("shop", "db"))),
        ];
        let plan =
            |fenced| plan_recovery(&members[1], fenced, &members, &instances, &deployed, &pools);
        let by =
            |p: &[RecoveryItem], n: &str| p.iter().find(|i| i.instance == n).cloned().expect(n);
        let waiting = plan(false);
        assert_eq!(waiting.len(), 5);
        let db = by(&waiting, "shop-db");
        assert_eq!(
            (db.action, db.target.as_deref()),
            (RecoveryAction::AfterFencing, Some("m3"))
        );
        let fenced = plan(true);
        let db = by(&fenced, "shop-db");
        assert_eq!(
            (db.action, db.target.as_deref()),
            (RecoveryAction::Restart, Some("m3"))
        );
        assert_eq!(db.application.as_deref(), Some("shop"));
        assert_eq!(by(&fenced, "shop-files").action, RecoveryAction::Wait);
        assert!(by(&fenced, "shop-files").reason.contains("profile"));
        assert!(by(&fenced, "shop-cache").reason.contains("pool local"));
        assert_eq!(by(&fenced, "shop-web").action, RecoveryAction::Substitute);
        assert!(
            by(&fenced, "other")
                .reason
                .contains("not deployed by Apelora")
        );
    }

    #[test]
    fn the_target_is_online_unheld_and_of_the_same_architecture() {
        let failed = member("m2", "Offline");
        let mut held = member("m1", "Online");
        held.scheduler = crate::enrolment::HELD.into();
        let mut arm = member("m3", "Online");
        arm.architecture = "x86_64".into();
        let evacuated = member("m4", "Evacuated");
        assert_eq!(
            choose_target(
                &failed,
                &[held, arm, evacuated, failed.clone()],
                &[],
                "a",
                "b"
            ),
            None
        );
        let busy = member("m5", "Online");
        let idle = member("m6", "Online");
        let instances = vec![instance("x", "m5", None)];
        assert_eq!(
            choose_target(&failed, &[busy, idle], &instances, "a", "b").as_deref(),
            Some("m6")
        );
    }

    #[test]
    fn a_volume_on_a_local_pool_keeps_the_module_waiting_even_with_a_shared_root() {
        let m = module(
            "db",
            Availability::Stateful,
            Some("shared"),
            Some(("local", VolumeClass::DataLocal)),
        );
        let pools = [pool("local", "btrfs"), pool("shared", "ceph")];
        assert!(
            blocked_by_storage(&m, &pools)
                .expect("blocked")
                .contains("volume data")
        );
        let ok = module(
            "db",
            Availability::Stateful,
            Some("shared"),
            Some(("shared", VolumeClass::DataReplicated)),
        );
        assert_eq!(blocked_by_storage(&ok, &pools), None);
        assert!(
            blocked_by_storage(&ok, &[pool("shared", "lvm")]).is_some(),
            "plain LVM is member-local"
        );
    }
}

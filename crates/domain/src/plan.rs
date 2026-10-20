// SPDX-License-Identifier: AGPL-3.0-only
//! Deployment plans: what applying a design would change, decided only from the desired
//! instances and the instances Incus reports. Pure, so the API can show a preview and then
//! recompute the same plan when the change is applied (docs/design/application-deployment.md).

use serde::{Deserialize, Serialize};

use crate::{Instance, InstanceSpec, Owner, is_valid_name};

/// At most this many replicas per appliance.
pub const MAX_REPLICAS: u8 = 9;

/// The instance name of an appliance's first replica: `<application>-<appliance>`.
pub fn instance_name(application: &str, appliance: &str) -> String {
    format!("{application}-{appliance}")
}

/// The instance name of replica `replica` (1-based): the first keeps the plain name, the
/// others get `-r<n>`, so a single-replica deployment keeps its names when replicas are added.
pub fn replica_name(application: &str, appliance: &str, replica: u8) -> String {
    if replica <= 1 {
        instance_name(application, appliance)
    } else {
        format!("{}-r{replica}", instance_name(application, appliance))
    }
}

/// The name of a replica's `n`th substitute: created on another member while the member
/// holding the replica was down (Incus cannot move instances off a failed member).
pub fn substitute_name(application: &str, appliance: &str, replica: u8, n: u8) -> String {
    format!("{}-s{n}", replica_name(application, appliance, replica))
}

/// Whether `name` is replica `base` itself or one of its substitutes (`<base>-s<n>`).
fn in_slot(name: &str, base: &str) -> bool {
    name == base
        || name
            .strip_prefix(base)
            .and_then(|r| r.strip_prefix("-s"))
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// The application's instances that fill replica `replica` of `appliance` in `project`: the
/// replica and its substitutes, recognised by Apelora's labels and their names. An instance
/// whose member is unreachable has no visible labels; one with the slot's name counts (names
/// are unique in a project, and the name was Apelora's before its member went away).
pub fn slot_instances<'a>(
    application: &str,
    project: &str,
    appliance: &str,
    replica: u8,
    observed: &'a [Instance],
) -> Vec<&'a Instance> {
    let base = replica_name(application, appliance, replica);
    observed
        .iter()
        .filter(|i| {
            i.project == project
                && in_slot(&i.name, &base)
                && (i.unreachable
                    || i.managed
                        && i.owner.as_ref().is_some_and(|o| {
                            o.application == application
                                && o.appliance == appliance
                                && o.replica == replica
                        }))
        })
        .collect()
}

/// How an appliance may be run and recovered (from its catalog class).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum Availability {
    /// Interchangeable replicas: several may run, and a lost one is recreated.
    Stateless,
    /// Exactly one instance; never recreated automatically.
    #[default]
    Singleton,
    /// Holds data: one writer, never recreated or failed over automatically.
    Stateful,
}

fn one() -> u8 {
    1
}

/// One appliance of the design, turned into the instances it should run as.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct DesiredAppliance {
    /// The appliance's stable id in the design (e.g. `nginx-01`).
    pub appliance: String,
    /// Image, kind and settings; the control plane sets the name, project and owner.
    pub spec: InstanceSpec,
    /// How many instances (1–9); more than one only for stateless appliances.
    #[serde(default = "one")]
    pub replicas: u8,
    #[serde(default)]
    pub availability: Availability,
    /// What it may reach outside the application (enforced with the connections).
    #[serde(default)]
    pub internet: crate::network::Internet,
}

impl DesiredAppliance {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=MAX_REPLICAS).contains(&self.replicas) {
            return Err(format!(
                "{}: replicas must be 1–{MAX_REPLICAS}",
                self.appliance
            ));
        }
        if self.replicas > 1 && self.availability != Availability::Stateless {
            return Err(format!(
                "{}: only stateless appliances can have more than one replica",
                self.appliance
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum PlanAction {
    /// The instance does not exist yet and will be created.
    Create,
    /// The application's instance already exists and is left as it is.
    Keep,
    /// The application's instance is no longer in the design.
    Remove,
    /// The name is taken by an instance that is not this appliance's: nothing can be applied.
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct PlanItem {
    /// The appliance in the design (for `remove`: the label of the instance being removed).
    pub appliance: String,
    /// 1-based replica number.
    pub replica: u8,
    pub instance: String,
    pub action: PlanAction,
    /// Why, in a sentence (e.g. what the conflicting instance is).
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct Plan {
    pub items: Vec<PlanItem>,
    /// Facts the designer should know before applying, e.g. single points of failure.
    pub warnings: Vec<String>,
}

impl Plan {
    pub fn has_conflicts(&self) -> bool {
        self.items.iter().any(|i| i.action == PlanAction::Conflict)
    }

    pub fn count(&self, action: PlanAction) -> usize {
        self.items.iter().filter(|i| i.action == action).count()
    }
}

/// The spec replica `replica` of an appliance is created with: its name, project and
/// ownership labels are set here, never taken from the caller.
pub fn owned_spec(
    application: &str,
    project: &str,
    desired: &DesiredAppliance,
    replica: u8,
) -> InstanceSpec {
    InstanceSpec {
        project: project.to_owned(),
        name: replica_name(application, &desired.appliance, replica),
        owner: Some(Owner {
            application: application.to_owned(),
            appliance: desired.appliance.clone(),
            replica,
        }),
        ..desired.spec.clone()
    }
}

/// The spec a replica is created with, including its network attachment when the
/// application's connections are enforced on `uplink`. Shared by the API (deployments) and
/// the replica supervisor (recovery), so both create identical instances.
pub fn deploy_spec(
    application: &str,
    project: &str,
    desired: &DesiredAppliance,
    replica: u8,
    uplink: Option<&str>,
) -> InstanceSpec {
    let mut spec = owned_spec(application, project, desired, replica);
    if let Some(uplink) = uplink {
        spec.network = Some(crate::network::attachment(
            application,
            &desired.appliance,
            uplink,
        ));
    }
    spec
}

/// Compares the design with the instances Incus reports in `project`.
pub fn plan(
    application: &str,
    project: &str,
    desired: &[DesiredAppliance],
    observed: &[Instance],
) -> Plan {
    let in_project: Vec<&Instance> = observed.iter().filter(|i| i.project == project).collect();
    let ours = |i: &Instance| {
        i.managed
            && i.owner
                .as_ref()
                .is_some_and(|o| o.application == application)
    };
    let mut items = Vec::new();
    let mut wanted = Vec::new();

    for d in desired {
        for replica in 1..=d.replicas.max(1) {
            let name = replica_name(application, &d.appliance, replica);
            wanted.push(name.clone());
            let item = |action, reason: String| PlanItem {
                appliance: d.appliance.clone(),
                replica,
                instance: name.clone(),
                action,
                reason,
            };
            if !is_valid_name(&name) {
                items.push(item(
                    PlanAction::Conflict,
                    format!("the instance name {name:?} is not valid (at most 63 letters, digits and hyphens)"),
                ));
                continue;
            }
            let slot = slot_instances(application, project, &d.appliance, replica, observed);
            for sub in slot.iter().filter(|i| i.name != name) {
                wanted.push(sub.name.clone());
                items.push(PlanItem {
                    instance: sub.name.clone(),
                    ..item(
                        PlanAction::Keep,
                        format!("substitute for {name} ({})", sub.status),
                    )
                });
            }
            items.push(match in_project.iter().find(|i| i.name == name) {
                None if !slot.is_empty() => continue,
                None => item(PlanAction::Create, "not deployed yet".into()),
                Some(i) if i.unreachable => item(
                    PlanAction::Keep,
                    format!("its member {} cannot be reached", i.location),
                ),
                Some(i) if ours(i) && i.owner.as_ref().is_some_and(|o| o.appliance == d.appliance && o.replica == replica) => {
                    item(PlanAction::Keep, format!("already deployed ({})", i.status))
                }
                Some(i) if !i.managed => item(
                    PlanAction::Conflict,
                    "an instance with this name exists and was not created by Apelora".into(),
                ),
                Some(i) => item(
                    PlanAction::Conflict,
                    match &i.owner {
                        Some(o) => format!("an instance with this name belongs to {}/{} replica {}", o.application, o.appliance, o.replica),
                        None => "an instance with this name was created by Apelora outside an application".into(),
                    },
                ),
            });
        }
    }

    for i in in_project
        .iter()
        .filter(|i| ours(i) && !wanted.contains(&i.name))
    {
        let owner = i.owner.as_ref();
        items.push(PlanItem {
            appliance: owner.map(|o| o.appliance.clone()).unwrap_or_default(),
            replica: owner.map_or(1, |o| o.replica),
            instance: i.name.clone(),
            action: PlanAction::Remove,
            reason: "no longer in the design".into(),
        });
    }

    let mut single: Vec<&str> = desired
        .iter()
        .filter(|d| d.replicas <= 1)
        .map(|d| d.appliance.as_str())
        .collect();
    single.sort_unstable();
    let mut warnings = Vec::new();
    if !single.is_empty() {
        warnings.push(format!(
            "single points of failure: {} run{} as one instance",
            single.join(", "),
            if single.len() == 1 { "s" } else { "" }
        ));
    }
    Plan { items, warnings }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ImageProtocol, ImageSource, InstanceKind};

    fn desired(appliance: &str) -> DesiredAppliance {
        DesiredAppliance {
            appliance: appliance.into(),
            spec: InstanceSpec {
                project: "ignored".into(),
                name: "ignored".into(),
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
            },
            replicas: 1,
            availability: Availability::Singleton,
            internet: Default::default(),
        }
    }

    fn stateless(appliance: &str, replicas: u8) -> DesiredAppliance {
        DesiredAppliance {
            replicas,
            availability: Availability::Stateless,
            ..desired(appliance)
        }
    }

    fn observed(
        name: &str,
        project: &str,
        managed: bool,
        owner: Option<(&str, &str, u8)>,
    ) -> Instance {
        Instance {
            name: name.into(),
            project: project.into(),
            kind: InstanceKind::Container,
            status: "Running".into(),
            location: "m1".into(),
            architecture: "x86_64".into(),
            description: String::new(),
            managed,
            owner: owner.map(|(a, b, r)| Owner {
                application: a.into(),
                appliance: b.into(),
                replica: r,
            }),
            evacuate: None,
            unreachable: false,
        }
    }

    fn actions(p: &Plan) -> Vec<(&str, PlanAction)> {
        p.items
            .iter()
            .map(|i| (i.instance.as_str(), i.action))
            .collect()
    }

    #[test]
    fn plans_create_keep_remove_and_conflicts() {
        use PlanAction::*;
        let want = [
            desired("web-01"),
            desired("db-01"),
            desired("cache-01"),
            desired("api-01"),
        ];
        let have = [
            observed("shop-web-01", "apelora", true, Some(("shop", "web-01", 1))),
            observed("shop-db-01", "apelora", false, None),
            observed(
                "shop-cache-01",
                "apelora",
                true,
                Some(("blog", "cache-01", 1)),
            ),
            observed("shop-old-01", "apelora", true, Some(("shop", "old-01", 1))),
            observed("shop-api-01", "other", true, Some(("shop", "api-01", 1))),
            observed("shop-old-02", "other", true, Some(("shop", "old-02", 1))),
        ];
        let p = plan("shop", "apelora", &want, &have);
        assert_eq!(
            actions(&p),
            vec![
                ("shop-web-01", Keep),
                ("shop-db-01", Conflict),
                ("shop-cache-01", Conflict),
                ("shop-api-01", Create),
                ("shop-old-01", Remove),
            ],
            "instances in other projects are neither kept nor removed"
        );
        assert!(p.has_conflicts());
        assert!(p.items[1].reason.contains("not created by Apelora"));
        assert!(p.items[2].reason.contains("blog/cache-01"));
        assert_eq!(p.items[4].appliance, "old-01");
        assert_eq!((p.count(Create), p.count(Keep), p.count(Remove)), (1, 1, 1));
        assert!(
            p.warnings[0].contains("single points of failure: api-01, cache-01, db-01, web-01")
        );
    }

    #[test]
    fn replicas_get_their_own_names_scale_up_and_down() {
        use PlanAction::*;
        assert_eq!(replica_name("shop", "web-01", 1), "shop-web-01");
        assert_eq!(replica_name("shop", "web-01", 3), "shop-web-01-r3");
        let have = [
            observed("shop-web-01", "apelora", true, Some(("shop", "web-01", 1))),
            observed(
                "shop-web-01-r2",
                "apelora",
                true,
                Some(("shop", "web-01", 2)),
            ),
            observed(
                "shop-web-01-r3",
                "apelora",
                true,
                Some(("shop", "web-01", 3)),
            ),
        ];
        let up = plan("shop", "apelora", &[stateless("web-01", 4)], &have[..2]);
        assert_eq!(
            actions(&up),
            vec![
                ("shop-web-01", Keep),
                ("shop-web-01-r2", Keep),
                ("shop-web-01-r3", Create),
                ("shop-web-01-r4", Create)
            ]
        );
        assert!(up.warnings.is_empty(), "no single point of failure");
        let down = plan("shop", "apelora", &[stateless("web-01", 2)], &have);
        assert_eq!(
            down.items[2],
            PlanItem {
                appliance: "web-01".into(),
                replica: 3,
                instance: "shop-web-01-r3".into(),
                action: Remove,
                reason: "no longer in the design".into()
            }
        );
        // A replica label that does not match the name is somebody else's instance.
        let odd = [observed(
            "shop-web-01-r2",
            "apelora",
            true,
            Some(("shop", "web-01", 5)),
        )];
        assert_eq!(
            plan("shop", "apelora", &[stateless("web-01", 2)], &odd).items[1].action,
            Conflict
        );
    }

    #[test]
    fn only_stateless_appliances_get_more_than_one_replica() {
        assert_eq!(stateless("web-01", 3).validate(), Ok(()));
        assert!(
            DesiredAppliance {
                replicas: 2,
                ..desired("db-01")
            }
            .validate()
            .is_err()
        );
        assert!(
            DesiredAppliance {
                replicas: 2,
                availability: Availability::Stateful,
                ..desired("db-01")
            }
            .validate()
            .is_err()
        );
        assert!(stateless("web-01", 0).validate().is_err());
        assert!(stateless("web-01", 10).validate().is_err());
    }

    #[test]
    fn names_that_would_be_too_long_are_conflicts_and_specs_are_owned() {
        let long = "a".repeat(60); // "shop-" + 60 = 65 characters
        let p = plan("shop", "apelora", &[desired(&long), desired("web-01")], &[]);
        assert_eq!(p.items[0].action, PlanAction::Conflict);
        assert!(p.items[0].reason.contains("not valid"));
        let spec = owned_spec("shop", "apelora", &stateless("web-01", 2), 2);
        assert_eq!(
            (spec.name.as_str(), spec.project.as_str()),
            ("shop-web-01-r2", "apelora")
        );
        assert_eq!(
            spec.owner,
            Some(Owner {
                application: "shop".into(),
                appliance: "web-01".into(),
                replica: 2
            })
        );
        assert_eq!(spec.validate(), Ok(()));
        assert!(plan("shop", "apelora", &[], &[]).warnings.is_empty());
    }

    #[test]
    fn substitutes_fill_their_replica_slot_and_are_never_removed_or_recreated() {
        let mut web = desired("web-01");
        web.replicas = 2;
        web.availability = Availability::Stateless;
        let observed = [
            observed_instance("shop-web-01", Some(("shop", "web-01", 1))),
            // Replica 2's member failed: its substitute runs elsewhere, the original is gone.
            observed_instance("shop-web-01-r2-s1", Some(("shop", "web-01", 2))),
            // A name that looks like a substitute but belongs to another slot is not one.
            observed_instance("shop-web-01-s1", Some(("shop", "web-01", 2))),
        ];
        let p = plan("shop", "apelora", &[web], &observed);
        assert_eq!(
            actions(&p),
            vec![
                ("shop-web-01", PlanAction::Keep),
                ("shop-web-01-r2-s1", PlanAction::Keep),
                ("shop-web-01-s1", PlanAction::Remove),
            ]
        );
        assert_eq!(substitute_name("shop", "web-01", 2, 3), "shop-web-01-r2-s3");
        assert_eq!(
            slot_instances("shop", "apelora", "web-01", 2, &observed)
                .iter()
                .map(|i| i.name.as_str())
                .collect::<Vec<_>>(),
            ["shop-web-01-r2-s1"]
        );
    }

    fn observed_instance(name: &str, owner: Option<(&str, &str, u8)>) -> Instance {
        observed(name, "apelora", true, owner)
    }
}

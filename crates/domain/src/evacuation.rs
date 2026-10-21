// SPDX-License-Identifier: AGPL-3.0-only
//! Evacuation dry run (Phase 08, M1): before a cluster member is emptied for maintenance, say
//! per instance what Incus will do with it and what that means for the application.
//!
//! Incus performs the evacuation (`cluster.evacuate` per instance, `auto` by default: move it
//! while stopped, or only stop it when a device cannot move); Apelora never moves instances
//! itself. This module only predicts and explains, from the inventory and the recorded
//! deployments. A member whose instances have nowhere to go is blocked.

use serde::{Deserialize, Serialize};

use crate::plan::{Availability, DesiredModule};
use crate::{ClusterMember, Instance};

/// One instance on the member being evacuated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct EvacuationItem {
    pub project: String,
    pub instance: String,
    pub status: String,
    /// Effective `cluster.evacuate` mode (`auto` when unset).
    pub mode: String,
    /// What Incus will do with it.
    pub outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub application: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(alias = "appliance")]
    pub module: Option<String>,
    /// What it means for the application while the member is evacuated.
    pub impact: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct EvacuationPlan {
    pub member: String,
    /// Members that can take instances (online, other than this one).
    pub targets: Vec<String>,
    pub items: Vec<EvacuationItem>,
    /// Why evacuating now is refused; empty when it may proceed.
    pub blocked: Vec<String>,
    /// Consequences to accept before proceeding.
    pub warnings: Vec<String>,
}

/// A recorded deployment: what an application runs in a project.
pub struct Deployed<'a> {
    pub application: &'a str,
    pub project: &'a str,
    pub desired: &'a [DesiredModule],
}

const MOVES: [&str; 4] = ["auto", "migrate", "live-migrate", "refresh-migrate"];

fn outcome(mode: &str, running: bool) -> &'static str {
    match mode {
        "stop" | "force-stop" => "stops here until restore",
        "stateful-stop" => "stops here, memory saved, until restore",
        "live-migrate" => "moves while running",
        "refresh-migrate" => "moves; stopped only for the final copy",
        "migrate" if running => "moves; stopped during the move",
        "migrate" | "auto" if !running => "moves (not running)",
        // `auto` only stops an instance with a device that cannot move; the inventory does
        // not show devices, so the prediction says so.
        "auto" => "moves if it can; stopped during the move",
        _ => "Incus decides (unknown mode)",
    }
}

pub fn plan_evacuation(
    member: &str,
    members: &[ClusterMember],
    instances: &[Instance],
    deployed: &[Deployed<'_>],
) -> Result<EvacuationPlan, String> {
    let this = members
        .iter()
        .find(|m| m.name == member)
        .ok_or_else(|| format!("no cluster member named {member:?}"))?;
    let targets: Vec<&ClusterMember> = members
        .iter()
        .filter(|m| m.name != member && m.status == "Online")
        .collect();
    let mut blocked = Vec::new();
    let mut warnings = Vec::new();
    if this.status != "Online" {
        blocked.push(format!("{member} is {}, not Online", this.status));
    }
    let on_member: Vec<&Instance> = instances.iter().filter(|i| i.location == member).collect();
    if targets.is_empty() && !on_member.is_empty() {
        blocked.push("no other online member can take its instances".into());
    }

    let availability = |i: &Instance| -> Option<Availability> {
        let o = i.owner.as_ref()?;
        deployed
            .iter()
            .find(|d| d.application == o.application && d.project == i.project)
            .and_then(|d| d.desired.iter().find(|x| x.module == o.module))
            .map(|x| x.availability)
    };
    let mut items = Vec::new();
    for i in &on_member {
        let mode = i.evacuate.clone().unwrap_or_else(|| "auto".into());
        let running = i.status == "Running";
        let moves = MOVES.contains(&mode.as_str());
        let mut out = outcome(&mode, running).to_owned();
        if moves && !targets.is_empty() && !targets.iter().any(|t| t.architecture == i.architecture)
        {
            out = format!("cannot move: no other member runs {}", i.architecture);
            blocked.push(format!("{}/{}: {out}", i.project, i.name));
        }
        let down_until_restore = !moves;
        let impact = match (&i.owner, availability(i)) {
            (None, _) if !i.managed => "not Apelora's".to_owned(),
            (None, _) => "no application".to_owned(),
            (Some(o), Some(Availability::Stateless)) => {
                let elsewhere = instances
                    .iter()
                    .filter(|x| {
                        x.location != member
                            && x.project == i.project
                            && x.status == "Running"
                            && x.owner.as_ref().is_some_and(|xo| {
                                xo.application == o.application && xo.module == o.module
                            })
                    })
                    .count();
                if elsewhere > 0 {
                    format!(
                        "stays up ({elsewhere} other replica{})",
                        if elsewhere == 1 { "" } else { "s" }
                    )
                } else if !running {
                    "not running".to_owned()
                } else {
                    "down while it moves: no other replica runs".to_owned()
                }
            }
            (Some(_), Some(Availability::Stateful)) if running && down_until_restore => {
                "down until restore (stateful)".to_owned()
            }
            (Some(_), Some(Availability::Stateful)) if running => {
                "down while its data is copied (stateful)".to_owned()
            }
            (Some(_), _) if running && down_until_restore => "down until restore".to_owned(),
            (Some(_), _) if running => "down while it moves".to_owned(),
            (Some(_), _) => "not running".to_owned(),
        };
        items.push(EvacuationItem {
            project: i.project.clone(),
            instance: i.name.clone(),
            status: i.status.clone(),
            mode,
            outcome: out,
            application: i.owner.as_ref().map(|o| o.application.clone()),
            module: i.owner.as_ref().map(|o| o.module.clone()),
            impact,
        });
    }
    items.sort_by(|a, b| (&a.project, &a.instance).cmp(&(&b.project, &b.instance)));

    // Replicas spread over this member and a single remaining one end up sharing it.
    if targets.len() == 1 {
        let mut shared: Vec<String> = items
            .iter()
            .filter(|it| MOVES.contains(&it.mode.as_str()))
            .filter_map(|it| {
                let (app, module) = (it.application.as_ref()?, it.module.as_ref()?);
                instances
                    .iter()
                    .any(|x| {
                        x.location == targets[0].name
                            && x.project == it.project
                            && x.owner
                                .as_ref()
                                .is_some_and(|o| &o.application == app && &o.module == module)
                    })
                    .then(|| format!("{app}/{module}"))
            })
            .collect();
        shared.sort();
        shared.dedup();
        if !shared.is_empty() {
            warnings.push(format!(
                "replicas of {} share {} until restore: one failure takes them all",
                shared.join(", "),
                targets[0].name
            ));
        }
    }
    let down: Vec<&str> = items
        .iter()
        .filter(|it| it.impact.contains("down"))
        .map(|it| it.instance.as_str())
        .collect();
    if !down.is_empty() {
        warnings.push(format!("down during the evacuation: {}", down.join(", ")));
    }
    Ok(EvacuationPlan {
        member: member.to_owned(),
        targets: targets.iter().map(|t| t.name.clone()).collect(),
        items,
        blocked,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ImageProtocol, ImageSource, InstanceKind, InstanceSpec, Owner};

    fn member(name: &str, status: &str, arch: &str) -> ClusterMember {
        ClusterMember {
            name: name.into(),
            url: String::new(),
            roles: vec![],
            status: status.into(),
            message: String::new(),
            architecture: arch.into(),
            failure_domain: "default".into(),
            database: true,
            groups: vec![],
            scheduler: "all".into(),
        }
    }

    fn inst(name: &str, at: &str, owner: Option<(&str, &str)>, status: &str) -> Instance {
        Instance {
            name: name.into(),
            project: "apelora".into(),
            kind: InstanceKind::Container,
            status: status.into(),
            location: at.into(),
            architecture: "aarch64".into(),
            description: String::new(),
            managed: owner.is_some(),
            owner: owner.map(|(a, b)| Owner {
                application: a.into(),
                module: b.into(),
                replica: 1,
            }),
            evacuate: None,
            unreachable: false,
            limits: Default::default(),
        }
    }

    fn desired(module: &str, availability: Availability) -> DesiredModule {
        DesiredModule {
            module: module.into(),
            spec: InstanceSpec {
                project: String::new(),
                name: String::new(),
                kind: InstanceKind::Container,
                source: ImageSource {
                    protocol: ImageProtocol::Simplestreams,
                    server: String::new(),
                    alias: String::new(),
                    fingerprint: None,
                },
                profiles: vec![],
                config: Default::default(),
                start: true,
                architectures: vec![],
                owner: None,
                network: None,
                volumes: Vec::new(),
                root_pool: None,
            },
            replicas: 2,
            availability,
            internet: Default::default(),
            guest_os: Default::default(),
            readiness: None,
        }
    }

    #[test]
    fn explains_each_instance_and_warns_about_shared_replicas_and_downtime() {
        let members = [
            member("m1", "Online", "aarch64"),
            member("m2", "Online", "aarch64"),
        ];
        let mut stopped_db = inst("shop-db-01", "m1", Some(("shop", "db-01")), "Running");
        stopped_db.evacuate = Some("stop".into());
        let instances = [
            inst("shop-web-01", "m1", Some(("shop", "web-01")), "Running"),
            inst("shop-web-01-r2", "m2", Some(("shop", "web-01")), "Running"),
            stopped_db,
            inst("legacy", "m1", None, "Running"),
        ];
        let d = [
            desired("web-01", Availability::Stateless),
            desired("db-01", Availability::Stateful),
        ];
        let plan = plan_evacuation(
            "m1",
            &members,
            &instances,
            &[Deployed {
                application: "shop",
                project: "apelora",
                desired: &d,
            }],
        )
        .expect("plan");
        assert!(plan.blocked.is_empty(), "{:?}", plan.blocked);
        assert_eq!(plan.targets, ["m2"]);
        let by = |n: &str| {
            plan.items
                .iter()
                .find(|i| i.instance == n)
                .expect(n)
                .clone()
        };
        assert_eq!(by("shop-web-01").impact, "stays up (1 other replica)");
        assert_eq!(
            by("shop-web-01").outcome,
            "moves if it can; stopped during the move"
        );
        assert_eq!(by("shop-db-01").outcome, "stops here until restore");
        assert_eq!(by("shop-db-01").impact, "down until restore (stateful)");
        assert_eq!(by("legacy").impact, "not Apelora's");
        assert!(
            plan.warnings
                .iter()
                .any(|w| w.contains("replicas of shop/web-01 share m2")),
            "{:?}",
            plan.warnings
        );
        assert!(
            plan.warnings
                .iter()
                .any(|w| w == "down during the evacuation: shop-db-01"),
            "{:?}",
            plan.warnings
        );
    }

    #[test]
    fn blocks_without_a_target_or_a_matching_architecture() {
        let instances = [inst("a", "m1", None, "Running")];
        let alone = plan_evacuation("m1", &[member("m1", "Online", "aarch64")], &instances, &[])
            .expect("plan");
        assert_eq!(
            alone.blocked,
            ["no other online member can take its instances"]
        );
        let other_arch = [
            member("m1", "Online", "aarch64"),
            member("m2", "Online", "x86_64"),
        ];
        let plan = plan_evacuation("m1", &other_arch, &instances, &[]).expect("plan");
        assert_eq!(
            plan.blocked,
            ["apelora/a: cannot move: no other member runs aarch64"]
        );
        let evacuated = [
            member("m1", "Evacuated", "aarch64"),
            member("m2", "Online", "aarch64"),
        ];
        assert_eq!(
            plan_evacuation("m1", &evacuated, &[], &[])
                .expect("plan")
                .blocked,
            ["m1 is Evacuated, not Online"]
        );
        assert!(plan_evacuation("nope", &evacuated, &[], &[]).is_err());
    }
}

// SPDX-License-Identifier: AGPL-3.0-only
//! Removing a cluster member for good (Phase 09): the dry run that decides whether it is safe.
//!
//! Incus removes the member (`DELETE /1.0/cluster/members/<name>`); Apelora only checks first,
//! from the inventory and its own volume records, that nothing is lost with it: no instance is
//! still on it (an evacuation stops instances with member-local volumes in place), no recorded
//! volume lives on it, and the cluster keeps running without it. An offline member is not
//! removed here: Incus can only force that, and it is a decision for a runbook.

use serde::{Deserialize, Serialize};

use crate::{ClusterMember, Instance};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct RemovalPlan {
    pub member: String,
    /// The member's status now (`Evacuated`, `Online`, `Offline`…).
    pub status: String,
    /// Instances still on it (`project/name`).
    pub instances: Vec<String>,
    /// Application volumes on it (`project/volume`).
    pub volumes: Vec<String>,
    /// Why removing it now is refused; empty when it may proceed.
    pub blocked: Vec<String>,
    /// Consequences to accept before proceeding.
    pub warnings: Vec<String>,
}

/// A volume Apelora recorded, with the member it lives on.
pub struct RecordedVolume<'a> {
    pub project: &'a str,
    pub name: &'a str,
    pub member: &'a str,
}

/// Decides whether `member` can be removed. `Err` when it is not a cluster member.
pub fn plan_removal(
    member: &str,
    members: &[ClusterMember],
    instances: &[Instance],
    volumes: &[RecordedVolume<'_>],
) -> Result<RemovalPlan, String> {
    let m = members
        .iter()
        .find(|m| m.name == member)
        .ok_or_else(|| format!("{member} is not a cluster member"))?;
    let on_it: Vec<String> = instances
        .iter()
        .filter(|i| i.location == member)
        .map(|i| format!("{}/{}", i.project, i.name))
        .collect();
    let vols: Vec<String> = volumes
        .iter()
        .filter(|v| v.member == member)
        .map(|v| format!("{}/{}", v.project, v.name))
        .collect();
    let others: Vec<&ClusterMember> = members.iter().filter(|x| x.name != member).collect();
    let mut blocked = Vec::new();
    let mut warnings = Vec::new();
    if others.is_empty() {
        blocked.push("it is the only member of the cluster".into());
    }
    match m.status.as_str() {
        "Evacuated" => {}
        "Online" => blocked.push("evacuate it first, so Incus moves its instances away".into()),
        other => blocked.push(format!(
            "it is {other}: Incus can remove it only by force, which may leave its data behind; follow docs/runbooks/member-failure.md"
        )),
    }
    if !on_it.is_empty() {
        blocked.push(format!(
            "{} instance{} still on it ({}); an evacuation leaves instances whose volumes are on the member stopped in place",
            on_it.len(),
            if on_it.len() == 1 { " is" } else { "s are" },
            on_it.join(", ")
        ));
    }
    if !vols.is_empty() {
        blocked.push(format!(
            "application volumes are on it and would be lost ({}); copy their data elsewhere and delete them first",
            vols.join(", ")
        ));
    }
    let online_others = others.iter().filter(|x| x.status == "Online").count();
    if !others.is_empty() && online_others == 0 {
        blocked.push("no other member is online".into());
    }
    if m.database {
        let voters_left = others.iter().filter(|x| x.database).count();
        if voters_left < 3 {
            warnings.push(format!(
                "the cluster keeps {voters_left} database member{} (fewer than three): losing one more member can stop it",
                if voters_left == 1 { "" } else { "s" }
            ));
        }
    }
    if others.len() < 3 && !others.is_empty() {
        warnings.push(format!(
            "{} member{} remain{}: replicas have fewer places to spread over",
            others.len(),
            if others.len() == 1 { "" } else { "s" },
            if others.len() == 1 { "s" } else { "" }
        ));
    }
    warnings.push("to use the server again, reinstall it and enrol it as a new machine".into());
    Ok(RemovalPlan {
        member: member.to_owned(),
        status: m.status.clone(),
        instances: on_it,
        volumes: vols,
        blocked,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::InstanceKind;

    fn member(name: &str, status: &str, database: bool) -> ClusterMember {
        ClusterMember {
            name: name.into(),
            url: String::new(),
            roles: vec![],
            status: status.into(),
            message: String::new(),
            architecture: "x86_64".into(),
            failure_domain: "default".into(),
            database,
            groups: vec![],
            scheduler: "all".into(),
        }
    }

    fn instance(name: &str, location: &str) -> Instance {
        Instance {
            name: name.into(),
            project: "apelora".into(),
            kind: InstanceKind::Container,
            status: "Stopped".into(),
            location: location.into(),
            architecture: "x86_64".into(),
            description: String::new(),
            managed: true,
            owner: None,
            evacuate: None,
            unreachable: false,
            built_from: None,
            evacuated_from: None,
            limits: Default::default(),
        }
    }

    fn four(status_of_4: &str) -> Vec<ClusterMember> {
        vec![
            member("m1", "Online", true),
            member("m2", "Online", true),
            member("m3", "Online", true),
            member("m4", status_of_4, false),
        ]
    }

    #[test]
    fn an_evacuated_empty_member_may_go() {
        let p =
            plan_removal("m4", &four("Evacuated"), &[instance("a", "m1")], &[]).expect("member");
        assert!(p.blocked.is_empty(), "{:?}", p.blocked);
        assert_eq!(p.status, "Evacuated");
        assert!(p.warnings.iter().any(|w| w.contains("reinstall it")));
    }

    #[test]
    fn it_must_be_evacuated_and_reachable() {
        let online = plan_removal("m4", &four("Online"), &[], &[]).expect("member");
        assert!(
            online
                .blocked
                .iter()
                .any(|b| b.contains("evacuate it first"))
        );
        let offline = plan_removal("m4", &four("Offline"), &[], &[]).expect("member");
        assert!(offline.blocked.iter().any(|b| b.contains("only by force")));
        assert!(plan_removal("m9", &four("Evacuated"), &[], &[]).is_err());
    }

    #[test]
    fn nothing_may_be_left_on_it() {
        let vols = [RecordedVolume {
            project: "apelora",
            name: "shop-db-01-data",
            member: "m4",
        }];
        let p = plan_removal(
            "m4",
            &four("Evacuated"),
            &[instance("shop-db-01", "m4")],
            &vols,
        )
        .expect("member");
        assert_eq!(p.instances, vec!["apelora/shop-db-01".to_owned()]);
        assert_eq!(p.volumes, vec!["apelora/shop-db-01-data".to_owned()]);
        assert!(
            p.blocked
                .iter()
                .any(|b| b.contains("1 instance is still on it"))
        );
        assert!(p.blocked.iter().any(|b| b.contains("would be lost")));
    }

    #[test]
    fn the_cluster_must_keep_running_and_quorum_loss_is_named() {
        let alone =
            plan_removal("m1", &[member("m1", "Evacuated", true)], &[], &[]).expect("member");
        assert!(alone.blocked.iter().any(|b| b.contains("only member")));
        let three = vec![
            member("m1", "Online", true),
            member("m2", "Offline", true),
            member("m3", "Evacuated", true),
        ];
        let p = plan_removal("m3", &three, &[], &[]).expect("member");
        assert!(p.blocked.is_empty(), "{:?}", p.blocked);
        assert!(p.warnings.iter().any(|w| w.contains("2 database members")));
        let none_online = vec![
            member("m1", "Offline", true),
            member("m2", "Evacuated", true),
        ];
        let p = plan_removal("m2", &none_online, &[], &[]).expect("member");
        assert!(
            p.blocked
                .iter()
                .any(|b| b.contains("no other member is online"))
        );
    }
}

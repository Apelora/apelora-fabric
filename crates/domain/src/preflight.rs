// SPDX-License-Identifier: AGPL-3.0-only
//! Read-only readiness checks for an Incus server or cluster before Apelora deploys to it.
//! Pure: the caller gathers the observations; this decides. A check that cannot be measured
//! from what was observed says `not-checked`, never `pass`.

use serde::{Deserialize, Serialize};

use crate::{ClusterMember, Network, Project, ServerInfo, StoragePool};

/// Oldest Incus release series Apelora supports (the pinned LTS).
pub const MIN_INCUS_MAJOR: u32 = 7;
/// Oldest kernel Apelora supports, as (major, minor).
pub const MIN_KERNEL: (u32, u32) = (6, 12);
/// API extensions instance lifecycle relies on.
pub const REQUIRED_EXTENSIONS: [&str; 2] = ["instance_create_start", "operation_wait"];
/// Needed only for OCI application containers.
pub const OCI_EXTENSION: &str = "instance_oci";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum CheckStatus {
    Pass,
    NotChecked,
    Warn,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct Check {
    /// Stable identifier, e.g. `kernel-version`.
    pub id: String,
    pub title: String,
    pub status: CheckStatus,
    pub detail: String,
}

/// What preflight looks at.
pub struct Observed<'a> {
    pub server: &'a ServerInfo,
    pub members: &'a [ClusterMember],
    pub projects: &'a [Project],
    pub pools: &'a [StoragePool],
    pub networks: &'a [Network],
    /// Projects Apelora is configured to deploy into.
    pub deploy_projects: &'a [String],
    /// The pool behind the `data-local` volume class (`APELORA_VOLUME_POOL`), if configured.
    pub volume_pool: Option<&'a str>,
}

pub(crate) fn check(
    id: &str,
    title: &str,
    status: CheckStatus,
    detail: impl Into<String>,
) -> Check {
    Check {
        id: id.into(),
        title: title.into(),
        status,
        detail: detail.into(),
    }
}

/// Leading `major.minor` of a version string such as `7.0.2` or `6.12.0-1-amd64`.
pub(crate) fn major_minor(v: &str) -> Option<(u32, u32)> {
    let mut parts = v.split(|c: char| !c.is_ascii_digit());
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().and_then(|m| m.parse().ok()).unwrap_or(0);
    Some((major, minor))
}

pub fn evaluate(o: &Observed<'_>) -> Vec<Check> {
    use CheckStatus::*;
    let mut out = Vec::new();

    out.push(
        match o
            .server
            .server_version
            .as_deref()
            .map(|v| (v, major_minor(v)))
        {
            None => check(
                "incus-version",
                "Incus version",
                NotChecked,
                "the server did not report its version",
            ),
            Some((v, Some((major, _)))) if major >= MIN_INCUS_MAJOR => {
                check("incus-version", "Incus version", Pass, format!("Incus {v}"))
            }
            Some((v, _)) => check(
                "incus-version",
                "Incus version",
                Fail,
                format!("Incus {v}; Apelora needs {MIN_INCUS_MAJOR}.0 or later"),
            ),
        },
    );

    out.push(
        match o
            .server
            .kernel_version
            .as_deref()
            .map(|v| (v, major_minor(v)))
        {
            None => check(
                "kernel-version",
                "Kernel version",
                NotChecked,
                "the server did not report its kernel",
            ),
            Some((v, Some(mm))) if mm >= MIN_KERNEL => check(
                "kernel-version",
                "Kernel version",
                Pass,
                format!("Linux {v}"),
            ),
            Some((v, _)) => check(
                "kernel-version",
                "Kernel version",
                Fail,
                format!(
                    "Linux {v}; Apelora needs {}.{} or later",
                    MIN_KERNEL.0, MIN_KERNEL.1
                ),
            ),
        },
    );

    let has = |e: &str| o.server.api_extensions.iter().any(|x| x == e);
    let missing: Vec<&str> = REQUIRED_EXTENSIONS
        .into_iter()
        .filter(|e| !has(e))
        .collect();
    out.push(if missing.is_empty() {
        check(
            "api-extensions",
            "Required API extensions",
            Pass,
            REQUIRED_EXTENSIONS.join(", "),
        )
    } else {
        check(
            "api-extensions",
            "Required API extensions",
            Fail,
            format!("missing: {}", missing.join(", ")),
        )
    });
    out.push(if has(OCI_EXTENSION) {
        check("oci", "OCI application containers", Pass, OCI_EXTENSION)
    } else {
        check(
            "oci",
            "OCI application containers",
            Warn,
            format!("{OCI_EXTENSION} is missing; OCI appliances cannot be deployed"),
        )
    });

    let offline: Vec<&str> = o
        .members
        .iter()
        .filter(|m| m.status != "Online")
        .map(|m| m.name.as_str())
        .collect();
    out.push(if !o.server.clustered {
        check(
            "members",
            "Cluster members",
            Warn,
            "a single Incus server: no failover between members",
        )
    } else if o.members.is_empty() {
        check(
            "members",
            "Cluster members",
            Fail,
            "the cluster reports no members",
        )
    } else if offline.is_empty() {
        check(
            "members",
            "Cluster members",
            Pass,
            format!("{} online", o.members.len()),
        )
    } else {
        check(
            "members",
            "Cluster members",
            Warn,
            format!("not online: {}", offline.join(", ")),
        )
    });

    for name in o.deploy_projects {
        let id = format!("project-{name}");
        let title = format!("Project {name}");
        out.push(match o.projects.iter().find(|p| &p.name == name) {
            None => check(
                &id,
                &title,
                Fail,
                // A restricted identity only sees the projects it may use, so a project that
                // exists but was not granted (or lost its grant when it was recreated) looks
                // the same as a missing one.
                format!(
                    "project {name} is not visible to Apelora's Incus identity: create it with \
                     restricted=true, or grant it to the identity (its certificate's projects)"
                ),
            ),
            Some(p) if !p.restricted => check(
                &id,
                &title,
                Warn,
                "exists but is not restricted (restricted=true limits what its instances may do)",
            ),
            Some(_) => check(&id, &title, Pass, "exists and is restricted"),
        });
    }

    let pools: Vec<&str> = o
        .pools
        .iter()
        .filter(|p| p.status == "Created")
        .map(|p| p.name.as_str())
        .collect();
    out.push(if pools.is_empty() {
        check("storage", "Storage pools", Fail, "no storage pool is ready")
    } else {
        check("storage", "Storage pools", Pass, pools.join(", "))
    });

    out.push(match o.volume_pool {
        None => check(
            "volume-pool",
            "Pool for volumes",
            Warn,
            "none configured (APELORA_VOLUME_POOL): designs with volumes are refused",
        ),
        Some(v) if pools.contains(&v) => check(
            "volume-pool",
            "Pool for volumes",
            Pass,
            format!("{v} (volume class data-local)"),
        ),
        Some(v) => check(
            "volume-pool",
            "Pool for volumes",
            Fail,
            format!("APELORA_VOLUME_POOL is {v}, which is not a ready storage pool"),
        ),
    });

    let nets: Vec<&str> = o
        .networks
        .iter()
        .filter(|n| n.managed && n.status == "Created")
        .map(|n| n.name.as_str())
        .collect();
    out.push(if nets.is_empty() {
        check(
            "network",
            "Managed networks",
            Warn,
            "no managed network is ready; instances need a network from a profile",
        )
    } else {
        check("network", "Managed networks", Pass, nets.join(", "))
    });

    for (id, title) in [
        ("clock", "Clock agreement between members"),
        ("latency", "Latency between members (≤ 10 ms)"),
    ] {
        out.push(check(
            id,
            title,
            NotChecked,
            "not measurable through the Incus API; check on the members",
        ));
    }
    out
}

/// The worst status among the checks (`pass` when there are none).
pub fn overall(checks: &[Check]) -> CheckStatus {
    checks
        .iter()
        .map(|c| c.status)
        .max()
        .unwrap_or(CheckStatus::Pass)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(version: Option<&str>, kernel: Option<&str>, ext: &[&str]) -> ServerInfo {
        ServerInfo {
            architectures: vec![],
            api_version: "1.0".into(),
            server_version: version.map(Into::into),
            kernel_version: kernel.map(Into::into),
            api_extensions: ext.iter().map(|e| (*e).to_owned()).collect(),
            clustered: true,
            auth: "trusted".into(),
        }
    }
    fn member(name: &str, status: &str) -> ClusterMember {
        ClusterMember {
            name: name.into(),
            url: String::new(),
            roles: vec![],
            status: status.into(),
            message: String::new(),
            architecture: "x86_64".into(),
            failure_domain: String::new(),
            database: true,
            groups: vec![],
            scheduler: "all".into(),
        }
    }
    fn status_of(checks: &[Check], id: &str) -> CheckStatus {
        checks
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.status)
            .expect(id)
    }

    #[test]
    fn a_ready_cluster_passes_and_unmeasured_checks_say_so() {
        let s = server(
            Some("7.0.2"),
            Some("6.12.48-1-amd64"),
            &["instance_create_start", "operation_wait", "instance_oci"],
        );
        let members = [member("m1", "Online"), member("m2", "Online")];
        let projects = [Project {
            name: "apelora".into(),
            description: String::new(),
            restricted: true,
        }];
        let pools = [StoragePool {
            name: "local".into(),
            driver: "zfs".into(),
            status: "Created".into(),
            description: String::new(),
            locations: vec![],
        }];
        let networks = [Network {
            name: "br0".into(),
            project: "default".into(),
            kind: "bridge".into(),
            managed: true,
            status: "Created".into(),
            description: String::new(),
            locations: vec![],
        }];
        let deploy = ["apelora".to_owned()];
        let checks = evaluate(&Observed {
            server: &s,
            members: &members,
            projects: &projects,
            pools: &pools,
            networks: &networks,
            deploy_projects: &deploy,
            volume_pool: Some("local"),
        });
        for c in &checks {
            let want = if c.id == "clock" || c.id == "latency" {
                CheckStatus::NotChecked
            } else {
                CheckStatus::Pass
            };
            assert_eq!(c.status, want, "{c:?}");
        }
        assert_eq!(overall(&checks), CheckStatus::NotChecked);

        // The pool for volumes: missing is a warning, one Incus does not have is a failure.
        let with = |volume_pool| {
            evaluate(&Observed {
                server: &s,
                members: &members,
                projects: &projects,
                pools: &pools,
                networks: &networks,
                deploy_projects: &deploy,
                volume_pool,
            })
            .into_iter()
            .find(|c| c.id == "volume-pool")
            .map(|c| c.status)
        };
        assert_eq!(with(None), Some(CheckStatus::Warn));
        assert_eq!(with(Some("fast-ssd")), Some(CheckStatus::Fail));
    }

    #[test]
    fn problems_are_reported_with_the_worst_first_in_overall() {
        let s = server(Some("6.0.4"), Some("6.8.0"), &["operation_wait"]);
        let members = [member("m1", "Online"), member("m2", "Offline")];
        let projects = [Project {
            name: "apelora".into(),
            description: String::new(),
            restricted: false,
        }];
        let deploy = ["apelora".to_owned(), "edge".to_owned()];
        let checks = evaluate(&Observed {
            server: &s,
            members: &members,
            projects: &projects,
            pools: &[],
            networks: &[],
            deploy_projects: &deploy,
            volume_pool: None,
        });
        use CheckStatus::*;
        for (id, want) in [
            ("incus-version", Fail),
            ("kernel-version", Fail),
            ("api-extensions", Fail),
            ("oci", Warn),
            ("members", Warn),
            ("project-apelora", Warn),
            ("project-edge", Fail),
            ("storage", Fail),
            ("network", Warn),
        ] {
            assert_eq!(status_of(&checks, id), want, "{id}");
        }
        assert!(
            checks
                .iter()
                .any(|c| c.id == "api-extensions" && c.detail.contains("instance_create_start"))
        );
        assert_eq!(overall(&checks), Fail);
    }

    #[test]
    fn versions_parse_leniently() {
        assert_eq!(major_minor("7.0.2"), Some((7, 0)));
        assert_eq!(major_minor("6.12.48-1-amd64"), Some((6, 12)));
        assert_eq!(major_minor("7"), Some((7, 0)));
        assert_eq!(major_minor("dev"), None);
        let unknown = server(None, None, &[]);
        let checks = evaluate(&Observed {
            server: &unknown,
            members: &[],
            projects: &[],
            pools: &[],
            networks: &[],
            deploy_projects: &[],
            volume_pool: None,
        });
        assert_eq!(status_of(&checks, "incus-version"), CheckStatus::NotChecked);
        assert_eq!(
            status_of(&checks, "kernel-version"),
            CheckStatus::NotChecked
        );
    }
}

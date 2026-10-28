// SPDX-License-Identifier: AGPL-3.0-only
//! Where an application would live when an installation has more than one site (ADR 0008, ADR 0009).
//!
//! An application stays in one sector. Surviving a site means a second deployment of that
//! application in a sector of another site. Incus places instances inside the sector; this
//! module only names the sector and checks how other sectors are registered. It does not open
//! an Incus connection, move anything, or peer networks.

/// Enrolment warns at this many members in one sector (ADR 0008). This decision does not enrol.
pub const MEMBER_WARN: u32 = 50;

/// One Incus cluster, in one site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sector {
    pub site: String,
    pub name: String,
    /// Members already in the sector. Not a reason to refuse a deployment.
    pub members: u32,
}

/// The sector a second deployment would use, or the reason none is planned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteSurvival {
    /// A sector in another site was named. The deployment is not started.
    pub planned: bool,
    pub region: String,
    pub site: String,
    pub sector: String,
    pub members: u32,
    /// Distinct sites in `sectors`.
    pub connected_sites: u32,
    /// One sentence for the Console. It says when the deployment is not started.
    pub summary: String,
    pub warning: Option<String>,
    pub second_site: Option<String>,
    pub second_sector: Option<String>,
}

/// Names the sector for a second deployment, or refuses while every sector is in `home`'s site.
///
/// `home` is the sector this control plane talks to. `sectors` is every sector Apelora is
/// connected to, including `home`. A later connection can pass more than one; today the API
/// passes the one cluster it can reach.
pub fn survival(region: &str, home: &Sector, sectors: &[Sector]) -> SiteSurvival {
    if let Some(reason) = invalid_label("region", region) {
        return refuse(home, region, 0, reason);
    }
    for sector in sectors {
        if let Some(reason) = invalid_label("site", &sector.site) {
            return refuse(home, region, 0, reason);
        }
        if let Some(reason) = invalid_label("sector", &sector.name) {
            return refuse(home, region, 0, reason);
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    for sector in sectors {
        if !seen.insert((sector.site.as_str(), sector.name.as_str())) {
            return refuse(
                home,
                region,
                count_sites(sectors),
                format!("Sector {}/{} is listed twice.", sector.site, sector.name),
            );
        }
    }
    let Some(known) = sectors
        .iter()
        .find(|s| s.site == home.site && s.name == home.name)
    else {
        return refuse(
            home,
            region,
            count_sites(sectors),
            "The connected sector is not among the sectors Apelora knows.".to_owned(),
        );
    };
    let connected = count_sites(sectors);
    let mut others: Vec<&Sector> = sectors.iter().filter(|s| s.site != known.site).collect();
    if others.is_empty() {
        let same = sectors.iter().filter(|s| s.site == known.site).count() > 1;
        let extra = if same {
            " Other sectors here are in the same site, so they do not survive a site failure."
        } else {
            ""
        };
        return SiteSurvival {
            planned: false,
            region: region.to_owned(),
            site: known.site.clone(),
            sector: known.name.clone(),
            members: known.members,
            connected_sites: connected,
            summary: format!(
                "Connected to site {site}, sector {name} ({members} members), in region {region}. Surviving a site means deploying the application again in another site's sector. No other site is connected, so that deployment is not planned.{extra}",
                site = known.site,
                name = known.name,
                members = known.members,
            ),
            warning: None,
            second_site: None,
            second_sector: None,
        };
    }
    others.sort_by(|a, b| {
        a.members
            .cmp(&b.members)
            .then(a.site.cmp(&b.site))
            .then(a.name.cmp(&b.name))
    });
    let second = others[0];
    let warning = match (enrolment_warning(known), enrolment_warning(second)) {
        (Some(a), Some(b)) => Some(format!("{a} {b}")),
        (Some(a), None) | (None, Some(a)) => Some(a),
        (None, None) => None,
    };
    SiteSurvival {
        planned: true,
        region: region.to_owned(),
        site: known.site.clone(),
        sector: known.name.clone(),
        members: known.members,
        connected_sites: connected,
        summary: format!(
            "Connected to site {site}, sector {name} ({members} members), in region {region}. A second deployment of the application would be recorded in site {other_site}, sector {other_name} ({other_members} members). This control plane does not start that deployment. Incus places instances inside one sector.",
            site = known.site,
            name = known.name,
            members = known.members,
            other_site = second.site,
            other_name = second.name,
            other_members = second.members,
        ),
        warning,
        second_site: Some(second.site.clone()),
        second_sector: Some(second.name.clone()),
    }
}

fn refuse(home: &Sector, region: &str, connected: u32, summary: String) -> SiteSurvival {
    SiteSurvival {
        planned: false,
        region: region.to_owned(),
        site: home.site.clone(),
        sector: home.name.clone(),
        members: home.members,
        connected_sites: connected,
        summary,
        warning: None,
        second_site: None,
        second_sector: None,
    }
}

fn count_sites(sectors: &[Sector]) -> u32 {
    let mut sites = std::collections::BTreeSet::new();
    for sector in sectors {
        sites.insert(sector.site.as_str());
    }
    u32::try_from(sites.len()).unwrap_or(u32::MAX)
}

fn enrolment_warning(sector: &Sector) -> Option<String> {
    (sector.members >= MEMBER_WARN).then(|| {
        format!(
            "Sector {}/{} has {} members. Enrolment warns at {MEMBER_WARN}.",
            sector.site, sector.name, sector.members
        )
    })
}

/// Another Incus cluster Apelora is told about, besides the one it runs against (ADR 0008):
/// reached at its own address with its own identity. Only what is not secret is stored; the
/// identity's certificate and key and the cluster's certificate are files named by environment
/// variables on the control plane ([`credential_vars`]), like Apelora's other Incus identities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectorRegistration {
    pub name: String,
    pub site: String,
    /// `https://host:port` of the sector's Incus API.
    pub url: String,
    /// Names the variables that hold the identity's files: `APELORA_SECTOR_<CREDENTIALS>_…`.
    pub credentials: String,
}

/// Why a registration cannot be stored, or `None`.
pub fn check_registration(r: &SectorRegistration) -> Option<String> {
    if let Some(reason) = invalid_label("sector", &r.name) {
        return Some(reason);
    }
    if let Some(reason) = invalid_label("site", &r.site) {
        return Some(reason);
    }
    let authority = r
        .url
        .strip_prefix("https://")
        .map(|rest| rest.strip_suffix('/').unwrap_or(rest))
        .unwrap_or_default();
    if authority.is_empty()
        || authority.contains(['/', '?', '#', '@', ' '])
        || authority.len() > 253
    {
        return Some(format!(
            "url {:?} is not an Incus address like https://incus.site-b.example:8443",
            r.url
        ));
    }
    let c = r.credentials.as_bytes();
    if c.is_empty()
        || c.len() > 40
        || !c
            .iter()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || *b == b'_')
    {
        return Some(format!(
            "credentials {:?} must be 1-40 capital letters, digits and '_' (it names APELORA_SECTOR_<CREDENTIALS>_CLIENT_CERT and the others)",
            r.credentials
        ));
    }
    None
}

/// Where the control plane finds a registered sector's identity: the client certificate, its key
/// and the sector's server certificate, each a file path.
pub fn credential_vars(credentials: &str) -> [String; 3] {
    ["CLIENT_CERT", "CLIENT_KEY", "SERVER_CERT"]
        .map(|f| format!("APELORA_SECTOR_{credentials}_{f}"))
}

/// Same shape as a metric place label (`apelora_metrics::valid_place_name`).
fn invalid_label(kind: &str, value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let ok = !bytes.is_empty()
        && bytes.len() <= 63
        && bytes
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
        && bytes[0] != b'-'
        && bytes[bytes.len() - 1] != b'-';
    if ok {
        None
    } else {
        Some(format!(
            "{kind} {value:?} is not usable: 1-63 lowercase letters, digits and '-', starting and ending with a letter or digit."
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sector(site: &str, name: &str, members: u32) -> Sector {
        Sector {
            site: site.to_owned(),
            name: name.to_owned(),
            members,
        }
    }

    #[test]
    fn one_site_is_not_planned() {
        let home = sector("default", "default", 3);
        let plan = survival("default", &home, std::slice::from_ref(&home));
        assert!(!plan.planned);
        assert_eq!(plan.connected_sites, 1);
        assert_eq!(plan.second_site, None);
        assert_eq!(
            plan.summary,
            "Connected to site default, sector default (3 members), in region default. Surviving a site means deploying the application again in another site's sector. No other site is connected, so that deployment is not planned."
        );
    }

    #[test]
    fn another_sector_in_the_same_site_does_not_survive_the_site() {
        let home = sector("lab", "a", 4);
        let plan = survival("north", &home, &[home.clone(), sector("lab", "b", 1)]);
        assert!(!plan.planned);
        assert_eq!(plan.connected_sites, 1);
        assert!(plan.summary.contains("same site"), "{}", plan.summary);
    }

    #[test]
    fn a_second_site_is_named_and_not_started() {
        let home = sector("west", "a", 10);
        let plan = survival(
            "north",
            &home,
            &[home.clone(), sector("east", "b", 2), sector("east", "c", 9)],
        );
        assert!(plan.planned);
        assert_eq!(plan.connected_sites, 2);
        assert_eq!(plan.second_site.as_deref(), Some("east"));
        assert_eq!(plan.second_sector.as_deref(), Some("b"));
        assert!(plan.summary.contains("does not start"), "{}", plan.summary);
        assert!(plan.warning.is_none());
    }

    #[test]
    fn the_smaller_other_site_is_chosen_and_ties_follow_the_name() {
        let home = sector("home", "a", 1);
        let plan = survival(
            "north",
            &home,
            &[
                home.clone(),
                sector("zeta", "b", 5),
                sector("alpha", "m", 5),
                sector("alpha", "a", 5),
            ],
        );
        assert_eq!(plan.second_site.as_deref(), Some("alpha"));
        assert_eq!(plan.second_sector.as_deref(), Some("a"));
    }

    #[test]
    fn fifty_members_warns_and_still_names_the_site() {
        let home = sector("west", "a", MEMBER_WARN);
        let plan = survival("north", &home, &[home.clone(), sector("east", "b", 51)]);
        assert!(plan.planned);
        let warning = plan.warning.expect("warning");
        assert!(warning.contains("west/a has 50"), "{warning}");
        assert!(warning.contains("east/b has 51"), "{warning}");
    }

    #[test]
    fn a_missing_or_repeated_sector_is_refused() {
        let home = sector("west", "a", 1);
        let missing = survival("north", &home, &[sector("east", "b", 1)]);
        assert!(!missing.planned);
        assert!(missing.summary.contains("not among"), "{}", missing.summary);

        let twice = survival("north", &home, &[home.clone(), home.clone()]);
        assert!(!twice.planned);
        assert!(twice.summary.contains("listed twice"), "{}", twice.summary);
    }

    #[test]
    fn a_bad_name_is_refused() {
        let home = sector("West", "a", 1);
        let plan = survival("north", &home, std::slice::from_ref(&home));
        assert!(!plan.planned);
        assert!(plan.summary.contains("not usable"), "{}", plan.summary);

        let long = "a".repeat(64);
        let home = sector("west", &long, 1);
        assert!(!survival("north", &home, std::slice::from_ref(&home)).planned);
    }

    fn registration() -> SectorRegistration {
        SectorRegistration {
            name: "rack-b1".into(),
            site: "site-b".into(),
            url: "https://incus.site-b.example:8443".into(),
            credentials: "SITE_B".into(),
        }
    }

    #[test]
    fn a_registration_names_its_sector_and_site_and_holds_no_secret() {
        assert_eq!(check_registration(&registration()), None);
        for bad in [
            SectorRegistration {
                name: "Rack B".into(),
                ..registration()
            },
            SectorRegistration {
                site: String::new(),
                ..registration()
            },
            SectorRegistration {
                url: "http://incus.site-b.example:8443".into(),
                ..registration()
            },
            SectorRegistration {
                url: "incus.site-b.example".into(),
                ..registration()
            },
            SectorRegistration {
                url: "https://user@incus.example".into(),
                ..registration()
            },
            SectorRegistration {
                url: "https://incus.example/1.0?x".into(),
                ..registration()
            },
            SectorRegistration {
                credentials: "site-b".into(),
                ..registration()
            },
            SectorRegistration {
                credentials: "/etc/apelora/admin.key".into(),
                ..registration()
            },
        ] {
            assert!(check_registration(&bad).is_some(), "{bad:?}");
        }
        assert_eq!(
            credential_vars("SITE_B"),
            [
                "APELORA_SECTOR_SITE_B_CLIENT_CERT",
                "APELORA_SECTOR_SITE_B_CLIENT_KEY",
                "APELORA_SECTOR_SITE_B_SERVER_CERT"
            ]
        );
    }
}

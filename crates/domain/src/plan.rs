// SPDX-License-Identifier: AGPL-3.0-only
//! Deployment plans: what applying a design would change, decided only from the desired
//! instances and the instances Incus reports. Pure, so the API can show a preview and then
//! recompute the same plan when the change is applied (docs/design/application-deployment.md).

use serde::{Deserialize, Serialize};

use crate::{Instance, InstanceSpec, Owner, is_valid_name};

/// At most this many replicas per module.
pub const MAX_REPLICAS: u8 = 9;

/// The instance name of a module's first replica: `<application>-<module>`.
pub fn instance_name(application: &str, module: &str) -> String {
    format!("{application}-{module}")
}

/// The instance name of replica `replica` (1-based): the first keeps the plain name, the
/// others get `-r<n>`, so a single-replica deployment keeps its names when replicas are added.
pub fn replica_name(application: &str, module: &str, replica: u8) -> String {
    if replica <= 1 {
        instance_name(application, module)
    } else {
        format!("{}-r{replica}", instance_name(application, module))
    }
}

/// The name of a replica's `n`th substitute: created on another member while the member
/// holding the replica was down (Incus cannot move instances off a failed member).
pub fn substitute_name(application: &str, module: &str, replica: u8, n: u8) -> String {
    format!("{}-s{n}", replica_name(application, module, replica))
}

/// The name of a replica's `n`th update: the instance a rolling update creates beside it with
/// the new image or settings, which takes the replica's place once it is healthy.
pub fn update_name(application: &str, module: &str, replica: u8, n: u8) -> String {
    format!("{}-u{n}", replica_name(application, module, replica))
}

/// The suffix `name` has after replica `base`: `""` for the replica itself, `s` for a
/// substitute (`<base>-s<n>`), `u` for an update (`<base>-u<n>`); `None` for another slot.
fn slot_role(name: &str, base: &str) -> Option<&'static str> {
    let rest = name.strip_prefix(base)?;
    if rest.is_empty() {
        return Some("");
    }
    let numbered = |r: &str| !r.is_empty() && r.bytes().all(|b| b.is_ascii_digit());
    match rest {
        r if r.strip_prefix("-s").is_some_and(numbered) => Some("s"),
        r if r.strip_prefix("-u").is_some_and(numbered) => Some("u"),
        _ => None,
    }
}

/// Whether `name` is replica `base` itself, one of its substitutes or one of its updates.
fn in_slot(name: &str, base: &str) -> bool {
    slot_role(name, base).is_some()
}

/// The application's instances that fill replica `replica` of `module` in `project`: the
/// replica, its substitutes and its updates, recognised by Apelora's labels and their names. An instance
/// whose member is unreachable has no visible labels; one with the slot's name counts (names
/// are unique in a project, and the name was Apelora's before its member went away).
pub fn slot_instances<'a>(
    application: &str,
    project: &str,
    module: &str,
    replica: u8,
    observed: &'a [Instance],
) -> Vec<&'a Instance> {
    let base = replica_name(application, module, replica);
    observed
        .iter()
        .filter(|i| {
            i.project == project
                && in_slot(&i.name, &base)
                && (i.unreachable
                    || i.managed
                        && i.owner.as_ref().is_some_and(|o| {
                            o.application == application
                                && o.module == module
                                && o.replica == replica
                        }))
        })
        .collect()
}

/// How a module may be run and recovered (from its catalog class).
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

/// One module of the design, turned into the instances it should run as.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct DesiredModule {
    /// The module's stable id in the design (e.g. `nginx-01`).
    #[serde(alias = "appliance")]
    pub module: String,
    /// Image, kind and settings; the control plane sets the name, project and owner.
    pub spec: InstanceSpec,
    /// How many instances (1–9); more than one only for stateless modules.
    #[serde(default = "one")]
    pub replicas: u8,
    #[serde(default)]
    pub availability: Availability,
    /// What it may reach outside the application (enforced with the connections).
    #[serde(default)]
    pub internet: crate::network::Internet,
    /// The operating system inside (from the class): decides which sessions it offers.
    #[serde(default)]
    pub guest_os: crate::access::GuestOs,
    /// A command that runs inside each instance and exits 0 when the application is ready
    /// (from the class); the port check alone cannot tell a hung application from a ready one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readiness: Option<crate::readiness::Readiness>,
    /// This module answers service names for the application. Absent
    /// means it does not: a resolver is never added unless the design asks for one.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dns_resolver: bool,
}

impl DesiredModule {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=MAX_REPLICAS).contains(&self.replicas) {
            return Err(format!(
                "{}: replicas must be 1–{MAX_REPLICAS}",
                self.module
            ));
        }
        if self.replicas > 1 && self.availability != Availability::Stateless {
            return Err(format!(
                "{}: only stateless modules can have more than one replica",
                self.module
            ));
        }
        if let Some(r) = &self.readiness {
            r.validate(&self.module)?;
        }
        // A stateless replica may be recreated anywhere, which a member-local volume cannot follow.
        if !self.spec.volumes.is_empty() && self.availability == Availability::Stateless {
            return Err(format!(
                "{}: stateless modules cannot have volumes; make it a singleton or stateful",
                self.module
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
    /// The application's instance exists with other resource limits than the design asks for:
    /// its CPU and memory limits are changed in place (`changes` says which).
    Resize,
    /// The instance runs another image or settings than the design asks for (`outdated` says
    /// which): a new replica is created beside it with the design's, and this one is removed
    /// once the new one is healthy, one replica at a time. Stateless modules only.
    Replace,
    /// The application's instance is no longer in the design.
    Remove,
    /// The name is taken by an instance that is not this module's: nothing can be applied.
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct PlanItem {
    /// The module in the design (for `remove`: the label of the instance being removed).
    #[serde(alias = "appliance")]
    pub module: String,
    /// 1-based replica number.
    pub replica: u8,
    pub instance: String,
    pub action: PlanAction,
    /// Why, in a sentence (e.g. what the conflicting instance is).
    pub reason: String,
    /// For `resize`: each limit that changes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<LimitChange>,
    /// What the running instance was built from that the design now asks differently:
    /// `image`, `settings` or `storage`. A stateless replica is replaced (`replace`); a singleton
    /// or stateful module is kept as it is (updating those is not built yet).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outdated: Vec<String>,
}

/// The instance config keys a resize may change: the resource limits Apelora writes from a
/// module's CPU and memory. Nothing else on an instance is changed in place.
pub const RESIZABLE_KEYS: [&str; 3] = ["limits.cpu", "limits.cpu.allowance", "limits.memory"];

/// One limit that changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct LimitChange {
    pub key: String,
    /// The value now; absent when no limit is set.
    pub from: Option<String>,
    /// The value after; absent when the limit is removed.
    pub to: Option<String>,
}

/// Memory as bytes, for comparing `1GiB` with `1024MiB`; `None` when it is not a size Apelora
/// writes or Incus reports (a plain number of bytes or a binary or decimal unit).
fn memory_bytes(v: &str) -> Option<u128> {
    let v = v.trim();
    let split = v.find(|c: char| !c.is_ascii_digit()).unwrap_or(v.len());
    let (digits, unit) = v.split_at(split);
    let n: u128 = digits.parse().ok()?;
    let mult: u128 = match unit {
        "" | "B" => 1,
        "kB" => 1_000,
        "MB" => 1_000_000,
        "GB" => 1_000_000_000,
        "TB" => 1_000_000_000_000,
        "KiB" => 1 << 10,
        "MiB" => 1 << 20,
        "GiB" => 1 << 30,
        "TiB" => 1 << 40,
        _ => return None,
    };
    n.checked_mul(mult)
}

fn same_limit(key: &str, a: &str, b: &str) -> bool {
    a == b
        || (key == "limits.memory"
            && matches!((memory_bytes(a), memory_bytes(b)), (Some(x), Some(y)) if x == y))
}

/// The limits that differ between what the design asks for (`wanted`: the instance config a
/// deployment would create) and what the instance has now. Only [`RESIZABLE_KEYS`] are compared;
/// a limit the design no longer sets is removed.
pub fn limit_changes(
    wanted: &std::collections::BTreeMap<String, String>,
    current: &std::collections::BTreeMap<String, String>,
) -> Vec<LimitChange> {
    RESIZABLE_KEYS
        .iter()
        .filter_map(|key| {
            let want = wanted.get(*key).filter(|v| !v.is_empty());
            let have = current.get(*key).filter(|v| !v.is_empty());
            match (want, have) {
                (None, None) => None,
                (Some(w), Some(h)) if same_limit(key, w, h) => None,
                (w, h) => Some(LimitChange {
                    key: (*key).to_owned(),
                    from: h.cloned(),
                    to: w.cloned(),
                }),
            }
        })
        .collect()
}

fn describe(changes: &[LimitChange]) -> String {
    let name = |k: &str| match k {
        "limits.cpu" => "CPUs",
        "limits.cpu.allowance" => "CPU quota",
        _ => "memory",
    };
    changes
        .iter()
        .map(|c| {
            format!(
                "{} {} → {}",
                name(&c.key),
                c.from.as_deref().unwrap_or("no limit"),
                c.to.as_deref().unwrap_or("no limit")
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct Plan {
    pub items: Vec<PlanItem>,
    /// Facts the designer should know before applying, e.g. single points of failure.
    pub warnings: Vec<String>,
    /// The deployment against the quotas above its environment; absent when no
    /// quota applies. `allowed: false` means applying is refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota: Option<crate::tenancy::QuotaCheck>,
}

impl Plan {
    pub fn has_conflicts(&self) -> bool {
        self.items.iter().any(|i| i.action == PlanAction::Conflict)
    }

    pub fn count(&self, action: PlanAction) -> usize {
        self.items.iter().filter(|i| i.action == action).count()
    }
}

/// What an instance is built from, recorded at creation as [`crate::SPEC_KEY`]: one short hash per
/// part a change would need a new instance for. `image`: kind, image and architectures.
/// `settings`: instance config and profiles, without resource limits (resized in place),
/// Apelora's discovery variables (`environment.APELORA_*`, re-synced by the network sync) and its
/// labels. `storage`: the root disk pool and each volume's name, class, content and path, without
/// sizes (grown in place) and pools (resolved from the class).
pub fn spec_fingerprint(spec: &InstanceSpec) -> String {
    use sha2::{Digest, Sha256};
    let hash = |v: serde_json::Value| {
        let digest = Sha256::digest(v.to_string().as_bytes());
        digest
            .iter()
            .take(6)
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };
    let image = serde_json::json!([
        spec.kind,
        spec.source.protocol,
        spec.source.server,
        spec.source.alias,
        spec.source.fingerprint,
        spec.architectures
    ]);
    let config: std::collections::BTreeMap<&String, &String> = spec
        .config
        .iter()
        .filter(|(k, _)| {
            !RESIZABLE_KEYS.contains(&k.as_str())
                && !k.starts_with("environment.APELORA_")
                && !k.starts_with("user.apelora.")
        })
        .collect();
    let settings = serde_json::json!([config, spec.profiles]);
    let volumes: Vec<_> = spec
        .volumes
        .iter()
        .map(|v| serde_json::json!([v.name, v.class, v.content, v.path]))
        .collect();
    let storage = serde_json::json!([spec.root_pool, volumes]);
    format!(
        "image={},settings={},storage={}",
        hash(image),
        hash(settings),
        hash(storage)
    )
}

/// The parts of `recorded` (an instance's [`spec_fingerprint`]) that differ from `wanted`, in
/// order: `image`, `settings`, `storage`. A part missing from either side is not compared.
pub fn outdated_parts(recorded: &str, wanted: &str) -> Vec<String> {
    let parts = |f: &str| -> std::collections::BTreeMap<String, String> {
        f.split(',')
            .filter_map(|p| p.split_once('='))
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect()
    };
    let (have, want) = (parts(recorded), parts(wanted));
    ["image", "settings", "storage"]
        .into_iter()
        .filter(|k| matches!((have.get(*k), want.get(*k)), (Some(h), Some(w)) if h != w))
        .map(str::to_owned)
        .collect()
}

/// The spec replica `replica` of a module is created with: its name, project and
/// ownership labels are set here, never taken from the caller.
pub fn owned_spec(
    application: &str,
    project: &str,
    desired: &DesiredModule,
    replica: u8,
) -> InstanceSpec {
    InstanceSpec {
        project: project.to_owned(),
        name: replica_name(application, &desired.module, replica),
        owner: Some(Owner {
            application: application.to_owned(),
            module: desired.module.clone(),
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
    desired: &DesiredModule,
    replica: u8,
    uplink: Option<&str>,
) -> InstanceSpec {
    let mut spec = owned_spec(application, project, desired, replica);
    if let Some(uplink) = uplink {
        spec.network = Some(crate::network::attachment(
            application,
            &desired.module,
            uplink,
        ));
    }
    spec
}

/// Compares the design with the instances Incus reports in `project`.
pub fn plan(
    application: &str,
    project: &str,
    desired: &[DesiredModule],
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
        let fingerprint = spec_fingerprint(&d.spec);
        for replica in 1..=d.replicas.max(1) {
            let name = replica_name(application, &d.module, replica);
            wanted.push(name.clone());
            let item = |action, reason: String| PlanItem {
                module: d.module.clone(),
                replica,
                instance: name.clone(),
                action,
                reason,
                changes: Vec::new(),
                outdated: Vec::new(),
            };
            if !is_valid_name(&name) {
                items.push(item(
                    PlanAction::Conflict,
                    format!("the instance name {name:?} is not valid (at most 63 letters, digits and hyphens)"),
                ));
                continue;
            }
            let slot = slot_instances(application, project, &d.module, replica, observed);
            for i in &slot {
                wanted.push(i.name.clone());
                let role = match slot_role(&i.name, &name) {
                    Some("s") => format!("substitute for {name}; "),
                    _ => String::new(),
                };
                let base = PlanItem {
                    instance: i.name.clone(),
                    ..item(PlanAction::Keep, String::new())
                };
                if i.unreachable {
                    items.push(PlanItem {
                        reason: format!("{role}its member {} cannot be reached", i.location),
                        ..base
                    });
                    continue;
                }
                // A substitute is temporary (one of the slot's two instances is retired when
                // the failed member returns): it is kept as it is.
                if !role.is_empty() {
                    items.push(PlanItem {
                        reason: format!("{role}kept as it is ({})", i.status),
                        ..base
                    });
                    continue;
                }
                let changes = limit_changes(&d.spec.config, &i.limits);
                let outdated = i
                    .built_from
                    .as_deref()
                    .map(|b| outdated_parts(b, &fingerprint))
                    .unwrap_or_default();
                let what = outdated.join(" and ");
                items.push(if !outdated.is_empty() && d.availability == Availability::Stateless {
                    PlanItem {
                        action: PlanAction::Replace,
                        reason: format!(
                            "{role}runs another {what} ({}): a new replica with the design's replaces it, healthy before this one is removed",
                            i.status
                        ),
                        outdated,
                        ..base
                    }
                } else {
                    let kept = if outdated.is_empty() {
                        String::new()
                    } else {
                        format!("; keeps its previous {what} (updating singleton and stateful modules is not built yet)")
                    };
                    if changes.is_empty() {
                        PlanItem {
                            reason: format!("{role}already deployed ({}){kept}", i.status),
                            outdated,
                            ..base
                        }
                    } else {
                        PlanItem {
                            action: PlanAction::Resize,
                            reason: format!("{role}resources change: {} ({}){kept}", describe(&changes), i.status),
                            changes,
                            outdated,
                            ..base
                        }
                    }
                });
            }
            if !slot.is_empty() {
                continue;
            }
            items.push(match in_project.iter().find(|i| i.name == name) {
                None => item(PlanAction::Create, "not deployed yet".into()),
                Some(i) if !i.managed => item(
                    PlanAction::Conflict,
                    "an instance with this name exists and was not created by Apelora".into(),
                ),
                Some(i) => item(
                    PlanAction::Conflict,
                    match &i.owner {
                        Some(o) => format!("an instance with this name belongs to {}/{} replica {}", o.application, o.module, o.replica),
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
            module: owner.map(|o| o.module.clone()).unwrap_or_default(),
            replica: owner.map_or(1, |o| o.replica),
            instance: i.name.clone(),
            action: PlanAction::Remove,
            reason: "no longer in the design".into(),
            changes: Vec::new(),
            outdated: Vec::new(),
        });
    }

    let mut single: Vec<&str> = desired
        .iter()
        .filter(|d| d.replicas <= 1)
        .map(|d| d.module.as_str())
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
    let outdated: Vec<&str> = items
        .iter()
        .filter(|i| !i.outdated.is_empty() && i.action != PlanAction::Replace)
        .map(|i| i.instance.as_str())
        .collect();
    if !outdated.is_empty() {
        warnings.push(format!(
            "{} keep{} the image or settings {} was created with: deploying does not update singleton or stateful modules yet; remove the module and deploy it again to apply the design",
            outdated.join(", "),
            if outdated.len() == 1 { "s" } else { "" },
            if outdated.len() == 1 { "it" } else { "they were each" },
        ));
    }
    Plan {
        items,
        warnings,
        quota: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ImageProtocol, ImageSource, InstanceKind};

    fn desired(module: &str) -> DesiredModule {
        DesiredModule {
            module: module.into(),
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
                volumes: Vec::new(),
                root_pool: None,
            },
            replicas: 1,
            availability: Availability::Singleton,
            internet: Default::default(),
            guest_os: Default::default(),
            readiness: None,
            dns_resolver: false,
        }
    }

    #[test]
    fn a_running_instance_built_from_another_image_or_settings_is_named_and_kept() {
        use crate::storage::{VolumeClass, VolumeContent, VolumeMount};
        let mut web = stateless("web-01", 1);
        web.spec
            .config
            .insert("environment.MODE".into(), "blue".into());
        web.spec
            .config
            .insert("limits.memory".into(), "512MiB".into());
        let built = |d: &DesiredModule| {
            let mut i = observed("shop-web-01", "apelora", true, Some(("shop", "web-01", 1)));
            i.limits = [("limits.memory".to_owned(), "512MiB".to_owned())].into();
            // What creation records is the spec deploy_spec builds: its name, owner and network
            // do not change the fingerprint.
            i.built_from = Some(spec_fingerprint(&deploy_spec(
                "shop",
                "apelora",
                d,
                1,
                Some("incusbr0"),
            )));
            i
        };
        let first = built(&web);
        let item = |d: &DesiredModule, i: &Instance| {
            plan(
                "shop",
                "apelora",
                std::slice::from_ref(d),
                std::slice::from_ref(i),
            )
        };

        // Same design: kept, nothing outdated, no warning beyond the single instance.
        let p = item(&web, &first);
        assert_eq!(
            (p.items[0].action, p.items[0].outdated.clone()),
            (PlanAction::Keep, vec![])
        );
        assert_eq!(p.warnings.len(), 1, "{:?}", p.warnings);

        // Created before Apelora recorded it: nothing to compare, kept as before.
        let mut old = first.clone();
        old.built_from = None;
        assert!(item(&web, &old).items[0].outdated.is_empty());

        // Limits are resized in place and Apelora's discovery variables are re-synced: neither
        // makes an instance outdated.
        let mut resized = web.clone();
        resized
            .spec
            .config
            .insert("limits.memory".into(), "1GiB".into());
        resized.spec.config.insert(
            "environment.APELORA_SERVICE_DB_01".into(),
            "198.18.100.2".into(),
        );
        let p = item(&resized, &first);
        assert_eq!(
            (p.items[0].action, p.items[0].outdated.len()),
            (PlanAction::Resize, 0)
        );

        // Another image and another setting on a stateless replica: replaced, with what
        // differs named, and no warning about keeping it.
        let mut changed = web.clone();
        changed.spec.source.alias = "debian/14".into();
        changed
            .spec
            .config
            .insert("environment.MODE".into(), "green".into());
        let p = item(&changed, &first);
        assert_eq!(p.items[0].action, PlanAction::Replace);
        assert_eq!(p.items[0].outdated, ["image", "settings"]);
        assert!(
            p.items[0]
                .reason
                .starts_with("runs another image and settings (Running): a new replica"),
            "{}",
            p.items[0].reason
        );
        assert!(
            !p.warnings.iter().any(|w| w.contains("keeps the image")),
            "{:?}",
            p.warnings
        );

        // The same change on a singleton: kept with the reason, and warned about.
        let single = DesiredModule {
            availability: Availability::Singleton,
            ..changed.clone()
        };
        let p = item(&single, &first);
        assert_eq!(p.items[0].action, PlanAction::Keep);
        assert_eq!(p.items[0].outdated, ["image", "settings"]);
        assert!(
            p.items[0].reason.contains(
                "keeps its previous image and settings (updating singleton and stateful modules is not built yet)"
            ),
            "{}",
            p.items[0].reason
        );
        assert!(
            p.warnings
                .iter()
                .any(|w| w
                    .starts_with("shop-web-01 keeps the image or settings it was created with")),
            "{:?}",
            p.warnings
        );

        // A volume's path is storage; its size grows in place and does not count.
        let volume = |path: &str, size_mib: u32| VolumeMount {
            name: "data".into(),
            size_mib,
            content: VolumeContent::Filesystem,
            class: VolumeClass::DataLocal,
            path: Some(path.into()),
            pool: None,
        };
        let mut stateful = desired("db-01");
        stateful.spec.volumes = vec![volume("/srv/data", 1024)];
        let mut db = observed("shop-db-01", "apelora", true, Some(("shop", "db-01", 1)));
        db.built_from = Some(spec_fingerprint(&stateful.spec));
        let mut grown = stateful.clone();
        grown.spec.volumes = vec![volume("/srv/data", 4096)];
        assert!(
            item(&grown, &db).items[0].outdated.is_empty(),
            "size is grown in place"
        );
        let mut moved = stateful.clone();
        moved.spec.volumes = vec![volume("/var/lib/db", 1024)];
        assert_eq!(item(&moved, &db).items[0].outdated, ["storage"]);
    }

    #[test]
    fn outdated_parts_compare_only_what_both_recorded() {
        assert_eq!(
            outdated_parts(
                "image=a,settings=b,storage=c",
                "image=a,settings=x,storage=c"
            ),
            ["settings"]
        );
        assert!(
            outdated_parts("image=a", "image=a,settings=x").is_empty(),
            "an older label without settings"
        );
        assert!(outdated_parts("garbage", "image=a").is_empty());
    }

    fn stateless(module: &str, replicas: u8) -> DesiredModule {
        DesiredModule {
            replicas,
            availability: Availability::Stateless,
            ..desired(module)
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
                module: b.into(),
                replica: r,
            }),
            evacuate: None,
            unreachable: false,
            built_from: None,
            evacuated_from: None,
            limits: Default::default(),
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
        assert_eq!(p.items[4].module, "old-01");
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
                module: "web-01".into(),
                replica: 3,
                instance: "shop-web-01-r3".into(),
                action: Remove,
                reason: "no longer in the design".into(),
                changes: vec![],
                outdated: vec![],
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
    fn only_stateless_modules_get_more_than_one_replica() {
        assert_eq!(stateless("web-01", 3).validate(), Ok(()));
        assert!(
            DesiredModule {
                replicas: 2,
                ..desired("db-01")
            }
            .validate()
            .is_err()
        );
        assert!(
            DesiredModule {
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
                module: "web-01".into(),
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

    #[test]
    fn an_update_takes_its_replicas_slot_and_is_itself_replaced_when_outdated() {
        let web = stateless("web-01", 2);
        let fingerprint = spec_fingerprint(&web.spec);
        let built = |name: &str, replica: u8, from: &str| {
            let mut i = observed_instance(name, Some(("shop", "web-01", replica)));
            i.built_from = Some(from.to_owned());
            i
        };
        // Mid-update: replica 1 has its update beside it, replica 2 is still the old one.
        let observed = [
            built("shop-web-01", 1, "image=old,settings=old,storage=old"),
            built("shop-web-01-u1", 1, &fingerprint),
            built("shop-web-01-r2", 2, "image=old,settings=old,storage=old"),
        ];
        let p = plan("shop", "apelora", std::slice::from_ref(&web), &observed);
        assert_eq!(
            actions(&p),
            vec![
                ("shop-web-01", PlanAction::Replace),
                ("shop-web-01-u1", PlanAction::Keep),
                ("shop-web-01-r2", PlanAction::Replace),
            ]
        );
        assert_eq!(update_name("shop", "web-01", 2, 1), "shop-web-01-r2-u1");
        // Done: the updates fill the slots, nothing is created again or removed.
        let observed = [
            built("shop-web-01-u1", 1, &fingerprint),
            built("shop-web-01-r2-u1", 2, &fingerprint),
        ];
        let p = plan("shop", "apelora", std::slice::from_ref(&web), &observed);
        assert_eq!(
            actions(&p),
            vec![
                ("shop-web-01-u1", PlanAction::Keep),
                ("shop-web-01-r2-u1", PlanAction::Keep),
            ]
        );
        // A name that only looks like an update of another slot is not one.
        assert!(!in_slot("shop-web-01-u", "shop-web-01"));
        assert!(!in_slot("shop-web-01-ux", "shop-web-01"));
    }

    fn with_limits(mut d: DesiredModule, limits: &[(&str, &str)]) -> DesiredModule {
        d.spec.config = limits
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        d
    }

    fn running_with(limits: &[(&str, &str)]) -> Instance {
        let mut i = observed("shop-web-01", "apelora", true, Some(("shop", "web-01", 1)));
        i.limits = limits
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        i
    }

    #[test]
    fn an_instance_with_other_limits_than_the_design_is_resized_in_place() {
        let want = [with_limits(
            desired("web-01"),
            &[("limits.cpu", "2"), ("limits.memory", "1024MiB")],
        )];
        let p = plan(
            "shop",
            "apelora",
            &want,
            &[running_with(&[
                ("limits.cpu", "1"),
                ("limits.memory", "512MiB"),
            ])],
        );
        assert_eq!(actions(&p), [("shop-web-01", PlanAction::Resize)]);
        let item = &p.items[0];
        assert_eq!(
            item.changes,
            vec![
                LimitChange {
                    key: "limits.cpu".into(),
                    from: Some("1".into()),
                    to: Some("2".into())
                },
                LimitChange {
                    key: "limits.memory".into(),
                    from: Some("512MiB".into()),
                    to: Some("1024MiB".into())
                },
            ]
        );
        assert_eq!(
            item.reason,
            "resources change: CPUs 1 → 2, memory 512MiB → 1024MiB (Running)"
        );
        assert!(!p.has_conflicts());
    }

    #[test]
    fn the_same_limits_written_differently_are_not_a_change() {
        let want = [with_limits(
            desired("web-01"),
            &[("limits.cpu", "1"), ("limits.memory", "1GiB")],
        )];
        let same = running_with(&[("limits.cpu", "1"), ("limits.memory", "1024MiB")]);
        assert_eq!(
            actions(&plan("shop", "apelora", &want, &[same])),
            [("shop-web-01", PlanAction::Keep)]
        );
        // Binary and decimal units are different sizes.
        let other = running_with(&[("limits.cpu", "1"), ("limits.memory", "1GB")]);
        assert_eq!(
            actions(&plan("shop", "apelora", &want, &[other])),
            [("shop-web-01", PlanAction::Resize)]
        );
        // Neither side has limits.
        let none = running_with(&[]);
        assert_eq!(
            actions(&plan("shop", "apelora", &[desired("web-01")], &[none])),
            [("shop-web-01", PlanAction::Keep)]
        );
    }

    #[test]
    fn a_limit_the_design_no_longer_sets_is_removed_and_a_new_one_is_added() {
        let want = [with_limits(desired("web-01"), &[("limits.cpu", "1")])];
        let have = running_with(&[("limits.cpu", "1"), ("limits.cpu.allowance", "50ms/100ms")]);
        let p = plan("shop", "apelora", &want, &[have]);
        assert_eq!(
            p.items[0].changes,
            vec![LimitChange {
                key: "limits.cpu.allowance".into(),
                from: Some("50ms/100ms".into()),
                to: None
            }]
        );
        assert!(
            p.items[0]
                .reason
                .contains("CPU quota 50ms/100ms → no limit"),
            "{}",
            p.items[0].reason
        );
        let added = plan(
            "shop",
            "apelora",
            &[with_limits(
                desired("web-01"),
                &[("limits.memory", "256MiB")],
            )],
            &[running_with(&[])],
        );
        assert_eq!(
            added.items[0].changes,
            vec![LimitChange {
                key: "limits.memory".into(),
                from: None,
                to: Some("256MiB".into())
            }]
        );
        // Other config keys (labels, network) are never compared or changed.
        let labels = with_limits(
            desired("web-01"),
            &[("user.apelora.x", "1"), ("limits.cpu", "1")],
        );
        let p = plan(
            "shop",
            "apelora",
            &[labels],
            &[running_with(&[("limits.cpu", "1")])],
        );
        assert_eq!(actions(&p), [("shop-web-01", PlanAction::Keep)]);
    }

    #[test]
    fn only_the_applications_own_reachable_instance_is_resized() {
        let want = [with_limits(desired("web-01"), &[("limits.cpu", "2")])];
        let mut stranded = running_with(&[("limits.cpu", "1")]);
        stranded.unreachable = true;
        assert_eq!(
            actions(&plan("shop", "apelora", &want, &[stranded])),
            [("shop-web-01", PlanAction::Keep)],
            "its member cannot be reached"
        );
        let mut stopped = running_with(&[("limits.cpu", "1")]);
        stopped.status = "Stopped".into();
        let p = plan("shop", "apelora", &want, &[stopped]);
        assert_eq!(
            p.items[0].action,
            PlanAction::Resize,
            "a stopped instance is resized too"
        );
        assert!(p.items[0].reason.ends_with("(Stopped)"));
        // Someone else's instance with the name stays a conflict, and a substitute is only kept.
        let foreign = observed("shop-web-01", "apelora", false, None);
        assert_eq!(
            plan("shop", "apelora", &want, &[foreign]).items[0].action,
            PlanAction::Conflict
        );
        let mut sub = observed(
            "shop-web-01-s1",
            "apelora",
            true,
            Some(("shop", "web-01", 1)),
        );
        sub.limits.insert("limits.cpu".into(), "1".into());
        let p = plan("shop", "apelora", &want, &[sub]);
        assert!(
            p.items.iter().all(|i| i.action != PlanAction::Resize),
            "{:?}",
            actions(&p)
        );
    }

    #[test]
    fn memory_sizes_compare_by_bytes() {
        assert_eq!(memory_bytes("1GiB"), Some(1 << 30));
        assert_eq!(memory_bytes("1024MiB"), Some(1 << 30));
        assert_eq!(memory_bytes("1GB"), Some(1_000_000_000));
        assert_eq!(memory_bytes("2048"), Some(2048));
        assert_eq!(memory_bytes("10%"), None);
        assert_eq!(memory_bytes("lots"), None);
        assert_eq!(memory_bytes(""), None);
        assert!(same_limit("limits.memory", "1GiB", "1024MiB"));
        assert!(!same_limit("limits.memory", "1GiB", "1GB"));
        assert!(
            !same_limit("limits.cpu", "1", "01"),
            "CPU counts are compared as written"
        );
    }

    fn observed_instance(name: &str, owner: Option<(&str, &str, u8)>) -> Instance {
        observed(name, "apelora", true, owner)
    }
}

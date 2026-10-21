// SPDX-License-Identifier: AGPL-3.0-only
//! Tenancy on Incus projects and quotas (design: `docs/design/tenancy-isolation.md`, ADR 0012).
//! Pure: the control plane reads Incus and PostgreSQL; these functions decide what an
//! environment's Incus project should look like, what limits it gets, and whether a deployment
//! fits the quotas above it.
//!
//! Every Incus key used here is documented for the pinned release (`doc/config_options.txt`,
//! groups `project-features`, `project-restricted`, `project-limits`, `project-specific`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const GIB: i64 = 1024 * 1024 * 1024;
pub const MIB: i64 = 1024 * 1024;

/// Root disk size the default profile of a project Apelora creates gives every instance.
/// Incus counts root disks against `limits.disk` only when they have a size.
pub const DEFAULT_ROOT_DISK_GIB: i64 = 10;
/// `limits.cpu` for instances that set none (the default profile's value).
pub const DEFAULT_INSTANCE_CPU: i64 = 1;
/// `limits.memory` for instances that set none (the default profile's value).
pub const DEFAULT_INSTANCE_MEMORY_GIB: i64 = 1;

/// Largest value accepted per dimension (a typo guard, not a capacity rule).
pub const MAX_INSTANCES: i64 = 100_000;
pub const MAX_CPU: i64 = 100_000;
pub const MAX_MEMORY_GIB: i64 = 10_000_000;
pub const MAX_DISK_GIB: i64 = 100_000_000;

/// Ownership marker on projects Apelora created.
pub const MANAGED_KEY: &str = "user.apelora.managed";
pub const ENVIRONMENT_KEY: &str = "user.apelora.environment";

/// The Incus project keys Apelora owns on an environment's project.
pub const LIMIT_KEYS: [&str; 4] = [
    "limits.instances",
    "limits.cpu",
    "limits.memory",
    "limits.disk",
];

/// One quota dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum QuotaDimension {
    /// Number of instances.
    Instances,
    /// CPUs: the sum of instances' `limits.cpu`.
    Cpu,
    /// Bytes: the sum of instances' `limits.memory`.
    Memory,
    /// Bytes: root disk sizes plus volume sizes.
    Disk,
}

impl QuotaDimension {
    pub const ALL: [QuotaDimension; 4] = [
        QuotaDimension::Instances,
        QuotaDimension::Cpu,
        QuotaDimension::Memory,
        QuotaDimension::Disk,
    ];

    pub fn label(self) -> &'static str {
        match self {
            QuotaDimension::Instances => "instances",
            QuotaDimension::Cpu => "CPUs",
            QuotaDimension::Memory => "memory",
            QuotaDimension::Disk => "disk",
        }
    }

    /// The Incus project key that limits it.
    pub fn incus_key(self) -> &'static str {
        match self {
            QuotaDimension::Instances => LIMIT_KEYS[0],
            QuotaDimension::Cpu => LIMIT_KEYS[1],
            QuotaDimension::Memory => LIMIT_KEYS[2],
            QuotaDimension::Disk => LIMIT_KEYS[3],
        }
    }

    /// A value in this dimension's units, for messages (`3 instances`, `12 GiB`).
    pub fn show(self, v: i64) -> String {
        match self {
            QuotaDimension::Instances => format!("{v}"),
            QuotaDimension::Cpu => format!("{v}"),
            QuotaDimension::Memory | QuotaDimension::Disk => show_bytes(v),
        }
    }
}

/// Bytes as GiB with at most one decimal (`10 GiB`, `1.5 GiB`, `512 MiB` below one GiB).
pub fn show_bytes(v: i64) -> String {
    if v.abs() < GIB {
        return format!("{} MiB", v / MIB);
    }
    let tenths = (v * 10 + GIB / 2) / GIB;
    if tenths % 10 == 0 {
        format!("{} GiB", tenths / 10)
    } else {
        format!("{}.{} GiB", tenths / 10, tenths % 10)
    }
}

/// A quota: one limit per dimension, `None` meaning no limit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct QuotaLimits {
    #[serde(default)]
    pub instances: Option<i64>,
    #[serde(default)]
    pub cpu: Option<i64>,
    #[serde(default)]
    pub memory_gib: Option<i64>,
    #[serde(default)]
    pub disk_gib: Option<i64>,
}

impl QuotaLimits {
    pub fn is_empty(&self) -> bool {
        *self == QuotaLimits::default()
    }

    /// Checks every set value is within its range.
    pub fn validate(&self) -> Result<(), String> {
        for (name, v, max) in [
            ("instances", self.instances, MAX_INSTANCES),
            ("cpu", self.cpu, MAX_CPU),
            ("memoryGib", self.memory_gib, MAX_MEMORY_GIB),
            ("diskGib", self.disk_gib, MAX_DISK_GIB),
        ] {
            if let Some(v) = v
                && !(0..=max).contains(&v)
            {
                return Err(format!("{name} must be 0–{max}, or empty for no limit"));
            }
        }
        Ok(())
    }

    /// The limit in the dimension's own units (instances, CPUs, bytes).
    pub fn limit(&self, d: QuotaDimension) -> Option<i64> {
        match d {
            QuotaDimension::Instances => self.instances,
            QuotaDimension::Cpu => self.cpu,
            QuotaDimension::Memory => self.memory_gib.map(|g| g.saturating_mul(GIB)),
            QuotaDimension::Disk => self.disk_gib.map(|g| g.saturating_mul(GIB)),
        }
    }

    /// Per dimension, the smallest of the quotas that set one.
    pub fn tightest<'a>(all: impl IntoIterator<Item = &'a QuotaLimits>) -> QuotaLimits {
        let min = |a: Option<i64>, b: Option<i64>| match (a, b) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        all.into_iter()
            .fold(QuotaLimits::default(), |acc, q| QuotaLimits {
                instances: min(acc.instances, q.instances),
                cpu: min(acc.cpu, q.cpu),
                memory_gib: min(acc.memory_gib, q.memory_gib),
                disk_gib: min(acc.disk_gib, q.disk_gib),
            })
    }

    /// The dimensions in which `self` asks for more than `parent` allows (a child quota may
    /// not exceed its parent's, and "no limit" exceeds every limit).
    pub fn above(&self, parent: &QuotaLimits) -> Vec<QuotaDimension> {
        QuotaDimension::ALL
            .into_iter()
            .filter(|&d| match (self.limit(d), parent.limit(d)) {
                (_, None) => false,
                (None, Some(_)) => true,
                (Some(c), Some(p)) => c > p,
            })
            .collect()
    }
}

/// Resources in use or asked for, in Incus's units.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct QuotaUsage {
    pub instances: i64,
    pub cpu: i64,
    pub memory_bytes: i64,
    pub disk_bytes: i64,
}

impl QuotaUsage {
    pub fn get(&self, d: QuotaDimension) -> i64 {
        match d {
            QuotaDimension::Instances => self.instances,
            QuotaDimension::Cpu => self.cpu,
            QuotaDimension::Memory => self.memory_bytes,
            QuotaDimension::Disk => self.disk_bytes,
        }
    }

    pub fn plus(self, o: QuotaUsage) -> QuotaUsage {
        QuotaUsage {
            instances: self.instances.saturating_add(o.instances),
            cpu: self.cpu.saturating_add(o.cpu),
            memory_bytes: self.memory_bytes.saturating_add(o.memory_bytes),
            disk_bytes: self.disk_bytes.saturating_add(o.disk_bytes),
        }
    }

    pub fn sum(all: impl IntoIterator<Item = QuotaUsage>) -> QuotaUsage {
        all.into_iter()
            .fold(QuotaUsage::default(), QuotaUsage::plus)
    }
}

/// `limits.cpu` as Incus accepts it for project accounting: a count of CPUs. A set or range
/// (`0-3`, `1,3`) counts its CPUs.
pub fn parse_cpu(v: &str) -> Option<i64> {
    let v = v.trim();
    if let Ok(n) = v.parse::<i64>() {
        return (n >= 0).then_some(n);
    }
    let mut count = 0i64;
    for part in v.split(',') {
        let part = part.trim();
        match part.split_once('-') {
            Some((a, b)) => {
                let (a, b) = (a.trim().parse::<i64>().ok()?, b.trim().parse::<i64>().ok()?);
                if b < a {
                    return None;
                }
                count += b - a + 1;
            }
            None => {
                part.parse::<i64>().ok()?;
                count += 1;
            }
        }
    }
    (count > 0).then_some(count)
}

/// A size as Incus writes them (`512MiB`, `1GiB`, `2GB`, `1024`): bytes. Percentages and
/// anything else give `None`.
pub fn parse_bytes(v: &str) -> Option<i64> {
    let v = v.trim();
    let split = v
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(v.len());
    let (num, unit) = v.split_at(split);
    let n: f64 = num.parse().ok()?;
    let mult: f64 = match unit.trim() {
        "" | "B" => 1.0,
        "kB" | "KB" => 1e3,
        "MB" => 1e6,
        "GB" => 1e9,
        "TB" => 1e12,
        "PB" => 1e15,
        "KiB" => 1024.0,
        "MiB" => 1024.0 * 1024.0,
        "GiB" => 1024.0 * 1024.0 * 1024.0,
        "TiB" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        "PiB" => 1024.0 * 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    let bytes = n * mult;
    (bytes.is_finite() && (0.0..9.0e18).contains(&bytes)).then_some(bytes.round() as i64)
}

/// What creating one instance adds: its CPU and memory limits (the profile's defaults when it
/// sets none or Incus could not count them), its root disk (`root_disk_bytes`, zero where the
/// project's profile gives root disks no size) and its volumes.
pub fn instance_usage(
    config: &BTreeMap<String, String>,
    volume_mib: i64,
    root_disk_bytes: i64,
) -> QuotaUsage {
    QuotaUsage {
        instances: 1,
        cpu: config
            .get("limits.cpu")
            .and_then(|v| parse_cpu(v))
            .unwrap_or(DEFAULT_INSTANCE_CPU),
        memory_bytes: config
            .get("limits.memory")
            .and_then(|v| parse_bytes(v))
            .unwrap_or(DEFAULT_INSTANCE_MEMORY_GIB * GIB),
        disk_bytes: root_disk_bytes.saturating_add(volume_mib.saturating_mul(MIB)),
    }
}

/// Which level of the tenancy tree a quota belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum QuotaScopeKind {
    Organization,
    Project,
    Environment,
}

impl QuotaScopeKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "organization" => Some(Self::Organization),
            "project" => Some(Self::Project),
            "environment" => Some(Self::Environment),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Organization => "organization",
            Self::Project => "project",
            Self::Environment => "environment",
        }
    }
}

/// One scope a deployment is checked against: its quota and what is in use beneath it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaScopeInput {
    pub kind: QuotaScopeKind,
    pub id: String,
    pub name: String,
    pub limits: QuotaLimits,
    pub used: QuotaUsage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct QuotaDimensionCheck {
    pub dimension: QuotaDimension,
    /// In use now (instances, CPUs or bytes).
    pub used: i64,
    /// What the deployment adds.
    pub adding: i64,
    pub limit: i64,
    pub exceeded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct QuotaScopeCheck {
    pub kind: QuotaScopeKind,
    pub id: String,
    pub name: String,
    /// Only the dimensions this scope limits.
    pub dimensions: Vec<QuotaDimensionCheck>,
}

/// A deployment against the quotas above its environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct QuotaCheck {
    /// False when any limited dimension would be exceeded: the deployment is refused.
    pub allowed: bool,
    /// What it adds (instances to create; removals are not credited before they happen).
    pub adding: QuotaUsage,
    pub scopes: Vec<QuotaScopeCheck>,
}

impl QuotaCheck {
    /// The refusal in words, naming each exceeded scope and dimension.
    pub fn refusal(&self) -> Option<String> {
        let parts: Vec<String> = self
            .scopes
            .iter()
            .flat_map(|s| {
                s.dimensions.iter().filter(|d| d.exceeded).map(move |d| {
                    format!(
                        "{} {}: {} {} in use, this adds {}, quota {}",
                        s.kind.as_str(),
                        s.name,
                        d.dimension.show(d.used),
                        d.dimension.label(),
                        d.dimension.show(d.adding),
                        d.dimension.show(d.limit),
                    )
                })
            })
            .collect();
        (!parts.is_empty()).then(|| parts.join("; "))
    }
}

/// Checks `adding` against every scope's quota. Only dimensions a scope limits are listed;
/// a deployment that adds nothing in a dimension never fails it (a quota lowered below use
/// does not block removals or unchanged redeploys).
pub fn check(scopes: &[QuotaScopeInput], adding: QuotaUsage) -> QuotaCheck {
    let scopes: Vec<QuotaScopeCheck> = scopes
        .iter()
        .filter(|s| !s.limits.is_empty())
        .map(|s| QuotaScopeCheck {
            kind: s.kind,
            id: s.id.clone(),
            name: s.name.clone(),
            dimensions: QuotaDimension::ALL
                .into_iter()
                .filter_map(|d| {
                    let limit = s.limits.limit(d)?;
                    let (used, add) = (s.used.get(d), adding.get(d));
                    Some(QuotaDimensionCheck {
                        dimension: d,
                        used,
                        adding: add,
                        limit,
                        exceeded: add > 0 && used.saturating_add(add) > limit,
                    })
                })
                .collect(),
        })
        .collect();
    QuotaCheck {
        allowed: !scopes
            .iter()
            .any(|s| s.dimensions.iter().any(|d| d.exceeded)),
        adding,
        scopes,
    }
}

// ---- the environment's Incus project ------------------------------------------------------------

/// What Apelora knows about an environment's Incus project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum TenancyIncusState {
    /// Not prepared by Apelora yet; the environment's detail says why.
    Pending,
    /// Created by Apelora with the baseline restrictions.
    Created,
    /// Existed already, restricted; Apelora granted it and keeps its restrictions.
    Adopted,
}

impl TenancyIncusState {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "created" => Some(Self::Created),
            "adopted" => Some(Self::Adopted),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Created => "created",
            Self::Adopted => "adopted",
        }
    }
}

/// Platform settings that shape a new project.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProvisionSettings {
    /// `APELORA_NETWORK_UPLINK`: applications get their own OVN network in the project.
    pub uplink: Option<String>,
    /// `APELORA_SERVICE_ADDRESSES`: the range load balancers take service addresses from.
    pub service_range: Option<String>,
}

/// The restrictions and features of a project Apelora creates (design §2.1). The effective
/// values of the lab's deploy project, where every deployment test ran, written out in full.
pub fn baseline_config(environment_id: &str, s: &ProvisionSettings) -> BTreeMap<String, String> {
    let mut c: BTreeMap<String, String> = [
        ("restricted", "true"),
        ("restricted.containers.nesting", "block"),
        ("restricted.containers.privilege", "unprivileged"),
        ("restricted.containers.lowlevel", "block"),
        ("restricted.containers.interception", "block"),
        ("restricted.virtual-machines.lowlevel", "block"),
        ("restricted.devices.disk", "managed"),
        ("restricted.devices.nic", "managed"),
        ("restricted.devices.gpu", "block"),
        ("restricted.devices.pci", "block"),
        ("restricted.devices.usb", "block"),
        ("restricted.devices.unix-char", "block"),
        ("restricted.devices.unix-block", "block"),
        ("restricted.devices.unix-hotplug", "block"),
        ("restricted.devices.infiniband", "block"),
        ("restricted.devices.proxy", "block"),
        // The worker places replicas and moves instances to named members.
        ("restricted.cluster.target", "allow"),
        // Volume snapshots and their schedules.
        ("restricted.snapshots", "allow"),
        // Backups are volume copies, not Incus backups.
        ("restricted.backups", "block"),
        ("restricted.networks.zones", "block"),
        // Images are shared with the default project, as in the lab's deploy project.
        ("features.images", "false"),
        ("features.profiles", "true"),
        ("features.storage.volumes", "true"),
        ("features.storage.buckets", "true"),
        (MANAGED_KEY, "true"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    c.insert(ENVIRONMENT_KEY.into(), environment_id.to_owned());
    match &s.uplink {
        Some(uplink) => {
            c.insert("features.networks".into(), "true".into());
            c.insert("restricted.networks.uplinks".into(), uplink.clone());
            if let Some(range) = &s.service_range {
                c.insert(
                    "restricted.networks.subnets".into(),
                    format!("{uplink}:{range}"),
                );
            }
        }
        None => {
            c.insert("features.networks".into(), "false".into());
        }
    }
    c
}

/// The four limit keys for an effective quota (memory and disk as `<n>GiB`).
pub fn limit_config(limits: &QuotaLimits) -> BTreeMap<String, String> {
    let mut c = BTreeMap::new();
    if let Some(n) = limits.instances {
        c.insert(LIMIT_KEYS[0].to_owned(), n.to_string());
    }
    if let Some(n) = limits.cpu {
        c.insert(LIMIT_KEYS[1].to_owned(), n.to_string());
    }
    if let Some(n) = limits.memory_gib {
        c.insert(LIMIT_KEYS[2].to_owned(), format!("{n}GiB"));
    }
    if let Some(n) = limits.disk_gib {
        c.insert(LIMIT_KEYS[3].to_owned(), format!("{n}GiB"));
    }
    c
}

/// The project configuration after applying `desired` limits: Apelora's limit keys replaced,
/// keys Apelora wrote before (`written`) and no longer wants removed, everything else kept.
/// A limit someone else set on an adopted project is left alone unless Apelora sets that key.
pub fn with_limits(
    current: &BTreeMap<String, String>,
    desired: &BTreeMap<String, String>,
    written: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut out = current.clone();
    for k in LIMIT_KEYS {
        match desired.get(k) {
            Some(v) => {
                out.insert(k.to_owned(), v.clone());
            }
            None if written.contains_key(k) => {
                out.remove(k);
            }
            None => {}
        }
    }
    out
}

/// The default profile of a project Apelora creates.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileSpec {
    pub config: BTreeMap<String, String>,
    pub devices: BTreeMap<String, BTreeMap<String, String>>,
}

/// The new project's `default` profile: the root disk of the `default` project's default
/// profile (`root`, same pool) with a size, CPU and memory defaults, and, when the project
/// uses the default project's networks (no uplink), that profile's managed `eth0`. Without a
/// root disk to copy, `fallback_pool` gives the pool.
pub fn default_profile(
    root: Option<&BTreeMap<String, String>>,
    eth0: Option<&BTreeMap<String, String>>,
    fallback_pool: Option<&str>,
    uplink: bool,
) -> Result<ProfileSpec, String> {
    let pool = root
        .and_then(|r| r.get("pool").cloned())
        .or_else(|| fallback_pool.map(str::to_owned))
        .ok_or("no storage pool for root disks: the default project's default profile has no root disk and APELORA_VOLUME_POOL is not set")?;
    let mut devices = BTreeMap::new();
    devices.insert(
        "root".to_owned(),
        [
            ("type", "disk".to_owned()),
            ("path", "/".to_owned()),
            ("pool", pool),
            ("size", format!("{DEFAULT_ROOT_DISK_GIB}GiB")),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect(),
    );
    if !uplink
        && let Some(nic) =
            eth0.filter(|d| d.get("type").is_some_and(|t| t == "nic") && d.get("network").is_some())
    {
        devices.insert("eth0".to_owned(), nic.clone());
    }
    Ok(ProfileSpec {
        config: [
            ("limits.cpu", DEFAULT_INSTANCE_CPU.to_string()),
            ("limits.memory", format!("{DEFAULT_INSTANCE_MEMORY_GIB}GiB")),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect(),
        devices,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum TenancyFindingSeverity {
    Info,
    Warning,
    Error,
}

/// Something about an environment's Incus project worth knowing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct TenancyFinding {
    pub severity: TenancyFindingSeverity,
    /// The configuration key it is about, if any.
    pub key: Option<String>,
    pub message: String,
}

/// Restriction values where `allow` (or a looser setting) differs from the baseline: what an
/// adopted project permits that a created one would not. An unrestricted project is an error.
pub fn findings(config: &BTreeMap<String, String>, s: &ProvisionSettings) -> Vec<TenancyFinding> {
    let mut out = Vec::new();
    if config.get("restricted").map(String::as_str) != Some("true") {
        out.push(TenancyFinding {
            severity: TenancyFindingSeverity::Error,
            key: Some("restricted".into()),
            message: "the project is not restricted: instances may use privileged containers, host devices and nesting".into(),
        });
        return out;
    }
    let baseline = baseline_config("", s);
    // Incus's documented default for each restriction when the key is absent.
    let default_of = |k: &str| -> &'static str {
        match k {
            "restricted.containers.privilege" => "unprivileged",
            "restricted.devices.disk" | "restricted.devices.nic" => "managed",
            _ => "block",
        }
    };
    for (k, want) in baseline
        .iter()
        .filter(|(k, _)| k.starts_with("restricted."))
    {
        if matches!(
            k.as_str(),
            "restricted.networks.uplinks" | "restricted.networks.subnets"
        ) {
            continue;
        }
        let have = config.get(k).map_or_else(|| default_of(k), String::as_str);
        if have == want {
            continue;
        }
        // How much a value permits: block/isolated < managed/unprivileged < allow.
        let rank = |v: &str| match v {
            "block" | "isolated" => 0,
            "allow" => 2,
            _ => 1,
        };
        let (severity, message) = if rank(have) > rank(want) {
            (
                TenancyFindingSeverity::Warning,
                format!("{k} is {have}: looser than the {want} of projects Apelora creates"),
            )
        } else if want == "allow" {
            (
                TenancyFindingSeverity::Warning,
                format!(
                    "{k} is {have}: Apelora needs {want} ({})",
                    if k == "restricted.snapshots" {
                        "volume snapshots and their schedules"
                    } else {
                        "placing replicas and moving instances between servers"
                    }
                ),
            )
        } else {
            (
                TenancyFindingSeverity::Info,
                format!("{k} is {have}: stricter than the {want} of projects Apelora creates"),
            )
        };
        out.push(TenancyFinding {
            severity,
            key: Some(k.clone()),
            message,
        });
    }
    if s.uplink.is_some() && config.get("features.networks").map(String::as_str) != Some("true") {
        out.push(TenancyFinding {
            severity: TenancyFindingSeverity::Warning,
            key: Some("features.networks".into()),
            message: "the project uses the default project's networks: application networks (APELORA_NETWORK_UPLINK) cannot be created in it".into(),
        });
    }
    out
}

/// The `restricted*` and `features.*` keys of a project, for display.
pub fn restrictions(config: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    config
        .iter()
        .filter(|(k, _)| k.starts_with("restricted") || k.starts_with("features."))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// One resource of an Incus project as `GET /1.0/projects/<name>/state` reports it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct TenancyResource {
    /// Instances, CPUs or bytes.
    pub used: i64,
    /// `None` when the project has no limit for it.
    pub limit: Option<i64>,
}

/// An Incus project's use and limits (Incus's own aggregate of instance limits, root disk
/// sizes and volume sizes).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct TenancyProjectUse {
    pub instances: TenancyResource,
    pub cpu: TenancyResource,
    pub memory: TenancyResource,
    pub disk: TenancyResource,
}

impl TenancyProjectUse {
    pub fn usage(&self) -> QuotaUsage {
        QuotaUsage {
            instances: self.instances.used,
            cpu: self.cpu.used,
            memory_bytes: self.memory.used,
            disk_bytes: self.disk.used,
        }
    }
}

/// An Incus project as the adapter reads it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenancyIncusProject {
    pub name: String,
    pub description: String,
    pub config: BTreeMap<String, String>,
}

/// A trusted client certificate as Incus lists it (no certificate body).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IncusTrustEntry {
    pub fingerprint: String,
    pub name: String,
    pub restricted: bool,
    pub projects: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(i: Option<i64>, c: Option<i64>, m: Option<i64>, d: Option<i64>) -> QuotaLimits {
        QuotaLimits {
            instances: i,
            cpu: c,
            memory_gib: m,
            disk_gib: d,
        }
    }

    #[test]
    fn the_tightest_quota_wins_per_dimension() {
        let org = q(Some(50), Some(64), None, Some(1000));
        let project = q(None, Some(16), Some(32), None);
        let env = q(Some(10), Some(32), None, None);
        assert_eq!(
            QuotaLimits::tightest([&org, &project, &env]),
            q(Some(10), Some(16), Some(32), Some(1000))
        );
        assert_eq!(
            QuotaLimits::tightest(std::iter::empty()),
            QuotaLimits::default()
        );
    }

    #[test]
    fn a_child_quota_may_not_exceed_its_parent() {
        let parent = q(Some(10), Some(8), None, None);
        assert!(
            q(Some(10), Some(8), Some(999), None)
                .above(&parent)
                .is_empty()
        );
        assert_eq!(
            q(Some(11), None, None, None).above(&parent),
            vec![QuotaDimension::Instances, QuotaDimension::Cpu],
            "more instances, and no CPU limit where the parent has one"
        );
    }

    #[test]
    fn quotas_are_validated() {
        assert!(q(Some(0), Some(1), Some(1), Some(1)).validate().is_ok());
        assert!(q(Some(-1), None, None, None).validate().is_err());
        assert!(q(None, Some(MAX_CPU + 1), None, None).validate().is_err());
    }

    #[test]
    fn sizes_and_cpus_parse_like_incus() {
        assert_eq!(parse_bytes("512MiB"), Some(512 * MIB));
        assert_eq!(parse_bytes("1GiB"), Some(GIB));
        assert_eq!(parse_bytes("1.5GiB"), Some(GIB + GIB / 2));
        assert_eq!(parse_bytes("2GB"), Some(2_000_000_000));
        assert_eq!(parse_bytes("1024"), Some(1024));
        assert_eq!(parse_bytes("50%"), None);
        assert_eq!(parse_bytes("lots"), None);
        assert_eq!(parse_cpu("4"), Some(4));
        assert_eq!(parse_cpu("0-3"), Some(4));
        assert_eq!(parse_cpu("1,3,5-6"), Some(4));
        assert_eq!(parse_cpu("3-1"), None);
        assert_eq!(parse_cpu("x"), None);
    }

    #[test]
    fn an_instance_counts_its_limits_or_the_profile_defaults() {
        let cfg: BTreeMap<String, String> = [("limits.cpu", "2"), ("limits.memory", "512MiB")]
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect();
        assert_eq!(
            instance_usage(&cfg, 1024, 10 * GIB),
            QuotaUsage {
                instances: 1,
                cpu: 2,
                memory_bytes: 512 * MIB,
                disk_bytes: 11 * GIB
            }
        );
        let none = instance_usage(&BTreeMap::new(), 0, 0);
        assert_eq!((none.cpu, none.memory_bytes, none.disk_bytes), (1, GIB, 0));
    }

    #[test]
    fn a_deployment_is_refused_when_any_scope_would_go_over() {
        let used = QuotaUsage {
            instances: 3,
            cpu: 14,
            memory_bytes: 20 * GIB,
            disk_bytes: 0,
        };
        let scopes = [
            QuotaScopeInput {
                kind: QuotaScopeKind::Organization,
                id: "o".into(),
                name: "Acme".into(),
                limits: q(Some(100), None, None, None),
                used,
            },
            QuotaScopeInput {
                kind: QuotaScopeKind::Project,
                id: "p".into(),
                name: "Shop".into(),
                limits: q(None, Some(16), Some(32), None),
                used,
            },
            QuotaScopeInput {
                kind: QuotaScopeKind::Environment,
                id: "e".into(),
                name: "Production".into(),
                limits: QuotaLimits::default(),
                used,
            },
        ];
        let adding = QuotaUsage {
            instances: 2,
            cpu: 4,
            memory_bytes: 2 * GIB,
            disk_bytes: 0,
        };
        let c = check(&scopes, adding);
        assert!(!c.allowed);
        assert_eq!(c.scopes.len(), 2, "a scope without a quota is not listed");
        assert_eq!(c.scopes[1].dimensions.len(), 2, "only limited dimensions");
        let cpu = &c.scopes[1].dimensions[0];
        assert_eq!(
            (cpu.dimension, cpu.used, cpu.adding, cpu.limit, cpu.exceeded),
            (QuotaDimension::Cpu, 14, 4, 16, true)
        );
        assert_eq!(
            c.refusal().as_deref(),
            Some("project Shop: 14 CPUs in use, this adds 4, quota 16")
        );
        // Exactly at the limit is allowed.
        let fits = check(&scopes, QuotaUsage { cpu: 2, ..adding });
        assert!(fits.allowed, "{:?}", fits.refusal());
    }

    #[test]
    fn adding_nothing_never_fails_a_quota_below_use() {
        let scopes = [QuotaScopeInput {
            kind: QuotaScopeKind::Environment,
            id: "e".into(),
            name: "Dev".into(),
            limits: q(Some(1), None, None, None),
            used: QuotaUsage {
                instances: 5,
                ..QuotaUsage::default()
            },
        }];
        assert!(check(&scopes, QuotaUsage::default()).allowed);
        assert!(
            !check(
                &scopes,
                QuotaUsage {
                    instances: 1,
                    ..QuotaUsage::default()
                }
            )
            .allowed
        );
    }

    #[test]
    fn the_baseline_restricts_everything_apelora_does_not_need() {
        let c = baseline_config("env-1", &ProvisionSettings::default());
        assert_eq!(c["restricted"], "true");
        assert_eq!(c["restricted.containers.privilege"], "unprivileged");
        assert_eq!(c["restricted.containers.nesting"], "block");
        assert_eq!(c["restricted.devices.disk"], "managed");
        assert_eq!(c["restricted.devices.nic"], "managed");
        assert_eq!(c["restricted.cluster.target"], "allow", "moves need it");
        assert_eq!(c["restricted.snapshots"], "allow", "volume snapshots");
        assert_eq!(c["features.networks"], "false");
        assert_eq!(c[ENVIRONMENT_KEY], "env-1");
        for (k, v) in &c {
            if k.starts_with("restricted.devices.")
                && !matches!(
                    k.as_str(),
                    "restricted.devices.disk" | "restricted.devices.nic"
                )
            {
                assert_eq!(v, "block", "{k}");
            }
        }
        assert!(
            !c.keys().any(|k| k.starts_with("limits.")),
            "limits come from quotas"
        );
        let ovn = baseline_config(
            "e",
            &ProvisionSettings {
                uplink: Some("incusbr0".into()),
                service_range: Some("10.99.0.0/24".into()),
            },
        );
        assert_eq!(ovn["features.networks"], "true");
        assert_eq!(ovn["restricted.networks.uplinks"], "incusbr0");
        assert_eq!(ovn["restricted.networks.subnets"], "incusbr0:10.99.0.0/24");
    }

    #[test]
    fn limits_replace_only_what_apelora_owns() {
        let current: BTreeMap<String, String> = [
            ("restricted", "true"),
            ("limits.cpu", "4"),
            ("limits.memory", "8GiB"),
            ("limits.processes", "500"),
        ]
        .into_iter()
        .map(|(k, v)| (k.into(), v.into()))
        .collect();
        let desired = limit_config(&q(Some(5), None, Some(16), None));
        assert_eq!(desired["limits.memory"], "16GiB");
        // Apelora wrote limits.cpu before; an administrator set limits.processes by hand.
        let written: BTreeMap<String, String> = [("limits.cpu".to_owned(), "4".to_owned())].into();
        let out = with_limits(&current, &desired, &written);
        assert_eq!(
            out.get("limits.cpu"),
            None,
            "Apelora's own limit is removed"
        );
        assert_eq!(out["limits.instances"], "5");
        assert_eq!(out["limits.memory"], "16GiB");
        assert_eq!(out["limits.processes"], "500", "not Apelora's key");
        assert_eq!(out["restricted"], "true");
        // A limit Apelora never wrote is left when no quota asks for that dimension.
        let untouched = with_limits(&current, &BTreeMap::new(), &BTreeMap::new());
        assert_eq!(untouched, current);
    }

    #[test]
    fn the_default_profile_copies_the_root_pool_and_sizes_it() {
        let root: BTreeMap<String, String> = [("type", "disk"), ("path", "/"), ("pool", "local")]
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect();
        let eth0: BTreeMap<String, String> =
            [("type", "nic"), ("network", "incusbr0"), ("name", "eth0")]
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect();
        let p = default_profile(Some(&root), Some(&eth0), None, false).expect("profile");
        assert_eq!(p.devices["root"]["pool"], "local");
        assert_eq!(p.devices["root"]["size"], "10GiB");
        assert_eq!(p.devices["eth0"]["network"], "incusbr0");
        assert_eq!(p.config["limits.cpu"], "1");
        assert_eq!(p.config["limits.memory"], "1GiB");
        let ovn = default_profile(Some(&root), Some(&eth0), None, true).expect("profile");
        assert!(
            !ovn.devices.contains_key("eth0"),
            "with an uplink, applications bring their own network"
        );
        assert_eq!(
            default_profile(None, None, Some("data"), true)
                .expect("fallback")
                .devices["root"]["pool"],
            "data"
        );
        assert!(default_profile(None, None, None, true).is_err());
    }

    #[test]
    fn findings_name_what_an_adopted_project_allows() {
        let s = ProvisionSettings::default();
        let unrestricted = findings(&BTreeMap::new(), &s);
        assert_eq!(unrestricted.len(), 1);
        assert_eq!(unrestricted[0].severity, TenancyFindingSeverity::Error);
        // The lab's deploy project: restricted with defaults, snapshots and targets allowed.
        let lab: BTreeMap<String, String> = [
            ("restricted", "true"),
            ("restricted.snapshots", "allow"),
            ("restricted.cluster.target", "allow"),
        ]
        .into_iter()
        .map(|(k, v)| (k.into(), v.into()))
        .collect();
        assert_eq!(
            findings(&lab, &s),
            vec![],
            "Incus's defaults match the baseline"
        );
        let mut loose = lab.clone();
        loose.insert("restricted.devices.gpu".into(), "allow".into());
        let f = findings(&loose, &s);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].severity, TenancyFindingSeverity::Warning);
        assert_eq!(f[0].key.as_deref(), Some("restricted.devices.gpu"));
    }

    #[test]
    fn bytes_read_as_people_expect() {
        assert_eq!(show_bytes(10 * GIB), "10 GiB");
        assert_eq!(show_bytes(GIB + GIB / 2), "1.5 GiB");
        assert_eq!(show_bytes(512 * MIB), "512 MiB");
    }
}

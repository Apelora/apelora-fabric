// SPDX-License-Identifier: AGPL-3.0-only
//! Persistent volumes of an appliance (docs/architecture/STORAGE.md).
//!
//! A design asks for a volume by name, size, class and mount path; it never names a pool. The
//! control plane resolves the class to a pool before the spec reaches the worker. Each instance
//! gets its own volumes, named after it, created on the member Incus placed the instance on
//! (Incus does not place an instance next to a member-local volume). Volumes outlive their
//! instance: removing an appliance never deletes its data.

use serde::{Deserialize, Serialize};

use crate::{InstanceKind, is_valid_name};

/// Volume config key naming the design's volume (`data`); the application and appliance keys
/// are the instance ones ([`crate::APPLICATION_KEY`], [`crate::APPLIANCE_KEY`]).
pub const VOLUME_KEY: &str = "user.apelora.volume";
/// Volumes one appliance may ask for.
pub const MAX_VOLUMES: usize = 4;
/// Smallest and largest volume Apelora creates, in MiB (64 MiB to 4 TiB).
pub const MIN_SIZE_MIB: u32 = 64;
pub const MAX_SIZE_MIB: u32 = 4 * 1024 * 1024;
/// Longest volume name in a design, so `<instance>-<volume>` stays a valid Incus name.
const MAX_NAME: usize = 16;

/// What the volume holds: a mounted filesystem or a raw disk (virtual machines only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum VolumeContent {
    #[default]
    Filesystem,
    Block,
}

impl VolumeContent {
    pub fn as_str(self) -> &'static str {
        match self {
            VolumeContent::Filesystem => "filesystem",
            VolumeContent::Block => "block",
        }
    }
}

/// The policy a volume is created under. Only `data-local` exists so far: persistent, one
/// writer, on the member's own pool; moving it means stopping the instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum VolumeClass {
    #[default]
    DataLocal,
}

impl VolumeClass {
    pub fn as_str(self) -> &'static str {
        match self {
            VolumeClass::DataLocal => "data-local",
        }
    }
}

/// A volume an appliance asks for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct VolumeMount {
    /// Name in the design, e.g. `data`: lowercase letters, digits and `-`, at most 16.
    pub name: String,
    pub size_mib: u32,
    #[serde(default)]
    pub content: VolumeContent,
    #[serde(default)]
    pub class: VolumeClass,
    /// Where a filesystem volume is mounted inside the instance; none for a block volume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The pool the class resolves to. Set by the control plane, never by a caller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool: Option<String>,
}

impl VolumeMount {
    /// Checks the volume on its own and for an instance of `kind`.
    pub fn validate(&self, kind: InstanceKind) -> Result<(), String> {
        let n = &self.name;
        if n.len() > MAX_NAME || !is_valid_name(n) || n.bytes().any(|c| c.is_ascii_uppercase()) {
            return Err(format!(
                "invalid volume name {n:?} (lowercase letters, digits and hyphens, at most {MAX_NAME})"
            ));
        }
        if !(MIN_SIZE_MIB..=MAX_SIZE_MIB).contains(&self.size_mib) {
            return Err(format!(
                "volume {n}: size must be {MIN_SIZE_MIB} MiB to {} GiB",
                MAX_SIZE_MIB / 1024
            ));
        }
        match (self.content, &self.path) {
            (VolumeContent::Filesystem, None) => {
                return Err(format!(
                    "volume {n}: a filesystem volume needs a mount path"
                ));
            }
            (VolumeContent::Filesystem, Some(p)) => {
                valid_mount_path(p).map_err(|why| format!("volume {n}: mount path {p:?} {why}"))?
            }
            (VolumeContent::Block, Some(_)) => {
                return Err(format!("volume {n}: a block volume has no mount path"));
            }
            (VolumeContent::Block, None) if kind != InstanceKind::VirtualMachine => {
                return Err(format!(
                    "volume {n}: block volumes attach only to virtual machines"
                ));
            }
            (VolumeContent::Block, None) => {}
        }
        if let Some(p) = &self.pool
            && !is_valid_name(p)
        {
            return Err(format!("volume {n}: invalid pool name {p:?}"));
        }
        Ok(())
    }

    /// The Incus disk device the volume is attached as.
    pub fn device(&self) -> String {
        format!("vol-{}", self.name)
    }
}

/// The Incus volume of `instance` for the design's volume `volume`.
pub fn volume_name(instance: &str, volume: &str) -> String {
    format!("{instance}-{volume}")
}

/// An absolute path without `.` or `..` segments, not `/` itself, of printable characters.
fn valid_mount_path(p: &str) -> Result<(), &'static str> {
    if !p.starts_with('/') || p == "/" {
        return Err("must be an absolute path below /");
    }
    if p.len() > 255 || p.chars().any(|c| c.is_control()) {
        return Err("is too long or has control characters");
    }
    if p.split('/').any(|s| s == "." || s == "..") {
        return Err("must not contain . or ..");
    }
    Ok(())
}

/// Checks a set of volumes for one instance: each on its own, names and paths unique, and the
/// Incus volume names valid for `instance`.
pub fn validate_volumes(
    instance: &str,
    kind: InstanceKind,
    volumes: &[VolumeMount],
) -> Result<(), String> {
    if volumes.len() > MAX_VOLUMES {
        return Err(format!("at most {MAX_VOLUMES} volumes per appliance"));
    }
    let mut names = std::collections::BTreeSet::new();
    let mut paths = std::collections::BTreeSet::new();
    for v in volumes {
        v.validate(kind)?;
        if !names.insert(v.name.as_str()) {
            return Err(format!("volume {} is listed twice", v.name));
        }
        if let Some(p) = &v.path
            && !paths.insert(p.trim_end_matches('/'))
        {
            return Err(format!("two volumes are mounted at {p}"));
        }
        let full = volume_name(instance, &v.name);
        if !is_valid_name(&full) {
            return Err(format!(
                "volume {}: {full:?} is too long for Incus; shorten the appliance or volume name",
                v.name
            ));
        }
    }
    Ok(())
}

/// Resize is grow-only: Incus accepts a shrink, which can destroy data.
pub fn check_resize(current_mib: u32, wanted_mib: u32) -> Result<(), String> {
    if wanted_mib < current_mib {
        return Err(format!(
            "volumes only grow: {wanted_mib} MiB is smaller than the current {current_mib} MiB"
        ));
    }
    if wanted_mib > MAX_SIZE_MIB {
        return Err(format!("at most {} GiB", MAX_SIZE_MIB / 1024));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fs(name: &str, path: &str) -> VolumeMount {
        VolumeMount {
            name: name.into(),
            size_mib: 1024,
            content: VolumeContent::Filesystem,
            class: VolumeClass::DataLocal,
            path: Some(path.into()),
            pool: None,
        }
    }

    #[test]
    fn a_filesystem_volume_needs_a_safe_absolute_path() {
        let ok = fs("data", "/var/lib/postgresql");
        assert_eq!(ok.validate(InstanceKind::Container), Ok(()));
        for bad in ["relative", "/", "/srv/../etc", "/srv/./x", "/a\nb"] {
            assert!(
                fs("data", bad).validate(InstanceKind::Container).is_err(),
                "{bad:?} accepted"
            );
        }
        let none = VolumeMount {
            path: None,
            ..ok.clone()
        };
        assert!(none.validate(InstanceKind::Container).is_err());
    }

    #[test]
    fn names_are_short_lowercase_and_sizes_bounded() {
        for bad in ["Data", "-x", "x-", "a_b", "abcdefghijklmnopq", ""] {
            assert!(
                fs(bad, "/srv").validate(InstanceKind::Container).is_err(),
                "{bad:?} accepted"
            );
        }
        assert!(
            fs("abcdefghijklmnop", "/srv")
                .validate(InstanceKind::Container)
                .is_ok()
        );
        for (size, ok) in [
            (63, false),
            (64, true),
            (MAX_SIZE_MIB, true),
            (MAX_SIZE_MIB + 1, false),
        ] {
            let v = VolumeMount {
                size_mib: size,
                ..fs("data", "/srv")
            };
            assert_eq!(v.validate(InstanceKind::Container).is_ok(), ok, "{size}");
        }
    }

    #[test]
    fn block_volumes_are_for_virtual_machines_and_have_no_path() {
        let block = VolumeMount {
            content: VolumeContent::Block,
            path: None,
            ..fs("disk", "/x")
        };
        assert!(block.validate(InstanceKind::VirtualMachine).is_ok());
        assert!(block.validate(InstanceKind::Container).is_err());
        let with_path = VolumeMount {
            path: Some("/srv".into()),
            ..block
        };
        assert!(with_path.validate(InstanceKind::VirtualMachine).is_err());
    }

    #[test]
    fn a_set_has_unique_names_and_paths_and_fits_incus_names() {
        let k = InstanceKind::Container;
        assert!(validate_volumes("shop-db-01", k, &[fs("data", "/a"), fs("logs", "/b")]).is_ok());
        assert!(validate_volumes("shop-db-01", k, &[fs("data", "/a"), fs("data", "/b")]).is_err());
        assert!(validate_volumes("shop-db-01", k, &[fs("data", "/a"), fs("logs", "/a/")]).is_err());
        let five: Vec<_> = (0..5)
            .map(|i| fs(&format!("v{i}"), &format!("/v{i}")))
            .collect();
        assert!(validate_volumes("shop-db-01", k, &five).is_err());
        let long = "a".repeat(59);
        assert!(validate_volumes(&long, k, &[fs("data", "/a")]).is_err());
    }

    #[test]
    fn resize_only_grows() {
        assert!(check_resize(1024, 1024).is_ok());
        assert!(check_resize(1024, 2048).is_ok());
        assert!(check_resize(1024, 512).is_err());
        assert!(check_resize(1024, MAX_SIZE_MIB + 1).is_err());
    }

    #[test]
    fn names_of_the_incus_volume_and_device() {
        assert_eq!(volume_name("shop-db-01", "data"), "shop-db-01-data");
        assert_eq!(fs("data", "/srv").device(), "vol-data");
    }
}

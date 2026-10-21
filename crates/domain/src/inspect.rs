// SPDX-License-Identifier: AGPL-3.0-only
//! One instance as Incus reports it, for its detail page: state, placement, image, limits and
//! use, addresses, disks and snapshots. Every field is what Incus returned; anything it did not
//! report is `None` or empty, never a default that looks like a value.

use serde::{Deserialize, Serialize};

use crate::{InstanceKind, Owner};

/// An instance in detail (`/1.0/instances/{name}`, its `state` and its `snapshots`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct InstanceView {
    pub name: String,
    pub project: String,
    pub kind: InstanceKind,
    /// `Running`, `Stopped`, `Frozen`, `Error`, …
    pub status: String,
    /// The member it runs on; empty on a standalone server.
    pub location: String,
    pub architecture: String,
    pub description: String,
    /// Carries Apelora's ownership marker.
    pub managed: bool,
    /// The application and module it was deployed for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<Owner>,
    /// RFC 3339, as Incus reports them.
    pub created_at: Option<String>,
    pub last_used_at: Option<String>,
    /// When the running instance started (`instance_state_started_at`).
    pub started_at: Option<String>,
    pub profiles: Vec<String>,
    pub image: InstanceViewImage,
    pub limits: InstanceViewLimits,
    /// `None` while stopped (Incus reports no usage then).
    pub usage: Option<InstanceViewUsage>,
    pub addresses: Vec<InstanceViewAddress>,
    pub disks: Vec<InstanceViewDisk>,
    pub snapshots: Vec<InstanceViewSnapshot>,
}

/// The image it was built from (`image.*` and `volatile.base_image` config).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct InstanceViewImage {
    pub description: Option<String>,
    pub os: Option<String>,
    pub release: Option<String>,
    pub fingerprint: Option<String>,
}

/// Limits from the expanded configuration (profiles applied); `None`: no limit set.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct InstanceViewLimits {
    /// `limits.cpu` as written (a count or a set of CPUs).
    pub cpu: Option<String>,
    /// `limits.memory` as written (e.g. `2GiB`, `50%`).
    pub memory: Option<String>,
    /// `size` of the root disk device as written.
    pub root_disk: Option<String>,
}

/// Use reported by Incus for a running instance.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct InstanceViewUsage {
    /// CPU time used since start, in nanoseconds.
    pub cpu_ns: Option<i64>,
    pub memory_bytes: Option<i64>,
    pub memory_peak_bytes: Option<i64>,
    /// Memory available to it, when Incus reports it (`instances_state_total`).
    pub memory_total_bytes: Option<i64>,
    pub processes: Option<i64>,
    /// Root disk use in bytes.
    pub root_disk_bytes: Option<i64>,
}

/// One address on one interface, as the instance reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct InstanceViewAddress {
    pub interface: String,
    /// `inet` or `inet6`.
    pub family: String,
    pub address: String,
    pub netmask: String,
    /// `global`, `link` or `local`.
    pub scope: String,
}

/// A disk device (expanded devices: profiles applied).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct InstanceViewDisk {
    pub device: String,
    /// Mount path in the instance; `/` for the root disk; `None` for a block device.
    pub path: Option<String>,
    pub pool: Option<String>,
    /// The custom volume, for disks backed by one.
    pub volume: Option<String>,
    pub size: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct InstanceViewSnapshot {
    pub name: String,
    pub created_at: Option<String>,
    pub expires_at: Option<String>,
    /// Includes the running state (memory).
    pub stateful: bool,
}

/// The end of a log as read: at most the requested number of bytes from its end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct LogTail {
    /// The text (invalid UTF-8 replaced); starts at a line boundary when it was cut.
    pub content: String,
    /// The whole log's size in bytes as received.
    pub size: u64,
    /// Only the end is shown.
    pub truncated: bool,
}

impl LogTail {
    /// The last `max` bytes of `bytes`, cut forward to the next line start so no line is shown
    /// half.
    pub fn from_bytes(bytes: &[u8], size: u64, max: usize) -> Self {
        let truncated = bytes.len() > max || size > bytes.len() as u64;
        let start = bytes.len().saturating_sub(max);
        let mut tail = &bytes[start..];
        if truncated && let Some(nl) = tail.iter().position(|b| *b == b'\n') {
            tail = &tail[nl + 1..];
        }
        LogTail {
            content: String::from_utf8_lossy(tail).into_owned(),
            size,
            truncated,
        }
    }
}

/// A log file name as Incus lists it: the last segment of its URL, and only names that cannot
/// address anything else (no separators, no `..`).
pub fn log_file_name(url_or_name: &str) -> Option<&str> {
    let name = url_or_name
        .split('?')
        .next()
        .unwrap_or_default()
        .rsplit('/')
        .next()
        .unwrap_or_default();
    let ok = !name.is_empty()
        && name.len() <= 128
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    ok.then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tail_starts_at_a_line_and_says_it_was_cut() {
        let log = b"one\ntwo\nthree\nfour\n";
        let all = LogTail::from_bytes(log, log.len() as u64, 1024);
        assert_eq!(
            (all.content.as_str(), all.truncated),
            ("one\ntwo\nthree\nfour\n", false)
        );
        let end = LogTail::from_bytes(log, log.len() as u64, 9);
        assert_eq!((end.content.as_str(), end.truncated), ("four\n", true));
        let bad = LogTail::from_bytes(&[0xff, b'a'], 2, 10);
        assert_eq!(bad.content, "\u{fffd}a");
    }

    #[test]
    fn only_plain_log_names_are_accepted() {
        assert_eq!(
            log_file_name("/1.0/instances/web/logs/lxc.log"),
            Some("lxc.log")
        );
        assert_eq!(log_file_name("qemu.log"), Some("qemu.log"));
        for bad in [
            "",
            "..",
            "/1.0/instances/web/logs/",
            "a b",
            "x%2F..",
            "lxc.log;rm",
        ] {
            assert_eq!(log_file_name(bad), None, "{bad}");
        }
    }
}

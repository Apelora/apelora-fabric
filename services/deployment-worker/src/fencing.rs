// SPDX-License-Identifier: AGPL-3.0-only
//! The fence probe (ADR 0013): reads the shared volume group's lockspace (`sanlock client gets
//! -h 1`) on a member through an SSH forced command. Each member has an unprivileged account
//! (`apelora-fence`, group `sanlock`) whose only authorised key runs exactly that read-only
//! command; the worker holds the key and pins every member's host key. No agent, no daemon.
//!
//! Settings: `APELORA_FENCE_PROBE` (`off`, the default, or `ssh`), `APELORA_FENCE_PROBE_KEY`
//! (private key file), `APELORA_FENCE_PROBE_KNOWN_HOSTS` (pinned host keys),
//! `APELORA_FENCE_PROBE_USER` (default `apelora-fence`), `APELORA_FENCE_PROBE_TARGETS`
//! (`member=host[:port],…`; default: the host of the member's Incus address, port 22) and
//! `APELORA_FENCE_LOCKSPACE` (default: the only lvmlockd lockspace joined).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use apelora_domain::ClusterMember;
use apelora_domain::fencing::{FenceObservation, parse_gets, pick_lockspace};

/// How long one probe may take, connection included.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// The most output a probe may return; a lockspace listing is a few hundred bytes.
const MAX_OUTPUT: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub struct SshProbe {
    key: PathBuf,
    known_hosts: PathBuf,
    user: String,
    targets: BTreeMap<String, (String, u16)>,
}

/// Where lease readings come from.
#[derive(Debug, Clone, Default)]
pub enum Probe {
    /// No probe: only a person's confirmation fences a member.
    #[default]
    Off,
    Ssh(SshProbe),
    /// Fixed readings per member: what `sanlock client gets -h 1` prints there, or an error.
    /// For tests and development; never configured from the environment.
    Fixed(Arc<Mutex<BTreeMap<String, Result<String, String>>>>),
}

/// A host name or address safe to hand to `ssh` (never an option).
fn valid_host(h: &str) -> bool {
    !h.is_empty()
        && h.len() <= 253
        && !h.starts_with('-')
        && h.bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b':' | b'_'))
}

fn parse_targets(v: &str) -> Result<BTreeMap<String, (String, u16)>, String> {
    let mut out = BTreeMap::new();
    for part in v.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let (member, at) = part
            .split_once('=')
            .ok_or_else(|| format!("{part:?}: use member=host[:port]"))?;
        if !apelora_domain::is_valid_name(member) {
            return Err(format!("{member:?} is not a member name"));
        }
        let (host, port) = match at.rsplit_once(':') {
            Some((h, p)) if !h.contains(':') => (
                h,
                p.parse::<u16>()
                    .map_err(|_| format!("{part:?}: invalid port"))?,
            ),
            _ => (at, 22),
        };
        if !valid_host(host) {
            return Err(format!("{part:?}: invalid host"));
        }
        out.insert(member.to_owned(), (host.to_owned(), port));
    }
    Ok(out)
}

/// The probe and the lockspace from the environment.
pub fn probe_from_env(
    get: impl Fn(&str) -> Option<String>,
) -> Result<(Probe, Option<String>), String> {
    let lockspace = get("APELORA_FENCE_LOCKSPACE")
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty());
    let probe = match get("APELORA_FENCE_PROBE").as_deref().map(str::trim) {
        None | Some("" | "off") => Probe::Off,
        Some("ssh") => {
            let path = |k: &str| {
                get(k)
                    .map(|v| v.trim().to_owned())
                    .filter(|v| !v.is_empty())
                    .map(PathBuf::from)
                    .ok_or_else(|| format!("APELORA_FENCE_PROBE=ssh needs {k}"))
            };
            let key = path("APELORA_FENCE_PROBE_KEY")?;
            let known_hosts = path("APELORA_FENCE_PROBE_KNOWN_HOSTS")?;
            for (k, p) in [("key", &key), ("known hosts", &known_hosts)] {
                if !p.is_file() {
                    return Err(format!(
                        "the fence probe's {k} file {} is missing",
                        p.display()
                    ));
                }
            }
            let user = get("APELORA_FENCE_PROBE_USER")
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| "apelora-fence".into());
            if !apelora_domain::is_valid_name(&user) {
                return Err(format!(
                    "APELORA_FENCE_PROBE_USER: invalid account {user:?}"
                ));
            }
            let targets = parse_targets(&get("APELORA_FENCE_PROBE_TARGETS").unwrap_or_default())
                .map_err(|e| format!("APELORA_FENCE_PROBE_TARGETS: {e}"))?;
            Probe::Ssh(SshProbe {
                key,
                known_hosts,
                user,
                targets,
            })
        }
        Some(other) => return Err(format!("APELORA_FENCE_PROBE={other:?}: use off or ssh")),
    };
    Ok((probe, lockspace))
}

/// The host part of an Incus address (`https://192.168.104.3:8443`).
fn url_host(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let host = rest.split('/').next()?;
    let host = match host.strip_prefix('[') {
        Some(v6) => v6.split(']').next()?,
        None => host.rsplit_once(':').map_or(host, |(h, _)| h),
    };
    valid_host(host).then(|| host.to_owned())
}

impl Probe {
    pub fn configured(&self) -> bool {
        !matches!(self, Probe::Off)
    }

    /// What `sanlock client gets -h 1` prints on `member`.
    async fn read_text(&self, member: &ClusterMember) -> Result<String, String> {
        match self {
            Probe::Off => Err("no fence probe is configured".into()),
            Probe::Fixed(m) => m
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&member.name)
                .cloned()
                .unwrap_or_else(|| Err("no reading for this member".into())),
            Probe::Ssh(s) => {
                let (host, port) = match s.targets.get(&member.name) {
                    Some(t) => t.clone(),
                    None => (
                        url_host(&member.url)
                            .ok_or_else(|| format!("no probe address for {}", member.name))?,
                        22,
                    ),
                };
                let mut cmd = tokio::process::Command::new("ssh");
                cmd.args(["-F", "/dev/null", "-T", "-i"])
                    .arg(&s.key)
                    .args([
                        "-o",
                        "IdentitiesOnly=yes",
                        "-o",
                        "BatchMode=yes",
                        "-o",
                        "StrictHostKeyChecking=yes",
                        "-o",
                    ])
                    .arg(format!("UserKnownHostsFile={}", s.known_hosts.display()))
                    .args([
                        "-o",
                        "GlobalKnownHostsFile=/dev/null",
                        "-o",
                        "ConnectTimeout=5",
                        "-o",
                        "LogLevel=ERROR",
                        "-p",
                    ])
                    .arg(port.to_string())
                    .args(["-l", &s.user, &host, "lockspaces"])
                    .stdin(std::process::Stdio::null())
                    .kill_on_drop(true);
                let out = tokio::time::timeout(PROBE_TIMEOUT, cmd.output())
                    .await
                    .map_err(|_| format!("no answer within {} s", PROBE_TIMEOUT.as_secs()))?
                    .map_err(|e| format!("ssh could not be run: {e}"))?;
                if !out.status.success() {
                    let err = String::from_utf8_lossy(&out.stderr);
                    let err: String = err.split_whitespace().collect::<Vec<_>>().join(" ");
                    return Err(format!(
                        "the probe failed ({}): {}",
                        out.status,
                        err.chars().take(200).collect::<String>()
                    ));
                }
                if out.stdout.len() > MAX_OUTPUT {
                    return Err("the probe's answer is too long".into());
                }
                String::from_utf8(out.stdout).map_err(|_| "the probe's answer is not text".into())
            }
        }
    }

    /// The shared lockspace as `member`'s sanlock sees it now.
    pub async fn observe(
        &self,
        member: &ClusterMember,
        lockspace: Option<&str>,
        now: i64,
    ) -> Result<FenceObservation, String> {
        let text = self.read_text(member).await?;
        let spaces = parse_gets(&text)?;
        let ls = pick_lockspace(&spaces, lockspace)?;
        Ok(FenceObservation {
            observed_by: member.name.clone(),
            observed_at: now,
            lockspace: ls.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_are_checked_and_targets_parsed() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| (*v).to_owned())
            }
        };
        assert!(matches!(probe_from_env(env(&[])), Ok((Probe::Off, None))));
        assert!(probe_from_env(env(&[("APELORA_FENCE_PROBE", "ipmi")])).is_err());
        let e = probe_from_env(env(&[("APELORA_FENCE_PROBE", "ssh")])).expect_err("refused");
        assert!(e.contains("APELORA_FENCE_PROBE_KEY"), "{e}");
        let e = probe_from_env(env(&[
            ("APELORA_FENCE_PROBE", "ssh"),
            ("APELORA_FENCE_PROBE_KEY", "/nonexistent/key"),
            ("APELORA_FENCE_PROBE_KNOWN_HOSTS", "/nonexistent/known"),
        ]))
        .expect_err("refused");
        assert!(e.contains("missing"), "{e}");
        let t = parse_targets("m1=127.0.0.1:50055, m2=host-2").expect("targets");
        assert_eq!(t["m1"], ("127.0.0.1".into(), 50055));
        assert_eq!(t["m2"], ("host-2".into(), 22));
        assert!(
            parse_targets("m1=-oProxyCommand=x").is_err(),
            "never an ssh option"
        );
        assert!(parse_targets("m1").is_err());
        assert_eq!(
            url_host("https://192.168.104.3:8443").as_deref(),
            Some("192.168.104.3")
        );
        assert_eq!(
            url_host("https://[fd00::1]:8443").as_deref(),
            Some("fd00::1")
        );
        assert_eq!(url_host("not a url"), None);
    }

    #[tokio::test]
    async fn a_fixed_reading_is_parsed_into_an_observation() {
        let fixed = Probe::Fixed(Arc::new(Mutex::new(BTreeMap::from([(
            "m2".to_owned(),
            Ok("s lvm_x:2:/dev/x:0\nh 2 gen 1 timestamp 9 LIVE\n".to_owned()),
        )]))));
        let mut m = ClusterMember {
            name: "m2".into(),
            url: String::new(),
            roles: vec![],
            status: "Online".into(),
            message: String::new(),
            architecture: String::new(),
            failure_domain: String::new(),
            database: false,
            groups: vec![],
            scheduler: "all".into(),
        };
        let o = fixed.observe(&m, None, 5).await.expect("observe");
        assert_eq!(
            (o.observed_by.as_str(), o.lockspace.local_host_id),
            ("m2", 2)
        );
        m.name = "m3".into();
        assert!(fixed.observe(&m, None, 5).await.is_err());
        assert!(!Probe::Off.configured());
    }
}

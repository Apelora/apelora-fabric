// SPDX-License-Identifier: AGPL-3.0-only
//! Fencing a failed member before anything of it restarts on another member (ADR 0013).
//!
//! A member is fenced when Incus reports it Offline **and** one of:
//! - its sanlock host lease in the shared volume group's lockspace is `DEAD`, as read on a
//!   healthy member, with the lease generation Apelora last saw `LIVE` while the member was
//!   Online. sanlock declares a host dead `host_dead_seconds` (8 × `io_timeout` + the 60 s
//!   watchdog timeout: 140 s by default) after its last renewal, by which time its watchdog has
//!   reset it if it was still running (sanlock(8), lvmlockd(8));
//! - a person confirmed that the server is powered off;
//! - its power state, read back from its BMC after a power-off (or already Off), is Off (ADR 0014).
//!
//! Time alone never fences: a server cut off from the other servers but not from the SAN keeps
//! renewing its lease, and virtual-machine disks on clustered LVM are activated shared, so the
//! lock layer would not stop a second writer of them. Pure: no I/O.

use serde::{Deserialize, Serialize};

/// sanlock's default `host_dead_seconds` with `io_timeout` 10 s: 8 × 10 s until renewals count
/// as failed, plus the 60 s watchdog timeout.
pub const HOST_DEAD_SECS: i64 = 140;

/// How a remote host's lease looks to sanlock on the member that was asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum FenceLeaseState {
    /// Not held (never used, or released cleanly).
    Free,
    /// Being renewed: the host is running.
    Live,
    /// Renewals stopped recently; not yet past `host_dead_seconds`.
    Fail,
    /// Not renewed for `host_dead_seconds`: the host stopped, or its watchdog reset it.
    Dead,
    Unknown,
}

impl FenceLeaseState {
    fn parse(s: &str) -> FenceLeaseState {
        match s {
            "FREE" => FenceLeaseState::Free,
            "LIVE" => FenceLeaseState::Live,
            "FAIL" => FenceLeaseState::Fail,
            "DEAD" => FenceLeaseState::Dead,
            _ => FenceLeaseState::Unknown,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            FenceLeaseState::Free => "FREE",
            FenceLeaseState::Live => "LIVE",
            FenceLeaseState::Fail => "FAIL",
            FenceLeaseState::Dead => "DEAD",
            FenceLeaseState::Unknown => "UNKNOWN",
        }
    }
}

/// One host's lease in a lockspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct FenceLeaseHost {
    pub host_id: u32,
    /// Grows each time the host joins the lockspace again.
    pub generation: u64,
    pub state: FenceLeaseState,
}

/// One lockspace as one member's sanlock daemon reports it (`sanlock client gets -h 1`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct FenceLockspace {
    /// e.g. `lvm_apelora-shared` (lvmlockd names it after the volume group).
    pub name: String,
    /// The host id of the member that was asked.
    pub local_host_id: u32,
    pub hosts: Vec<FenceLeaseHost>,
}

/// Parses the output of `sanlock client gets -h 1`:
///
/// ```text
/// s lvm_apelora-shared:2:/dev/mapper/apelora--shared-lvmlock:0
/// h 1 gen 1 timestamp 97252 LIVE
/// ```
///
/// Lines of other kinds are ignored; a malformed `s` or `h` line is an error (the reading is
/// then not used at all).
pub fn parse_gets(text: &str) -> Result<Vec<FenceLockspace>, String> {
    let mut out: Vec<FenceLockspace> = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let mut words = line.split_whitespace();
        match words.next() {
            Some("s") => {
                let spec = words.next().ok_or("an `s` line without a lockspace")?;
                let mut parts = spec.split(':');
                let name = parts.next().filter(|n| !n.is_empty());
                let id = parts.next().and_then(|v| v.parse::<u32>().ok());
                match (name, id) {
                    (Some(name), Some(id)) => out.push(FenceLockspace {
                        name: name.to_owned(),
                        local_host_id: id,
                        hosts: Vec::new(),
                    }),
                    _ => return Err(format!("unreadable lockspace line {line:?}")),
                }
            }
            Some("h") => {
                let w: Vec<&str> = words.collect();
                let host = match w.as_slice() {
                    [id, "gen", generation, "timestamp", _ts, state, ..] => {
                        match (id.parse::<u32>(), generation.parse::<u64>()) {
                            (Ok(host_id), Ok(generation)) => FenceLeaseHost {
                                host_id,
                                generation,
                                state: FenceLeaseState::parse(state),
                            },
                            _ => return Err(format!("unreadable host line {line:?}")),
                        }
                    }
                    _ => return Err(format!("unreadable host line {line:?}")),
                };
                out.last_mut()
                    .ok_or("a host line before any lockspace")?
                    .hosts
                    .push(host);
            }
            _ => {}
        }
    }
    Ok(out)
}

/// The lockspace of the shared pool: the configured one, or the only lvmlockd lockspace.
pub fn pick_lockspace<'a>(
    spaces: &'a [FenceLockspace],
    configured: Option<&str>,
) -> Result<&'a FenceLockspace, String> {
    match configured {
        Some(name) => spaces
            .iter()
            .find(|s| s.name == name)
            .ok_or_else(|| format!("lockspace {name} is not joined on this member")),
        None => {
            let lvm: Vec<&FenceLockspace> = spaces
                .iter()
                .filter(|s| s.name.starts_with("lvm_"))
                .collect();
            match lvm.as_slice() {
                [one] => Ok(one),
                [] => Err("no shared volume group's lockspace is joined on this member".into()),
                _ => Err("several lockspaces are joined; set APELORA_FENCE_LOCKSPACE".into()),
            }
        }
    }
}

/// What one healthy member's sanlock said, when (Unix seconds).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct FenceObservation {
    pub observed_by: String,
    pub observed_at: i64,
    pub lockspace: FenceLockspace,
}

/// A member's host lease, learned from the member's own reading while it was Online.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnownHost {
    pub member: String,
    pub lockspace: String,
    pub host_id: u32,
    pub generation: u64,
}

/// Learns `member`'s own host id and generation from its own reading, taken while it is
/// Online: the lockspace's local host id, which must be LIVE.
pub fn learn(member: &str, own: &FenceObservation) -> Option<KnownHost> {
    let ls = &own.lockspace;
    ls.hosts
        .iter()
        .find(|h| h.host_id == ls.local_host_id && h.state == FenceLeaseState::Live)
        .map(|h| KnownHost {
            member: member.to_owned(),
            lockspace: ls.name.clone(),
            host_id: h.host_id,
            generation: h.generation,
        })
}

/// How the failed member was fenced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum FenceKind {
    /// Its storage lease expired (sanlock: DEAD).
    StorageLease,
    /// A person confirmed it is powered off.
    Confirmed,
    /// Its BMC reported it Off, read back after Apelora switched it off (ADR 0014).
    PowerOff,
}

/// The evidence a fencing decision rests on; recorded and audited with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct FenceEvidence {
    pub kind: FenceKind,
    /// When it was observed or confirmed (Unix seconds).
    pub at: i64,
    /// The member whose sanlock reported the lease (storage lease).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lockspace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<u64>,
    /// Who confirmed (confirmation).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmed_by: Option<String>,
    /// Which fence provider read the power state (power-off): `redfish`, `lima`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    pub summary: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum FenceCondition {
    Waiting,
    Fenced,
}

/// A failed member's fencing state, with the reason and what a person can do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct FenceView {
    pub condition: FenceCondition,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_step: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<FenceEvidence>,
    /// The latest reading of the member's lease, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_state: Option<FenceLeaseState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<i64>,
}

impl FenceView {
    fn waiting(reason: impl Into<String>, next: Option<&str>) -> FenceView {
        FenceView {
            condition: FenceCondition::Waiting,
            reason: reason.into(),
            next_step: next.map(str::to_owned),
            evidence: None,
            lease_state: None,
            observed_by: None,
            observed_at: None,
        }
    }

    pub fn fenced(&self) -> bool {
        self.condition == FenceCondition::Fenced
    }
}

/// What a fence provider read back from a member's BMC (ADR 0014).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PowerOffReading {
    /// `redfish` or `lima`.
    pub provider: String,
    /// What was read: the BMC's system (a Redfish system path, a lab VM name).
    pub system: String,
    /// When the Off state was read back (Unix seconds).
    pub at: i64,
    /// Apelora switched it off in this pass (`false`: it was already off).
    pub switched_off: bool,
}

/// The shortest time a member may be Offline before Apelora switches it off.
pub const MIN_FENCE_AFTER_SECS: i64 = 20;
/// A member Apelora switched off is not switched off again within this time, in case someone
/// switched it back on while Incus still has not seen it.
pub const POWER_OFF_COOLDOWN_SECS: i64 = 600;

/// What the guards for switching a member off look at.
pub struct PowerOffContext<'a> {
    pub member: &'a str,
    pub members: &'a [crate::ClusterMember],
    /// How long Incus has reported the member Offline (since the failure was detected).
    pub offline_for_secs: i64,
    pub after_secs: i64,
    /// Other members with an open failure that Apelora switched off.
    pub others_powered_off: usize,
    /// How long ago Apelora switched this member off in this failure, if it did.
    pub last_power_off_ago: Option<i64>,
}

/// Why Apelora must not switch the member off now; `None` when every guard holds. Fails
/// closed: anything unexpected is a reason.
pub fn power_off_refusal(c: &PowerOffContext<'_>) -> Option<String> {
    let Some(me) = c.members.iter().find(|m| m.name == c.member) else {
        return Some(format!("{} is not a member Incus lists", c.member));
    };
    if me.status != "Offline" {
        return Some(format!(
            "Incus reports {} {}, not Offline",
            c.member, me.status
        ));
    }
    let after = c.after_secs.max(MIN_FENCE_AFTER_SECS);
    if c.offline_for_secs < after {
        return Some(format!(
            "{} has been Offline for {} s; Apelora waits {after} s before switching a server off",
            c.member, c.offline_for_secs
        ));
    }
    let offline = c.members.iter().filter(|m| m.status == "Offline").count();
    if offline * 2 >= c.members.len() {
        return Some(format!(
            "{offline} of {} servers are Offline: more likely a problem of the control plane's own network than of those servers, so none is switched off",
            c.members.len()
        ));
    }
    if c.others_powered_off > 0 {
        return Some(
            "Apelora already switched another server off and it has not come back; switching off a second one needs a person (confirm it on Servers → Members)".into(),
        );
    }
    if let Some(ago) = c.last_power_off_ago
        && ago < POWER_OFF_COOLDOWN_SECS
    {
        return Some(format!(
            "Apelora switched {} off {ago} s ago; it waits {POWER_OFF_COOLDOWN_SECS} s before trying again",
            c.member
        ));
    }
    None
}

const CONFIRM: &str = "Check that the server is switched off (or cut off from the SAN), then confirm it on Servers → Members";

/// What a fencing decision looks at.
pub struct FenceInput<'a> {
    pub member: &'a str,
    /// Incus reports the member Offline.
    pub offline: bool,
    /// The member's host lease learned while it was Online.
    pub known: Option<&'a KnownHost>,
    /// Readings from other members (stale ones are ignored).
    pub observations: &'a [FenceObservation],
    /// A person's confirmation: who and when.
    pub confirmed: Option<(&'a str, i64)>,
    /// The power state read back from the member's BMC: Off (ADR 0014).
    pub power_off: Option<&'a PowerOffReading>,
    /// A lease probe is configured at all.
    pub probe: bool,
    pub now: i64,
    /// Readings older than this are not used.
    pub max_age_secs: i64,
}

/// Decides whether `member` is fenced, and if not, why not.
pub fn decide(i: &FenceInput<'_>) -> FenceView {
    if !i.offline {
        return FenceView::waiting("Incus does not report the server Offline", None);
    }
    if let Some((by, at)) = i.confirmed {
        return FenceView {
            condition: FenceCondition::Fenced,
            reason: format!("{by} confirmed the server is powered off"),
            next_step: None,
            evidence: Some(FenceEvidence {
                kind: FenceKind::Confirmed,
                at,
                observed_by: None,
                lockspace: None,
                host_id: None,
                generation: None,
                confirmed_by: Some(by.to_owned()),
                provider: None,
                summary: format!("{by} confirmed that {} is powered off", i.member),
            }),
            lease_state: None,
            observed_by: None,
            observed_at: None,
        };
    }
    if let Some(r) = i.power_off {
        return FenceView {
            condition: FenceCondition::Fenced,
            reason: format!(
                "its power state was read back from its BMC as Off ({})",
                r.provider
            ),
            next_step: None,
            evidence: Some(FenceEvidence {
                kind: FenceKind::PowerOff,
                at: r.at,
                observed_by: None,
                lockspace: None,
                host_id: None,
                generation: None,
                confirmed_by: None,
                provider: Some(r.provider.clone()),
                summary: format!(
                    "the {} provider read {} ({}) as powered Off at {}{}",
                    r.provider,
                    i.member,
                    r.system,
                    r.at,
                    if r.switched_off {
                        ", after switching it off"
                    } else {
                        ", already off"
                    }
                ),
            }),
            lease_state: None,
            observed_by: None,
            observed_at: None,
        };
    }
    if !i.probe {
        return FenceView::waiting(
            "no storage-lease probe is configured (APELORA_FENCE_PROBE), so Apelora cannot see whether the server stopped writing",
            Some(CONFIRM),
        );
    }
    let Some(known) = i.known else {
        return FenceView::waiting(
            "Apelora never read this server's storage lease while it was online, so it cannot tell whether the lease expired",
            Some(CONFIRM),
        );
    };
    let fresh = i
        .observations
        .iter()
        .filter(|o| {
            o.observed_by != i.member
                && o.lockspace.name == known.lockspace
                && i.now - o.observed_at <= i.max_age_secs
        })
        .max_by_key(|o| o.observed_at);
    let Some(o) = fresh else {
        return FenceView::waiting(
            format!(
                "no healthy server answered the storage-lease probe in the last {} s",
                i.max_age_secs
            ),
            Some(
                "Check the probe (Servers → Members shows its last reading); or confirm the server is off",
            ),
        );
    };
    let host = o
        .lockspace
        .hosts
        .iter()
        .find(|h| h.host_id == known.host_id);
    let mut view = match host {
        None => FenceView::waiting(
            format!(
                "host {} is not listed in lockspace {}",
                known.host_id, known.lockspace
            ),
            Some(CONFIRM),
        ),
        Some(h) if h.generation != known.generation => FenceView::waiting(
            format!(
                "the lease of host {} has generation {}, not {} as while the server was online: it may have joined again",
                h.host_id, h.generation, known.generation
            ),
            Some(CONFIRM),
        ),
        Some(h) => match h.state {
            FenceLeaseState::Dead => FenceView {
                condition: FenceCondition::Fenced,
                reason: format!(
                    "its storage lease expired: {} reports host {} DEAD in {}",
                    o.observed_by, h.host_id, known.lockspace
                ),
                next_step: None,
                evidence: Some(FenceEvidence {
                    kind: FenceKind::StorageLease,
                    at: o.observed_at,
                    observed_by: Some(o.observed_by.clone()),
                    lockspace: Some(known.lockspace.clone()),
                    host_id: Some(h.host_id),
                    generation: Some(h.generation),
                    confirmed_by: None,
                    provider: None,
                    summary: format!(
                        "sanlock on {} reported host {} (generation {}) of {} DEAD in {}: its lease was not renewed for {HOST_DEAD_SECS} s, so it is off or its watchdog reset it",
                        o.observed_by, h.host_id, h.generation, i.member, known.lockspace
                    ),
                }),
                lease_state: None,
                observed_by: None,
                observed_at: None,
            },
            FenceLeaseState::Live => FenceView::waiting(
                "its storage lease is still being renewed (LIVE): the server may still be running and writing to the SAN",
                Some(
                    "If it is cut off from the other servers but not from the SAN, it keeps running its modules. Check it; switch it off and confirm if needed",
                ),
            ),
            FenceLeaseState::Fail => FenceView::waiting(
                format!(
                    "its storage lease stopped being renewed (FAIL); sanlock declares it dead {HOST_DEAD_SECS} s after the last renewal, once its watchdog has reset it"
                ),
                Some("Nothing to do: Apelora restarts its modules once the lease is dead"),
            ),
            FenceLeaseState::Free => FenceView::waiting(
                "it released its storage lease (FREE), which does not prove it is off",
                Some(CONFIRM),
            ),
            FenceLeaseState::Unknown => {
                FenceView::waiting("sanlock does not know its lease state yet", Some(CONFIRM))
            }
        },
    };
    view.lease_state = host.map(|h| h.state);
    view.observed_by = Some(o.observed_by.clone());
    view.observed_at = Some(o.observed_at);
    view
}

/// When Incus last heard from a member, from its message ("No heartbeat for 1m2s (2026-10-21
/// 14:02:03 +0000)"), as Unix seconds; `None` when the message has no such time.
pub fn last_heartbeat(message: &str) -> Option<i64> {
    let inner = message.rsplit_once('(')?.1.split_once(')')?.0;
    let mut it = inner.split_whitespace();
    let (date, clock, offset) = (it.next()?, it.next()?, it.next()?);
    let d: Vec<i32> = date
        .split('-')
        .map(|v| v.parse().ok())
        .collect::<Option<_>>()?;
    let c: Vec<u8> = clock
        .split(':')
        .map(|v| v.parse().ok())
        .collect::<Option<_>>()?;
    let ([y, mo, da], [h, mi, s]) = (d.as_slice(), c.as_slice()) else {
        return None;
    };
    let month = time::Month::try_from(u8::try_from(*mo).ok()?).ok()?;
    let date = time::Date::from_calendar_date(*y, month, u8::try_from(*da).ok()?).ok()?;
    let t = time::Time::from_hms(*h, *mi, *s).ok()?;
    let (sign, digits) = offset.split_at(1);
    if digits.len() != 4 {
        return None;
    }
    let oh: i8 = digits[..2].parse().ok()?;
    let om: i8 = digits[2..].parse().ok()?;
    let (oh, om) = match sign {
        "+" => (oh, om),
        "-" => (-oh, -om),
        _ => return None,
    };
    let off = time::UtcOffset::from_hms(oh, om, 0).ok()?;
    Some(
        time::PrimitiveDateTime::new(date, t)
            .assume_offset(off)
            .unix_timestamp(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// As read on the lab's member 2 (sanlock 3.9.5), REAL INCUS TESTED 2026-10-21.
    const GETS: &str = "s lvm_apelora-shared:2:/dev/mapper/apelora--shared-lvmlock:0 \n\
        h 1 gen 1 timestamp 97252 LIVE\n\
        h 2 gen 1 timestamp 40074 LIVE\n\
        h 3 gen 1 timestamp 73435 DEAD\n\
        h 4 gen 2 timestamp 73443 FAIL\n";

    fn obs(by: &str, at: i64) -> FenceObservation {
        FenceObservation {
            observed_by: by.into(),
            observed_at: at,
            lockspace: parse_gets(GETS).expect("parse").remove(0),
        }
    }

    fn known(id: u32, generation: u64) -> KnownHost {
        KnownHost {
            member: format!("m{id}"),
            lockspace: "lvm_apelora-shared".into(),
            host_id: id,
            generation,
        }
    }

    fn input<'a>(
        member: &'a str,
        known: Option<&'a KnownHost>,
        observations: &'a [FenceObservation],
    ) -> FenceInput<'a> {
        FenceInput {
            member,
            offline: true,
            known,
            observations,
            confirmed: None,
            power_off: None,
            probe: true,
            now: 1_000,
            max_age_secs: 60,
        }
    }

    #[test]
    fn sanlock_output_is_read_and_malformed_lines_refuse_the_reading() {
        let spaces = parse_gets(GETS).expect("parse");
        assert_eq!(spaces.len(), 1);
        assert_eq!(spaces[0].name, "lvm_apelora-shared");
        assert_eq!(spaces[0].local_host_id, 2);
        assert_eq!(spaces[0].hosts.len(), 4);
        assert_eq!(spaces[0].hosts[2].state, FenceLeaseState::Dead);
        assert_eq!(spaces[0].hosts[3].generation, 2);
        assert!(
            parse_gets("h 1 gen 1 timestamp 1 LIVE").is_err(),
            "no lockspace"
        );
        assert!(parse_gets("s lvm_x:1:/dev/x:0\nh one gen 1 timestamp 1 LIVE").is_err());
        assert!(parse_gets("s :1:/dev/x:0").is_err());
        assert_eq!(parse_gets("").expect("empty"), vec![]);
        assert_eq!(
            pick_lockspace(&spaces, None).map(|s| s.name.as_str()),
            Ok("lvm_apelora-shared")
        );
        assert!(pick_lockspace(&spaces, Some("lvm_other")).is_err());
        let two = [
            spaces[0].clone(),
            FenceLockspace {
                name: "lvm_b".into(),
                ..spaces[0].clone()
            },
        ];
        assert!(
            pick_lockspace(&two, None)
                .expect_err("two")
                .contains("APELORA_FENCE_LOCKSPACE")
        );
    }

    #[test]
    fn a_member_learns_its_own_live_lease_only() {
        let o = obs("m2", 1_000);
        assert_eq!(
            learn("m2", &o),
            Some(known(2, 1)).map(|k| KnownHost {
                member: "m2".into(),
                ..k
            })
        );
        let mut dead = o.clone();
        dead.lockspace.local_host_id = 3;
        assert_eq!(learn("m3", &dead), None, "its own lease is not LIVE");
    }

    #[test]
    fn only_a_fresh_dead_lease_of_the_same_generation_fences() {
        let k3 = known(3, 1);
        let readings = [obs("m2", 990)];
        let v = decide(&input("m3", Some(&k3), &readings));
        assert!(v.fenced(), "{v:?}");
        let e = v.evidence.expect("evidence");
        assert_eq!(
            (e.kind, e.host_id, e.observed_by.as_deref()),
            (FenceKind::StorageLease, Some(3), Some("m2"))
        );
        assert_eq!(v.lease_state, Some(FenceLeaseState::Dead));

        // Another generation: the host joined again (or the mapping is wrong).
        let k3b = known(3, 2);
        assert!(
            decide(&input("m3", Some(&k3b), &readings))
                .reason
                .contains("generation")
        );
        // Stale, or read by the failed member itself.
        let stale = [obs("m2", 900)];
        assert!(!decide(&input("m3", Some(&k3), &stale)).fenced());
        let own = [obs("m3", 999)];
        assert!(!decide(&input("m3", Some(&k3), &own)).fenced());
        // Still LIVE or only FAIL: waiting, with the reason.
        let k1 = known(1, 1);
        let live = decide(&input("m1", Some(&k1), &readings));
        assert!(!live.fenced() && live.reason.contains("LIVE"), "{live:?}");
        let k4 = known(4, 2);
        let fail = decide(&input("m4", Some(&k4), &readings));
        assert!(!fail.fenced() && fail.reason.contains("FAIL"));
        // Never learned, or no probe: only a person can fence it.
        assert!(
            decide(&input("m3", None, &readings))
                .reason
                .contains("never read")
        );
        let mut no_probe = input("m3", Some(&k3), &readings);
        no_probe.probe = false;
        assert!(!decide(&no_probe).fenced());
        no_probe.confirmed = Some(("user:ana", 995));
        let c = decide(&no_probe);
        assert!(c.fenced());
        assert_eq!(c.evidence.map(|e| e.kind), Some(FenceKind::Confirmed));
        // Not offline: never fenced, even when confirmed.
        no_probe.offline = false;
        assert!(!decide(&no_probe).fenced());
    }

    #[test]
    fn the_last_heartbeat_comes_from_incus_message() {
        assert_eq!(
            last_heartbeat("No heartbeat for 1m2s (2026-10-21 14:02:03 +0000)"),
            Some(1_792_591_323)
        );
        assert_eq!(
            last_heartbeat("No heartbeat for 3s (2026-10-21 16:02:03 +0200)"),
            Some(1_792_591_323)
        );
        assert_eq!(last_heartbeat("Fully operational"), None);
        assert_eq!(last_heartbeat("No heartbeat (garbage)"), None);
    }

    fn cluster(statuses: &[(&str, &str)]) -> Vec<crate::ClusterMember> {
        statuses
            .iter()
            .map(|(name, status)| crate::ClusterMember {
                name: (*name).into(),
                url: format!("https://{name}:8443"),
                roles: vec![],
                status: (*status).into(),
                message: String::new(),
                architecture: "aarch64".into(),
                failure_domain: "default".into(),
                database: true,
                groups: vec![],
                scheduler: "all".into(),
            })
            .collect()
    }

    fn ctx<'a>(members: &'a [crate::ClusterMember], member: &'a str) -> PowerOffContext<'a> {
        PowerOffContext {
            member,
            members,
            offline_for_secs: 45,
            after_secs: 30,
            others_powered_off: 0,
            last_power_off_ago: None,
        }
    }

    #[test]
    fn a_power_state_read_back_as_off_fences_an_offline_member_whatever_the_lease_says() {
        let reading = PowerOffReading {
            provider: "redfish".into(),
            system: "/redfish/v1/Systems/1".into(),
            at: 990,
            switched_off: true,
        };
        // The lease is still LIVE (a server cut off from the others but not from the SAN):
        // the power state is what decides.
        let live = [obs("m2", 995)];
        let host = known(1, 1);
        let mut i = input("m1", Some(&host), &live);
        i.power_off = Some(&reading);
        let v = decide(&i);
        assert!(v.fenced(), "{v:?}");
        let e = v.evidence.expect("evidence");
        assert_eq!(e.kind, FenceKind::PowerOff);
        assert_eq!(e.provider.as_deref(), Some("redfish"));
        assert!(
            e.summary.contains("after switching it off"),
            "{}",
            e.summary
        );
        // Never for a member Incus reports Online.
        i.offline = false;
        assert!(!decide(&i).fenced());
        // A person's confirmation still comes first.
        i.offline = true;
        i.confirmed = Some(("ada", 900));
        assert_eq!(
            decide(&i).evidence.map(|e| e.kind),
            Some(FenceKind::Confirmed)
        );
    }

    #[test]
    fn switching_a_member_off_needs_every_guard() {
        let members = cluster(&[
            ("m1", "Offline"),
            ("m2", "Online"),
            ("m3", "Online"),
            ("m4", "Online"),
        ]);
        assert_eq!(power_off_refusal(&ctx(&members, "m1")), None);

        // Not Offline, or not listed at all.
        assert!(
            power_off_refusal(&ctx(&members, "m2"))
                .expect("online")
                .contains("not Offline")
        );
        assert!(
            power_off_refusal(&ctx(&members, "m9"))
                .expect("unknown")
                .contains("not a member")
        );
        // Offline for too short a time; the floor is 20 s whatever is configured.
        let mut c = ctx(&members, "m1");
        c.offline_for_secs = 25;
        assert!(power_off_refusal(&c).expect("short").contains("waits 30 s"));
        c.after_secs = 1;
        c.offline_for_secs = 19;
        assert!(power_off_refusal(&c).expect("floor").contains("waits 20 s"));
        c.offline_for_secs = 20;
        assert_eq!(power_off_refusal(&c), None);
        // Half or more of the members Offline points at the control plane's own network.
        let split = cluster(&[
            ("m1", "Offline"),
            ("m2", "Offline"),
            ("m3", "Online"),
            ("m4", "Online"),
        ]);
        assert!(
            power_off_refusal(&ctx(&split, "m1"))
                .expect("half")
                .contains("2 of 4")
        );
        let one = cluster(&[("m1", "Offline")]);
        assert!(
            power_off_refusal(&ctx(&one, "m1")).is_some(),
            "a one-member cluster never fences itself away"
        );
        // One at a time.
        let mut c = ctx(&members, "m1");
        c.others_powered_off = 1;
        assert!(
            power_off_refusal(&c)
                .expect("second")
                .contains("needs a person")
        );
        // Not again within the cooldown, again after it.
        c.others_powered_off = 0;
        c.last_power_off_ago = Some(60);
        assert!(
            power_off_refusal(&c)
                .expect("cooldown")
                .contains("60 s ago")
        );
        c.last_power_off_ago = Some(POWER_OFF_COOLDOWN_SECS);
        assert_eq!(power_off_refusal(&c), None);
    }
}

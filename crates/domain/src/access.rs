// SPDX-License-Identifier: AGPL-3.0-only
//! Interactive access to a running appliance (terminal, PowerShell, remote desktop): which
//! kinds an appliance offers and whether the instance can take a session right now. Pure; the
//! control plane adds the caller's permissions, the approval rule and the session limits.

use serde::{Deserialize, Serialize};

/// The operating system inside an appliance, from its catalog class (never guessed from names).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum GuestOs {
    #[default]
    Linux,
    Windows,
}

/// What a person can open on an appliance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum SessionKind {
    /// A shell in a Linux appliance, through the Incus exec channel.
    Terminal,
    /// PowerShell in a Windows appliance, through the Incus exec channel (needs the guest agent).
    Powershell,
    /// A remote desktop, through the control plane's RDP gateway.
    Rdp,
}

impl SessionKind {
    pub const ALL: [SessionKind; 3] = [
        SessionKind::Terminal,
        SessionKind::Powershell,
        SessionKind::Rdp,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            SessionKind::Terminal => "terminal",
            SessionKind::Powershell => "powershell",
            SessionKind::Rdp => "rdp",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }

    /// The permission that opens this kind of session.
    pub fn permission(self) -> &'static str {
        match self {
            SessionKind::Terminal | SessionKind::Powershell => "instance.console",
            SessionKind::Rdp => "instance.desktop",
        }
    }

    /// The kinds an appliance with this guest operating system offers.
    pub fn offered_for(os: GuestOs) -> &'static [SessionKind] {
        match os {
            GuestOs::Linux => &[SessionKind::Terminal],
            GuestOs::Windows => &[SessionKind::Powershell, SessionKind::Rdp],
        }
    }

    /// The program an exec session starts (Linux: a POSIX shell; Windows: Windows PowerShell).
    pub fn command(self) -> Option<&'static [&'static str]> {
        match self {
            SessionKind::Terminal => Some(&["/bin/sh"]),
            SessionKind::Powershell => Some(&["powershell.exe", "-NoLogo"]),
            SessionKind::Rdp => None,
        }
    }
}

/// What the control plane could establish about the instance and its own setup.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    /// Incus could be read just now; without that nothing about the instance is known.
    pub observed: bool,
    /// The instance exists, carries Apelora's marker and belongs to this application.
    pub found: bool,
    /// Incus status, e.g. `Running`.
    pub status: String,
    /// The instance's cluster member cannot be reached.
    pub unreachable: bool,
    /// The control plane has an RDP gateway configured.
    pub rdp_gateway: bool,
    /// The operator has turned PowerShell sessions on. They are off by default because they
    /// have not been verified against a Windows guest.
    pub powershell: bool,
}

/// `Ok` when a session of this kind can be opened on the instance now, else why not.
pub fn availability(kind: SessionKind, facts: &Facts) -> Result<(), String> {
    if !facts.observed {
        return Err("Incus cannot be read, so the instance's state is unknown".into());
    }
    if !facts.found {
        return Err("this instance does not exist or is not managed by Apelora".into());
    }
    if facts.unreachable {
        return Err("its cluster member cannot be reached".into());
    }
    if facts.status != "Running" {
        return Err(format!(
            "the instance is {}; start the application first",
            facts.status.to_lowercase()
        ));
    }
    if kind == SessionKind::Powershell && !facts.powershell {
        return Err(
            "PowerShell sessions are not turned on: they have not been verified against a Windows guest"
                .into(),
        );
    }
    if kind == SessionKind::Rdp && !facts.rdp_gateway {
        return Err("the remote desktop gateway is not set up on this control plane".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running() -> Facts {
        Facts {
            observed: true,
            found: true,
            status: "Running".into(),
            unreachable: false,
            rdp_gateway: false,
            powershell: false,
        }
    }

    #[test]
    fn each_operating_system_offers_its_own_kinds() {
        assert_eq!(
            SessionKind::offered_for(GuestOs::Linux),
            &[SessionKind::Terminal]
        );
        assert_eq!(
            SessionKind::offered_for(GuestOs::Windows),
            &[SessionKind::Powershell, SessionKind::Rdp]
        );
    }

    #[test]
    fn kinds_map_to_permissions_and_round_trip() {
        for k in SessionKind::ALL {
            assert_eq!(SessionKind::parse(k.as_str()), Some(k));
        }
        assert_eq!(SessionKind::Terminal.permission(), "instance.console");
        assert_eq!(SessionKind::Powershell.permission(), "instance.console");
        assert_eq!(SessionKind::Rdp.permission(), "instance.desktop");
        assert!(SessionKind::Rdp.command().is_none());
        assert_eq!(SessionKind::Terminal.command(), Some(&["/bin/sh"][..]));
        assert_eq!(
            SessionKind::Powershell.command(),
            Some(&["powershell.exe", "-NoLogo"][..])
        );
        assert_eq!(SessionKind::parse("ssh"), None);
    }

    #[test]
    fn only_a_running_observed_instance_takes_a_session() {
        assert_eq!(availability(SessionKind::Terminal, &running()), Ok(()));
        let mut f = running();
        f.observed = false;
        assert!(
            availability(SessionKind::Terminal, &f)
                .expect_err("refused")
                .contains("unknown")
        );
        let mut f = running();
        f.found = false;
        assert!(availability(SessionKind::Terminal, &f).is_err());
        let mut f = running();
        f.unreachable = true;
        assert!(
            availability(SessionKind::Terminal, &f)
                .expect_err("refused")
                .contains("member")
        );
        let mut f = running();
        f.status = "Stopped".into();
        assert!(
            availability(SessionKind::Terminal, &f)
                .expect_err("refused")
                .contains("stopped")
        );
    }

    #[test]
    fn powershell_is_off_until_the_operator_turns_it_on() {
        let off = running();
        assert!(
            availability(SessionKind::Powershell, &off)
                .expect_err("refused")
                .contains("not turned on")
        );
        let mut on = running();
        on.powershell = true;
        assert_eq!(availability(SessionKind::Powershell, &on), Ok(()));
        assert_eq!(
            availability(SessionKind::Terminal, &off),
            Ok(()),
            "the terminal is not affected"
        );
    }

    #[test]
    fn remote_desktop_needs_the_gateway() {
        assert!(
            availability(SessionKind::Rdp, &running())
                .expect_err("refused")
                .contains("gateway")
        );
        let mut f = running();
        f.rdp_gateway = true;
        assert_eq!(availability(SessionKind::Rdp, &f), Ok(()));
    }
}

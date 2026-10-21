// SPDX-License-Identifier: AGPL-3.0-only
//! Application-level readiness: a command that runs inside a running instance and exits 0 when
//! the application there is ready (for example `pg_isready`, or a request to its own health
//! address). The load balancer's check only proves a port accepts connections; an application
//! that hangs behind an open port passes it. Pure: validation of what a module may declare, and
//! how a command's result combines with the port check. The worker runs the command.
//!
//! The command comes from the module class. It is an argument list, never a shell line, and runs
//! as an unprivileged user with a short time limit; only its exit status is read, never its output.

use serde::{Deserialize, Serialize};

use crate::health::Probe;

pub const MAX_ARGS: usize = 8;
pub const MAX_ARG_LEN: usize = 200;
pub const MAX_TIMEOUT_SECS: u8 = 15;
const DEFAULT_TIMEOUT_SECS: u8 = 5;

/// Programs that would turn an argument list into a script. A class that needs one should name
/// the real program it runs instead.
const SHELLS: &[&str] = &[
    "sh", "bash", "dash", "ash", "zsh", "ksh", "csh", "tcsh", "fish", "busybox",
];

fn default_timeout() -> u8 {
    DEFAULT_TIMEOUT_SECS
}

/// What to run inside the instance and how long it may take.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct Readiness {
    /// Program and arguments, as an argument list (no shell). Exit status 0 means ready.
    pub command: Vec<String>,
    /// Longer than this counts as failing (1–15, default 5).
    #[serde(default = "default_timeout")]
    pub timeout_secs: u8,
}

impl Readiness {
    pub fn validate(&self, module: &str) -> Result<(), String> {
        let bad = |why: String| Err(format!("{module}: readiness command {why}"));
        if self.command.is_empty() || self.command.len() > MAX_ARGS {
            return bad(format!("needs 1 to {MAX_ARGS} arguments"));
        }
        if !(1..=MAX_TIMEOUT_SECS).contains(&self.timeout_secs) {
            return bad(format!("timeout is 1 to {MAX_TIMEOUT_SECS} seconds"));
        }
        for a in &self.command {
            if a.is_empty() || a.len() > MAX_ARG_LEN || a.chars().any(char::is_control) {
                return bad(format!(
                    "has an empty, over-long or control-character argument (at most {MAX_ARG_LEN} characters)"
                ));
            }
        }
        let program = &self.command[0];
        if !program
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'/' | b'-' | b'+'))
        {
            return bad(
                "must start with a program name or path (letters, digits and _ . / - +)".into(),
            );
        }
        let name = program.rsplit('/').next().unwrap_or(program);
        if SHELLS.contains(&name) {
            return bad(format!(
                "may not run a shell ({name}): name the program itself"
            ));
        }
        Ok(())
    }
}

/// What a command's end says: exit status 0 passes, any other status or a timeout fails, and not
/// knowing (the command could not be run at all) says nothing.
pub fn probe_of_run(exit: Option<i32>, timed_out: bool) -> Probe {
    if timed_out {
        return Probe::Failing;
    }
    match exit {
        Some(0) => Probe::Passing,
        Some(_) => Probe::Failing,
        None => Probe::Unknown,
    }
}

/// One verdict from the port check and the readiness command: any failure fails the instance,
/// otherwise any pass passes it (an instance without a service address has only the command), and
/// with no evidence at all nothing is decided.
pub fn combine(port: Probe, application: Option<Probe>) -> Probe {
    let opinions = [Some(port), application];
    let seen = opinions.iter().flatten();
    if seen.clone().any(|p| *p == Probe::Failing) {
        Probe::Failing
    } else if seen.clone().any(|p| *p == Probe::Passing) {
        Probe::Passing
    } else {
        Probe::Unknown
    }
}

/// Words for the health detail, naming what failed.
pub fn detail(port: Probe, application: Option<Probe>) -> &'static str {
    let word = |p: Probe| match p {
        Probe::Passing => 0,
        Probe::Failing => 1,
        Probe::Unknown => 2,
    };
    match (application, word(port)) {
        (None, 0) => "port check passing",
        (None, 1) => "port check failing",
        (None, _) => "port check not reporting",
        (Some(app), p) => match (p, word(app)) {
            (1, 1) => "port check and readiness command failing",
            (1, _) => "port check failing",
            (_, 1) => "readiness command failing",
            (0, 0) | (0, 2) => "port check and readiness command passing",
            (2, 0) => "readiness command passing",
            _ => "port check and readiness command not reporting",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Probe::{Failing, Passing, Unknown};

    fn r(cmd: &[&str]) -> Readiness {
        Readiness {
            command: cmd.iter().map(|s| (*s).to_owned()).collect(),
            timeout_secs: 5,
        }
    }

    #[test]
    fn a_command_is_a_program_and_arguments_with_limits() {
        assert!(r(&["pg_isready", "-q"]).validate("db").is_ok());
        assert!(
            r(&["/usr/bin/curl", "-fsS", "http://127.0.0.1/health"])
                .validate("web")
                .is_ok()
        );
        assert!(r(&[]).validate("m").is_err());
        assert!(r(&["x"; MAX_ARGS]).validate("m").is_ok());
        assert!(r(&["x"; MAX_ARGS + 1]).validate("m").is_err());
        assert!(r(&["curl", ""]).validate("m").is_err());
        assert!(r(&["curl", &"a".repeat(MAX_ARG_LEN)]).validate("m").is_ok());
        assert!(
            r(&["curl", &"a".repeat(MAX_ARG_LEN + 1)])
                .validate("m")
                .is_err()
        );
        assert!(r(&["curl", "a\nb"]).validate("m").is_err());
        let err = r(&[]).validate("db").expect_err("refused");
        assert!(err.starts_with("db: readiness command"), "{err}");
    }

    #[test]
    fn the_program_is_a_name_and_never_a_shell() {
        for bad in ["curl; reboot", "$(id)", "a b", "`x`", "a|b", "", "../x y"] {
            assert!(r(&[bad]).validate("m").is_err(), "{bad:?}");
        }
        for shell in [
            "sh",
            "bash",
            "/bin/sh",
            "/usr/bin/bash",
            "dash",
            "zsh",
            "busybox",
        ] {
            let e = r(&[shell, "-c", "true"])
                .validate("m")
                .expect_err("refused");
            assert!(e.contains("shell"), "{shell}: {e}");
        }
        assert!(r(&["/usr/bin/pg_isready"]).validate("m").is_ok());
        assert!(
            r(&["shasum"]).validate("m").is_ok(),
            "only exact shell names are refused"
        );
    }

    #[test]
    fn the_time_limit_is_one_to_thirty_seconds() {
        let with = |t| Readiness {
            timeout_secs: t,
            ..r(&["pg_isready"])
        };
        assert!(with(0).validate("m").is_err());
        assert!(with(1).validate("m").is_ok());
        assert!(with(15).validate("m").is_ok());
        assert!(with(16).validate("m").is_err());
        let parsed: Readiness =
            serde_json::from_str(r#"{"command":["pg_isready"]}"#).expect("json");
        assert_eq!(parsed.timeout_secs, 5, "default");
    }

    #[test]
    fn an_exit_status_decides_and_not_running_says_nothing() {
        assert_eq!(probe_of_run(Some(0), false), Passing);
        assert_eq!(probe_of_run(Some(1), false), Failing);
        assert_eq!(probe_of_run(Some(127), false), Failing, "command not found");
        assert_eq!(
            probe_of_run(Some(0), true),
            Failing,
            "too slow, whatever it said"
        );
        assert_eq!(probe_of_run(None, false), Unknown);
        assert_eq!(probe_of_run(None, true), Failing);
    }

    #[test]
    fn the_port_check_and_the_command_combine_so_either_failure_fails() {
        assert_eq!(combine(Passing, None), Passing);
        assert_eq!(combine(Failing, None), Failing);
        assert_eq!(combine(Unknown, None), Unknown);
        assert_eq!(combine(Passing, Some(Passing)), Passing);
        assert_eq!(
            combine(Passing, Some(Failing)),
            Failing,
            "open port, hung application"
        );
        assert_eq!(combine(Failing, Some(Passing)), Failing);
        assert_eq!(combine(Failing, Some(Failing)), Failing);
        assert_eq!(
            combine(Unknown, Some(Passing)),
            Passing,
            "no service address: the command alone"
        );
        assert_eq!(combine(Unknown, Some(Failing)), Failing);
        assert_eq!(
            combine(Passing, Some(Unknown)),
            Passing,
            "the command could not run: the port check stands"
        );
        assert_eq!(combine(Unknown, Some(Unknown)), Unknown);
    }

    #[test]
    fn the_detail_names_what_failed() {
        assert_eq!(detail(Passing, None), "port check passing");
        assert_eq!(detail(Failing, None), "port check failing");
        assert_eq!(detail(Unknown, None), "port check not reporting");
        assert_eq!(detail(Passing, Some(Failing)), "readiness command failing");
        assert_eq!(detail(Failing, Some(Passing)), "port check failing");
        assert_eq!(
            detail(Failing, Some(Failing)),
            "port check and readiness command failing"
        );
        assert_eq!(
            detail(Passing, Some(Passing)),
            "port check and readiness command passing"
        );
        assert_eq!(detail(Unknown, Some(Passing)), "readiness command passing");
        assert_eq!(
            detail(Unknown, Some(Unknown)),
            "port check and readiness command not reporting"
        );
    }
}

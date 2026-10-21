// SPDX-License-Identifier: AGPL-3.0-only
//! The standard process families (`process_*`), read when the exposition is rendered. On Linux
//! they come from `/proc/self`; elsewhere (development on macOS) only the start time is known.

use std::fmt::Write as _;
use std::sync::OnceLock;

static STARTED: OnceLock<f64> = OnceLock::new();

/// Records the start time (the first call wins).
pub fn start() {
    let _ = STARTED.set(crate::now_secs());
}

struct Sample {
    name: &'static str,
    help: &'static str,
    kind: &'static str,
    value: f64,
}

fn samples() -> Vec<Sample> {
    let mut out = vec![Sample {
        name: "process_start_time_seconds",
        help: "Start time of the process since the Unix epoch in seconds.",
        kind: "gauge",
        value: *STARTED.get_or_init(crate::now_secs),
    }];
    out.extend(linux::samples());
    out
}

pub(crate) fn render(out: &mut String, constant: &[(String, String)], openmetrics: bool) {
    let labels = if constant.is_empty() {
        String::new()
    } else {
        let inner: Vec<String> = constant
            .iter()
            .map(|(k, v)| format!("{k}=\"{v}\""))
            .collect();
        format!("{{{}}}", inner.join(","))
    };
    for s in samples() {
        // Counters are named with `_total`; OpenMetrics names the family without it.
        let family = if openmetrics && s.kind == "counter" {
            s.name.trim_end_matches("_total")
        } else {
            s.name
        };
        let _ = writeln!(out, "# HELP {family} {}", s.help);
        let _ = writeln!(out, "# TYPE {family} {}", s.kind);
        let _ = writeln!(
            out,
            "{}{labels} {}",
            s.name,
            crate::registry::number(s.value)
        );
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::Sample;

    /// Clock ticks per second as seen by user space (`USER_HZ`), fixed at 100 by the Linux ABI.
    const USER_HZ: f64 = 100.0;

    pub(super) fn samples() -> Vec<Sample> {
        let mut out = Vec::new();
        if let Ok(stat) = std::fs::read_to_string("/proc/self/stat")
            // Fields after the command name, which is in parentheses and may contain spaces.
            && let Some((_, rest)) = stat.rsplit_once(')')
        {
            let f: Vec<&str> = rest.split_whitespace().collect();
            // rest starts at field 3 (state): utime is field 14, stime field 15.
            let field = |n: usize| f.get(n - 3).and_then(|v| v.parse::<f64>().ok());
            if let (Some(u), Some(s)) = (field(14), field(15)) {
                out.push(Sample {
                    name: "process_cpu_seconds_total",
                    help: "Total user and system CPU time spent in seconds.",
                    kind: "counter",
                    value: (u + s) / USER_HZ,
                });
            }
        }
        if let Ok(status) = std::fs::read_to_string("/proc/self/status")
            && let Some(kb) = status
                .lines()
                .find_map(|l| l.strip_prefix("VmRSS:"))
                .and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<f64>().ok())
        {
            out.push(Sample {
                name: "process_resident_memory_bytes",
                help: "Resident memory size in bytes.",
                kind: "gauge",
                value: kb * 1024.0,
            });
        }
        if let Ok(dir) = std::fs::read_dir("/proc/self/fd") {
            out.push(Sample {
                name: "process_open_fds",
                help: "Number of open file descriptors.",
                kind: "gauge",
                value: dir.count() as f64,
            });
        }
        if let Ok(limits) = std::fs::read_to_string("/proc/self/limits")
            && let Some(max) = limits
                .lines()
                .find(|l| l.starts_with("Max open files"))
                .and_then(|l| l.split_whitespace().nth(3))
                .and_then(|v| v.parse::<f64>().ok())
        {
            out.push(Sample {
                name: "process_max_fds",
                help: "Maximum number of open file descriptors.",
                kind: "gauge",
                value: max,
            });
        }
        out
    }
}

#[cfg(not(target_os = "linux"))]
mod linux {
    pub(super) fn samples() -> Vec<super::Sample> {
        Vec::new()
    }
}

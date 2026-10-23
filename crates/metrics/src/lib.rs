// SPDX-License-Identifier: AGPL-3.0-only
//! Apelora's own metrics: one registry per process, every family declared in this file, and
//! the text exposition format a collector inside the installation scrapes from `GET /metrics`
//! (control plane and worker). Design, label rules and what each family means:
//! `docs/architecture/MONITORING.md`.
//!
//! Rules that keep this cheap and safe at every size (ADR 0011):
//! - instrument at choke points only (request middleware, job runner, Incus HTTP client,
//!   supervisor loop, scrape-time readers), never inside business logic;
//! - label values are route templates, outcome classes and catalogue ids — never a raw path,
//!   a name, an id, an email address, a token or an error text;
//! - every series carries the constant `region`, `site` and `sector` labels, so summaries roll
//!   up per sector, site and region without relabelling.

pub mod process;
pub mod registry;

use std::sync::OnceLock;

pub use registry::{Counter, Family, Gauge, Histogram, MAX_SERIES, OVERFLOW};

/// `Content-Type` of the classic text format (the default answer).
pub const TEXT_CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";
/// `Content-Type` of the OpenMetrics text format (answered when the collector asks for it).
pub const OPENMETRICS_CONTENT_TYPE: &str =
    "application/openmetrics-text; version=1.0.0; charset=utf-8";

/// Where the worker's metrics listener binds unless `APELORA_WORKER_METRICS_ADDR` says otherwise.
pub const WORKER_METRICS_DEFAULT_ADDR: &str = "127.0.0.1:9471";

/// Should the answer use the OpenMetrics variant? Chosen from the request's `Accept` header.
pub fn wants_openmetrics(accept: Option<&str>) -> bool {
    accept.is_some_and(|a| a.contains("application/openmetrics-text"))
}

// ---- where this process runs ---------------------------------------------------------------

/// Where the process runs in the fabric (ADR 0009): constant labels on every series.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub region: String,
    pub site: String,
    pub sector: String,
}

/// The value used when a level is not configured.
pub const DEFAULT_PLACE: &str = "default";

impl Default for Place {
    fn default() -> Self {
        Self {
            region: DEFAULT_PLACE.into(),
            site: DEFAULT_PLACE.into(),
            sector: DEFAULT_PLACE.into(),
        }
    }
}

/// A region, site or sector name: lowercase letters, digits and `-`, 1–63 characters, starting
/// and ending with a letter or digit (the same shape as a DNS label, so it also fits hostnames).
pub fn valid_place_name(v: &str) -> bool {
    let b = v.as_bytes();
    !b.is_empty()
        && b.len() <= 63
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
        && b[0] != b'-'
        && b[b.len() - 1] != b'-'
}

impl Place {
    /// Reads `APELORA_REGION`, `APELORA_SITE` and `APELORA_SECTOR`; unset or empty means
    /// [`DEFAULT_PLACE`]. A value of the wrong shape is a configuration error.
    pub fn from_env(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let one = |key: &str| -> Result<String, String> {
            match get(key)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
            {
                None => Ok(DEFAULT_PLACE.to_owned()),
                Some(v) if valid_place_name(&v) => Ok(v),
                Some(v) => Err(format!(
                    "{key}={v:?}: use 1-63 lowercase letters, digits and '-', starting and ending with a letter or digit"
                )),
            }
        };
        Ok(Self {
            region: one("APELORA_REGION")?,
            site: one("APELORA_SITE")?,
            sector: one("APELORA_SECTOR")?,
        })
    }
}

static PLACE: OnceLock<Place> = OnceLock::new();

/// Sets the constant labels once at start-up; later calls are ignored (tests share a process).
pub fn set_place(place: Place) {
    let _ = PLACE.set(place);
}

pub fn place() -> Place {
    PLACE.get().cloned().unwrap_or_default()
}

/// Records which binary this is and its build (`apelora_build_info`), and the start time.
pub fn set_build(component: &'static str, version: &'static str) {
    let commit = option_env!("APELORA_BUILD_COMMIT").unwrap_or("unknown");
    BUILD_INFO.set(&[component, version, commit], 1.0);
    process::start();
}

/// Records whether this process talks to a real Incus or to the mock (`mock` or `incus`), so
/// numbers from the mock are never mistaken for a real cluster.
pub fn set_provider(mode: &str) {
    PROVIDER_INFO.replace([(vec![mode], 1.0)]);
}

// ---- small helpers for label values --------------------------------------------------------

/// `2xx`, `3xx`, `4xx` or `5xx`.
pub fn status_class(status: u16) -> &'static str {
    match status {
        100..=199 => "1xx",
        200..=299 => "2xx",
        300..=399 => "3xx",
        400..=499 => "4xx",
        _ => "5xx",
    }
}

/// HTTP methods Apelora serves or sends; anything else is `other`.
pub fn method_label(m: &str) -> &'static str {
    match m {
        "GET" => "GET",
        "POST" => "POST",
        "PUT" => "PUT",
        "PATCH" => "PATCH",
        "DELETE" => "DELETE",
        "HEAD" => "HEAD",
        "OPTIONS" => "OPTIONS",
        _ => "other",
    }
}

/// A job kind as a label: kinds are names from code (`instance.create`), but the column is free
/// text, so anything that does not look like one becomes `other`.
pub fn job_kind_label(kind: &str) -> &str {
    if !kind.is_empty()
        && kind.len() <= 40
        && kind
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c == b'.' || c == b'_')
    {
        kind
    } else {
        "other"
    }
}

// ---- buckets ---------------------------------------------------------------------------------

const HTTP_BUCKETS: &[f64] = &[
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];
const INCUS_BUCKETS: &[f64] = &[
    0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 20.0, 30.0,
];
const LONG_BUCKETS: &[f64] = &[
    0.1, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0, 600.0,
];
const ACQUIRE_BUCKETS: &[f64] = &[
    0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0,
];

// ---- API (control plane) --------------------------------------------------------------------

pub static HTTP_REQUESTS: Counter = Counter::new(
    "apelora_http_requests",
    "API requests answered, by route template, method and status class.",
    &["route", "method", "status"],
);
pub static HTTP_DURATION: Histogram = Histogram::new(
    "apelora_http_request_duration_seconds",
    "Time to answer an API request, by route template and method.",
    &["route", "method"],
    HTTP_BUCKETS,
);
pub static HTTP_IN_FLIGHT: Gauge = Gauge::new(
    "apelora_http_requests_in_flight",
    "API requests being handled now.",
    &[],
);
pub static HTTP_RATE_LIMITED: Counter = Counter::new(
    "apelora_http_rate_limited",
    "API requests refused with 429 (a rate or quota limit), by route template.",
    &["route"],
);
pub static AUTH_ATTEMPTS: Counter = Counter::new(
    "apelora_auth_attempts",
    "Credentials checked, by method (oidc, personal_token, service, api_token, none, development, unrecognised) and result (ok, refused, error).",
    &["method", "result"],
);
pub static AUTHZ_DENIALS: Counter = Counter::new(
    "apelora_authz_denials",
    "Requests refused by authorisation, by the permission that was missing.",
    &["permission"],
);
pub static STORE_ERRORS: Counter = Counter::new(
    "apelora_store_errors",
    "Database errors that reached a caller or the job loop, by class (store_unavailable, store_error).",
    &["class"],
);

// ---- jobs (read from PostgreSQL at scrape) and the worker -----------------------------------

pub static JOBS: Gauge = Gauge::new(
    "apelora_jobs",
    "Jobs waiting or running, by kind and status (read from PostgreSQL; the same in every API process).",
    &["kind", "status"],
);
pub static JOBS_OLDEST_RUNNABLE_AGE: Gauge = Gauge::new(
    "apelora_jobs_oldest_runnable_age_seconds",
    "How long the oldest runnable pending job has been waiting for a worker (0 when none).",
    &[],
);
pub static JOBS_EXPIRED_LEASES: Gauge = Gauge::new(
    "apelora_jobs_expired_leases",
    "Running jobs whose lease has expired: their worker stopped and another one will take them over.",
    &[],
);
pub static JOB_DURATION: Histogram = Histogram::new(
    "apelora_worker_job_duration_seconds",
    "Time one job attempt took, by kind and outcome.",
    &["kind", "outcome"],
    LONG_BUCKETS,
);
pub static JOB_ATTEMPTS: Counter = Counter::new(
    "apelora_worker_job_attempts",
    "Job attempts finished, by kind and outcome (succeeded, retry, failed, rejected, waiting, cancelled, lost).",
    &["kind", "outcome"],
);
pub static LEASE_LOST: Counter = Counter::new(
    "apelora_worker_lease_lost",
    "Times this worker found that another worker had taken over its job, by stage (renew, handler, complete).",
    &["stage"],
);
pub static SUPERVISOR_ACTIONS: Counter = Counter::new(
    "apelora_supervisor_actions",
    "Replica actions the supervisor took, by action (recreate, restart, substitute, retire, health_restart) and outcome (queued, deferred).",
    &["action", "outcome"],
);
pub static SUPERVISOR_PASS: Histogram = Histogram::new(
    "apelora_supervisor_pass_duration_seconds",
    "Time one supervisor pass took, by result (ok, error).",
    &["result"],
    LONG_BUCKETS,
);
pub static SUPERVISOR_LAST_PASS: Gauge = Gauge::new(
    "apelora_supervisor_last_pass_timestamp_seconds",
    "When the last supervisor pass finished (Unix seconds).",
    &[],
);
pub static VOLUME_SCHEDULE_RUNS: Counter = Counter::new(
    "apelora_volume_schedule_runs",
    "Scheduled snapshot and backup runs, by kind (snapshot, backup) and outcome (ok, failed, not_configured).",
    &["kind", "outcome"],
);
// ---- the outbox relay (worker; only the holder of the `outbox-relay` lease reports) ---------

pub static OUTBOX_PENDING: Gauge = Gauge::new(
    "apelora_outbox_pending",
    "Outbox messages not delivered yet (read by the relay leader every pass; other workers report nothing).",
    &[],
);
pub static OUTBOX_OLDEST_PENDING_AGE: Gauge = Gauge::new(
    "apelora_outbox_oldest_pending_age_seconds",
    "How long the oldest undelivered outbox message has waited (0 when none).",
    &[],
);
pub static OUTBOX_FAILING: Gauge = Gauge::new(
    "apelora_outbox_failing",
    "Undelivered outbox messages whose last delivery attempt failed (they are retried with backoff).",
    &[],
);
pub static OUTBOX_DELIVERIES: Counter = Counter::new(
    "apelora_outbox_deliveries",
    "Outbox delivery attempts, by topic (audit.recorded, ...) and outcome (delivered, failed).",
    &["topic", "outcome"],
);

/// An outbox topic as a label: topics are names from code (`audit.recorded`), anything else
/// becomes `other`.
pub fn topic_label(topic: &str) -> &str {
    job_kind_label(topic)
}

// ---- alerts -----------------------------------------------------------------------------------

pub static MEMBER_FAILURES: Gauge = Gauge::new(
    "apelora_member_failures",
    "Failed members (Incus reports them Offline) by fencing state (waiting, fenced). Set each pass by the worker holding the replica-supervisor lease; the others report none.",
    &["state"],
);
pub static FENCE_PROBES: Counter = Counter::new(
    "apelora_fence_probes",
    "Reads of the shared storage's lease state on a member (sanlock, through the fence probe), by outcome (ok, failed).",
    &["outcome"],
);
pub static BMC_POWER_OFFS: Counter = Counter::new(
    "apelora_bmc_power_offs",
    "Attempts to fence a failed member by switching it off through its BMC (ADR 0014), by outcome (success: the power state read back Off; failed; refused: a guard held it back; unreadable: the power state could not be read).",
    &["outcome"],
);
pub static RECOVERIES: Counter = Counter::new(
    "apelora_recoveries",
    "Instances of a failed member restarted on another member after fencing, by outcome (queued, succeeded, failed).",
    &["outcome"],
);
pub static ALERTS_FIRING: Gauge = Gauge::new(
    "apelora_alerts_firing",
    "Open firing alerts by severity (critical, warning, info), silenced ones included. Set each pass by the worker holding the alert-evaluator lease; the others report none.",
    &["severity"],
);
pub static ALERT_EVALUATION: Histogram = Histogram::new(
    "apelora_alert_evaluation_duration_seconds",
    "Time one alert evaluation pass took, by result (ok, error).",
    &["result"],
    LONG_BUCKETS,
);

pub static LAST_BACKUP: Gauge = Gauge::new(
    "apelora_volume_last_backup_timestamp_seconds",
    "When the newest volume backup in each backup pool was made (Unix seconds; read from PostgreSQL).",
    &["pool"],
);

// ---- Incus calls ------------------------------------------------------------------------------

pub static INCUS_REQUESTS: Counter = Counter::new(
    "apelora_incus_requests",
    "Requests to the Incus API, by endpoint template, method and outcome (ok, not_found, client_error, auth_error, server_error, unreachable, bad_response).",
    &["endpoint", "method", "outcome"],
);
pub static INCUS_DURATION: Histogram = Histogram::new(
    "apelora_incus_request_duration_seconds",
    "Time an Incus API request took, by endpoint template and method.",
    &["endpoint", "method"],
    INCUS_BUCKETS,
);
pub static INCUS_OPERATION_WAIT: Histogram = Histogram::new(
    "apelora_incus_operation_wait_seconds",
    "Time spent waiting on one Incus operation call.",
    &[],
    LONG_BUCKETS,
);
pub static INCUS_UP: Gauge = Gauge::new(
    "apelora_incus_up",
    "1 when the last Incus request got an answer, 0 when it could not reach Incus.",
    &[],
);
pub static INCUS_LAST_SUCCESS: Gauge = Gauge::new(
    "apelora_incus_last_success_timestamp_seconds",
    "When an Incus request last succeeded (Unix seconds).",
    &[],
);

// ---- PostgreSQL pool --------------------------------------------------------------------------

pub static DB_CONNECTIONS: Gauge = Gauge::new(
    "apelora_db_pool_connections",
    "Database connections of this process, by state (in_use, idle, max).",
    &["state"],
);
pub static DB_ACQUIRE: Histogram = Histogram::new(
    "apelora_db_pool_acquire_seconds",
    "Time to get a connection from the pool, sampled once per scrape.",
    &[],
    ACQUIRE_BUCKETS,
);
pub static DB_UP: Gauge = Gauge::new(
    "apelora_db_up",
    "1 when the last scrape could get a database connection within 2 seconds, 0 otherwise.",
    &[],
);

// ---- inventory and capacity -------------------------------------------------------------------

pub static MEMBERS: Gauge = Gauge::new(
    "apelora_incus_members",
    "Incus cluster members by status, as last read (at most every 30 s, only when scraped).",
    &["status"],
);
pub static INSTANCES: Gauge = Gauge::new(
    "apelora_incus_instances",
    "Incus instances by status and kind (container, virtual-machine), as last read.",
    &["status", "kind"],
);
pub static APPLICATIONS: Gauge = Gauge::new(
    "apelora_applications",
    "Applications that are not archived, by derived status (running, degraded, failed, ...).",
    &["status"],
);
pub static CERTIFICATE_EXPIRY: Gauge = Gauge::new(
    "apelora_certificate_expiry_timestamp_seconds",
    "When the certificates Apelora uses to reach Incus expire (Unix seconds), by certificate (deploy, admin, server).",
    &["certificate"],
);
pub static INVENTORY_LAST_READ: Gauge = Gauge::new(
    "apelora_inventory_last_read_timestamp_seconds",
    "When members, instances and applications were last read successfully for these metrics (Unix seconds).",
    &[],
);
pub static IDP_KEY_FETCHES: Counter = Counter::new(
    "apelora_identity_key_fetches",
    "Reads of an identity provider's discovery document or signing keys, by source (discovery, key_set) and result (ok, error).",
    &["source", "result"],
);

// ---- process ----------------------------------------------------------------------------------

pub static BUILD_INFO: Gauge = Gauge::new(
    "apelora_build_info",
    "Always 1: which component, version and commit is running.",
    &["component", "version", "commit"],
);
pub static PROVIDER_INFO: Gauge = Gauge::new(
    "apelora_provider_info",
    "Always 1: whether this process talks to Incus or to the mock (mode).",
    &["mode"],
);

/// Every Apelora family, in the order they are rendered.
pub static FAMILIES: &[&dyn Family] = &[
    &BUILD_INFO,
    &PROVIDER_INFO,
    &HTTP_REQUESTS,
    &HTTP_DURATION,
    &HTTP_IN_FLIGHT,
    &HTTP_RATE_LIMITED,
    &AUTH_ATTEMPTS,
    &AUTHZ_DENIALS,
    &STORE_ERRORS,
    &JOBS,
    &JOBS_OLDEST_RUNNABLE_AGE,
    &JOBS_EXPIRED_LEASES,
    &JOB_DURATION,
    &JOB_ATTEMPTS,
    &LEASE_LOST,
    &SUPERVISOR_ACTIONS,
    &SUPERVISOR_PASS,
    &SUPERVISOR_LAST_PASS,
    &VOLUME_SCHEDULE_RUNS,
    &LAST_BACKUP,
    &OUTBOX_PENDING,
    &OUTBOX_OLDEST_PENDING_AGE,
    &OUTBOX_FAILING,
    &OUTBOX_DELIVERIES,
    &MEMBER_FAILURES,
    &FENCE_PROBES,
    &BMC_POWER_OFFS,
    &RECOVERIES,
    &ALERTS_FIRING,
    &ALERT_EVALUATION,
    &INCUS_REQUESTS,
    &INCUS_DURATION,
    &INCUS_OPERATION_WAIT,
    &INCUS_UP,
    &INCUS_LAST_SUCCESS,
    &DB_CONNECTIONS,
    &DB_ACQUIRE,
    &DB_UP,
    &MEMBERS,
    &INSTANCES,
    &APPLICATIONS,
    &CERTIFICATE_EXPIRY,
    &INVENTORY_LAST_READ,
    &IDP_KEY_FETCHES,
];

/// Seconds since the Unix epoch, now.
pub fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// The whole exposition: every Apelora family, then the standard process families.
pub fn render(openmetrics: bool) -> String {
    let p = place();
    let constant = vec![
        ("region".to_owned(), p.region),
        ("site".to_owned(), p.site),
        ("sector".to_owned(), p.sector),
    ];
    let mut out = String::with_capacity(16 * 1024);
    for f in FAMILIES {
        f.render(&mut out, &constant, openmetrics);
    }
    process::render(&mut out, &constant, openmetrics);
    if openmetrics {
        out.push_str("# EOF\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn family_names_are_unique_prefixed_and_never_use_the_constant_labels() {
        let mut seen = std::collections::HashSet::new();
        for f in FAMILIES {
            assert!(seen.insert(f.name()), "duplicate {}", f.name());
            assert!(f.name().starts_with("apelora_"), "{}", f.name());
            assert!(
                f.name()
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c == b'_'),
                "{}",
                f.name()
            );
            for l in f.label_names() {
                assert!(
                    !matches!(*l, "region" | "site" | "sector" | "le"),
                    "{} uses a reserved label {l}",
                    f.name()
                );
            }
        }
    }

    /// The cardinality guard in review form: no family may be labelled by anything that names
    /// one object (a path, a name, an id, a user).
    #[test]
    fn no_family_is_labelled_by_an_identifier() {
        const ALLOWED: &[&str] = &[
            "route",
            "method",
            "status",
            "result",
            "permission",
            "class",
            "kind",
            "outcome",
            "stage",
            "action",
            "pool",
            "endpoint",
            "state",
            "certificate",
            "source",
            "component",
            "version",
            "commit",
            "mode",
            // Outbox topics are names from code, few (`topic_label`).
            "topic",
            // Alert severities: critical, warning, info.
            "severity",
        ];
        for f in FAMILIES {
            for l in f.label_names() {
                assert!(
                    ALLOWED.contains(l),
                    "{} has label {l}: add it here only if its values are a small fixed set",
                    f.name()
                );
            }
        }
    }

    #[test]
    fn place_names_are_checked_and_default_when_unset() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == k)
                    .map(|(_, v)| (*v).to_owned())
            }
        };
        assert_eq!(Place::from_env(env(&[])), Ok(Place::default()));
        let p = Place::from_env(env(&[
            ("APELORA_REGION", "eu-west"),
            ("APELORA_SITE", "fra1"),
            ("APELORA_SECTOR", " s01 "),
        ]))
        .expect("valid");
        assert_eq!(
            (p.region.as_str(), p.site.as_str(), p.sector.as_str()),
            ("eu-west", "fra1", "s01")
        );
        for good in ["a", "eu-west", "s01", "fra1", &"a".repeat(63)] {
            assert!(valid_place_name(good), "{good}");
        }
        for bad in [
            "",
            "EU",
            "-eu",
            "eu-",
            "eu west",
            "eu\"x",
            "A",
            &"a".repeat(64),
        ] {
            assert!(!valid_place_name(bad), "{bad}");
        }
        assert!(Place::from_env(env(&[("APELORA_SITE", "Site 1")])).is_err());
    }

    #[test]
    fn helpers_map_free_text_to_small_sets() {
        assert_eq!(status_class(99), "5xx");
        assert_eq!(status_class(100), "1xx");
        assert_eq!(status_class(199), "1xx");
        assert_eq!(status_class(200), "2xx");
        assert_eq!(status_class(204), "2xx");
        assert_eq!(status_class(299), "2xx");
        assert_eq!(status_class(300), "3xx");
        assert_eq!(status_class(399), "3xx");
        assert_eq!(status_class(400), "4xx");
        assert_eq!(status_class(429), "4xx");
        assert_eq!(status_class(499), "4xx");
        assert_eq!(status_class(500), "5xx");
        assert_eq!(status_class(503), "5xx");
        assert_eq!(WORKER_METRICS_DEFAULT_ADDR, "127.0.0.1:9471");
        assert_eq!(method_label("BREW"), "other");
        assert_eq!(job_kind_label("instance.create"), "instance.create");
        assert_eq!(job_kind_label("x'; DROP"), "other");
        assert!(wants_openmetrics(Some(
            "application/openmetrics-text; version=1.0.0"
        )));
        assert!(!wants_openmetrics(Some("text/plain")));
        assert!(!wants_openmetrics(None));
    }

    #[test]
    fn the_exposition_lists_every_family_with_the_place_labels() {
        set_build("test", "0.0.0");
        HTTP_REQUESTS.inc(&["/api/v1/health", "GET", "2xx"]);
        let text = render(false);
        for f in FAMILIES {
            assert!(
                text.contains(&format!("# HELP {}", f.name())),
                "{}",
                f.name()
            );
        }
        assert!(text.contains("process_start_time_seconds"), "{text}");
        assert!(text.contains("sector=\"default\""), "{text}");
        assert!(!text.contains("# EOF"));
        assert!(render(true).ends_with("# EOF\n"));
    }
}

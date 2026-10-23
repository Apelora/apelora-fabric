// SPDX-License-Identifier: AGPL-3.0-only
//! Effective settings (Console plan, Administration, P1): every `APELORA_*` setting the control
//! plane and the worker read, with the value this control plane runs with, whether it is the
//! default, and where it came from; plus the sign-in settings saved in the Console.
//!
//! **Values are shown by allow-list.** A value appears only when [`SETTINGS`] marks the setting
//! `shown` *and* its name does not suggest a secret ([`secret_name`]) *and* [`scrub`] leaves it
//! unchanged (it does not contain anything that looks like a token, key or password). Everything
//! else, and every `APELORA_*` variable this version does not know, says only that it is set.
//! Paths to certificate and key files are not shown either.
//!
//! The control plane reads its environment once at start; this view shows that snapshot
//! (`AppState::with_environment`). The worker does not report its settings anywhere Apelora can
//! read: the worker rows below say which settings it reads, and the Health page shows each
//! worker's version.

use apelora_auth::Resource;
use axum::extract::State;
use axum::{Extension, Json};
use serde::Serialize;
use utoipa::ToSchema;

use super::authz::allow;
use super::{ApiError, AppState, ErrorBody, Principal};

/// Where a setting is read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reads {
    ControlPlane,
    Worker,
    Both,
    /// The web development server (`npm run dev`), not a service.
    DevServer,
}

impl Reads {
    fn names(self) -> Vec<String> {
        match self {
            Reads::ControlPlane => vec!["control-plane".into()],
            Reads::Worker => vec!["worker".into()],
            Reads::Both => vec!["control-plane".into(), "worker".into()],
            Reads::DevServer => vec!["web-dev-server".into()],
        }
    }
}

/// One setting this version reads.
pub struct Setting {
    pub name: &'static str,
    pub reads: Reads,
    /// What applies when it is not set; `None`: the feature is off or the value is computed.
    pub default: Option<&'static str>,
    /// The value may be shown (still subject to [`secret_name`] and [`scrub`]).
    pub shown: bool,
    pub what: &'static str,
}

const fn s(
    name: &'static str,
    reads: Reads,
    default: Option<&'static str>,
    what: &'static str,
) -> Setting {
    Setting {
        name,
        reads,
        default,
        shown: true,
        what,
    }
}

/// A setting whose value is never shown (a secret, or the path of a certificate or key file).
const fn hidden(name: &'static str, reads: Reads, what: &'static str) -> Setting {
    Setting {
        name,
        reads,
        default: None,
        shown: false,
        what,
    }
}

use Reads::{Both, ControlPlane as Cp, DevServer, Worker};

/// Every setting the control plane and the worker read (a test compares this with the source).
pub const SETTINGS: &[Setting] = &[
    hidden(
        "DATABASE_URL",
        Both,
        "PostgreSQL connection (contains a password); without it the control plane runs without sign-in on loopback only",
    ),
    s(
        "RUST_LOG",
        Both,
        Some("info"),
        "Which log lines are written",
    ),
    s(
        "APELORA_LOG_FORMAT",
        Both,
        Some("text"),
        "text or json log lines",
    ),
    s(
        "APELORA_OTLP_ENDPOINT",
        Both,
        Some("off"),
        "OTLP/HTTP traces for request and job spans (off, or an http(s) URL of a Tempo or collector inside the site)",
    ),
    s(
        "APELORA_API_ADDR",
        Cp,
        Some("127.0.0.1:8080"),
        "Address the API listens on",
    ),
    hidden(
        "APELORA_TLS_CERT",
        Cp,
        "Certificate chain file for HTTPS served by the API itself",
    ),
    hidden(
        "APELORA_TLS_KEY",
        Cp,
        "Private key file for HTTPS served by the API itself",
    ),
    s(
        "APELORA_BEHIND_PROXY",
        Cp,
        Some("0"),
        "1: a TLS-terminating proxy is in front",
    ),
    s(
        "APELORA_TRUSTED_PROXIES",
        Cp,
        None,
        "Proxies whose X-Forwarded-For is believed",
    ),
    s(
        "APELORA_RATE_LIMIT_PUBLIC_PER_MIN",
        Cp,
        Some("120"),
        "Public requests per client address per minute",
    ),
    s(
        "APELORA_RATE_LIMIT_PUBLIC_BURST",
        Cp,
        Some("60"),
        "Public requests saved up per client address",
    ),
    s(
        "APELORA_RATE_LIMIT_FAILED_AUTH_PER_MIN",
        Cp,
        Some("10"),
        "Refused credentials per client address per minute",
    ),
    s(
        "APELORA_RATE_LIMIT_FAILED_AUTH_BURST",
        Cp,
        Some("20"),
        "Refused credentials saved up per client address",
    ),
    s(
        "APELORA_RATE_LIMIT_WRITES_PER_MIN",
        Cp,
        Some("600"),
        "Changes per signed-in caller per minute",
    ),
    s(
        "APELORA_RATE_LIMIT_WRITES_BURST",
        Cp,
        Some("200"),
        "Changes saved up per signed-in caller",
    ),
    s(
        "APELORA_RATE_LIMIT_MAX_CLIENTS",
        Cp,
        Some("10000"),
        "Buckets kept per limit",
    ),
    s(
        "APELORA_INCUS_URL",
        Both,
        None,
        "Incus API address; without it the mock is used",
    ),
    hidden(
        "APELORA_INCUS_CLIENT_CERT",
        Both,
        "Client certificate file of the restricted deploy identity",
    ),
    hidden(
        "APELORA_INCUS_CLIENT_KEY",
        Both,
        "Private key file of the restricted deploy identity",
    ),
    hidden(
        "APELORA_INCUS_SERVER_CERT",
        Both,
        "Pinned Incus server certificate file",
    ),
    s(
        "APELORA_INCUS_TLS_SERVER_NAME",
        Both,
        None,
        "Name to expect in the Incus server certificate",
    ),
    hidden(
        "APELORA_INCUS_ADMIN_CLIENT_CERT",
        Cp,
        "Client certificate file of the unrestricted admin identity",
    ),
    hidden(
        "APELORA_INCUS_ADMIN_CLIENT_KEY",
        Cp,
        "Private key file of the unrestricted admin identity",
    ),
    s(
        "APELORA_DEPLOY_PROJECTS",
        Both,
        Some("apelora"),
        "Incus projects Apelora may change",
    ),
    s(
        "APELORA_NETWORK_UPLINK",
        Cp,
        None,
        "Uplink for per-application OVN networks",
    ),
    s(
        "APELORA_SERVICE_ADDRESSES",
        Cp,
        None,
        "Routed IPv4 (/16–/30) or IPv6 (/112–/126) range for service addresses",
    ),
    s(
        "APELORA_VOLUME_POOL",
        Cp,
        None,
        "Storage pool behind the data-local volume class",
    ),
    s(
        "APELORA_SHARED_POOL",
        Cp,
        None,
        "Shared storage pool behind the data-replicated volume class (modules that restart on another server after fencing)",
    ),
    s(
        "APELORA_PROBE_UID",
        Worker,
        Some("65534"),
        "User a module's readiness command runs as inside its instances (not root by default)",
    ),
    s(
        "APELORA_DB_BACKUP_DIR",
        Cp,
        None,
        "Directory the database backup and restore-drill scripts write their records to (read only; Administration shows whether backups are recent and a restore was verified)",
    ),
    s(
        "APELORA_IMAGE_MIRRORS",
        Both,
        None,
        "Image mirrors, upstream=mirror pairs",
    ),
    s(
        "APELORA_IMAGE_SERVERS",
        Both,
        Some("any"),
        "mirrors-only refuses every server that is not a mirror",
    ),
    s(
        "APELORA_LEGACY_TOKENS",
        Cp,
        Some("enabled"),
        "disabled refuses legacy apl_ API tokens",
    ),
    s(
        "APELORA_OIDC_ISSUER",
        Cp,
        None,
        "Identity provider set by the environment",
    ),
    s(
        "APELORA_OIDC_AUDIENCE",
        Cp,
        None,
        "Names this API has at the provider",
    ),
    s(
        "APELORA_OIDC_CLIENT_ID",
        Cp,
        None,
        "Public browser client (no secret)",
    ),
    s(
        "APELORA_OIDC_SCOPES",
        Cp,
        Some("openid profile email"),
        "Scopes the browser asks for",
    ),
    s("APELORA_OIDC_LABEL", Cp, None, "Name on the sign-in button"),
    s("APELORA_OIDC_JWKS_URL", Cp, None, "The provider's key set"),
    s(
        "APELORA_OIDC_GROUPS_CLAIM",
        Cp,
        Some("groups"),
        "Claim holding group names",
    ),
    s(
        "APELORA_OIDC_USERNAME_CLAIM",
        Cp,
        Some("preferred_username"),
        "Claim holding the user name",
    ),
    s(
        "APELORA_OIDC_MFA_AMR",
        Cp,
        None,
        "amr values that count as multi-factor",
    ),
    s(
        "APELORA_OIDC_MFA_ACR",
        Cp,
        None,
        "acr values that count as multi-factor",
    ),
    s(
        "APELORA_OIDC_AUTHORIZED_PARTIES",
        Cp,
        None,
        "Clients a token must be issued to (azp)",
    ),
    s(
        "APELORA_CONSOLE_ACCESS",
        Cp,
        Some("enabled"),
        "disabled turns terminal sessions off",
    ),
    s(
        "APELORA_CONSOLE_UID",
        Cp,
        Some("1000"),
        "User terminal sessions run as",
    ),
    s(
        "APELORA_CONSOLE_GID",
        Cp,
        Some("1000"),
        "Group terminal sessions run as",
    ),
    s(
        "APELORA_CONSOLE_IDLE_SECS",
        Cp,
        Some("900"),
        "No typing for this long ends a session",
    ),
    s(
        "APELORA_CONSOLE_MAX_SECS",
        Cp,
        Some("3600"),
        "Absolute limit of a session",
    ),
    s(
        "APELORA_CONSOLE_MAX_SESSIONS",
        Cp,
        Some("3"),
        "Open sessions and tickets per person",
    ),
    s(
        "APELORA_CONSOLE_POWERSHELL",
        Cp,
        Some("disabled"),
        "enabled offers PowerShell",
    ),
    s(
        "APELORA_ALLOWED_ORIGINS",
        Cp,
        None,
        "Browser origins besides the API's own host",
    ),
    s(
        "APELORA_REGION",
        Both,
        Some("default"),
        "Constant region label on metrics",
    ),
    s(
        "APELORA_SITE",
        Both,
        Some("default"),
        "Constant site label on metrics",
    ),
    s(
        "APELORA_SECTOR",
        Both,
        Some("default"),
        "Constant sector label on metrics",
    ),
    s(
        "APELORA_METRICS_ADDR",
        Cp,
        Some("off"),
        "Separate metrics listener without authentication",
    ),
    s(
        "APELORA_WORKER_METRICS_ADDR",
        Both,
        Some("127.0.0.1:9471"),
        "The worker's metrics and liveness listener",
    ),
    s(
        "APELORA_WORKER_ID",
        Worker,
        None,
        "Worker name (default worker-<process id>)",
    ),
    s(
        "APELORA_SUPERVISE_SECS",
        Worker,
        Some("30"),
        "Seconds between replica supervisor passes",
    ),
    s(
        "APELORA_RECOVER_AFTER_SECS",
        Worker,
        Some("120"),
        "Offline time before replicas are substituted",
    ),
    s(
        "APELORA_REBALANCE",
        Worker,
        None,
        "1: move a stateless replica that shares a server with another of its module (off when unset)",
    ),
    s(
        "APELORA_DNS_BINARY",
        Worker,
        None,
        "Path of the Linux apelora-dns executable the worker copies into a resolver (default: apelora-dns next to the worker)",
    ),
    s(
        "APELORA_HEALTH_FAIL_AFTER_SECS",
        Worker,
        Some("20"),
        "Failing checks for this long: unhealthy",
    ),
    s(
        "APELORA_HEALTH_RESTART_AFTER_SECS",
        Worker,
        Some("60"),
        "Unhealthy for this long: restarted",
    ),
    s(
        "APELORA_FENCE_PROBE",
        Worker,
        Some("off"),
        "How the worker reads the shared storage's leases to fence a failed server (off or ssh)",
    ),
    hidden(
        "APELORA_FENCE_PROBE_KEY",
        Worker,
        "Private key of the fence probe's forced SSH command",
    ),
    hidden(
        "APELORA_FENCE_PROBE_KNOWN_HOSTS",
        Worker,
        "Pinned host keys of the members the fence probe reads",
    ),
    s(
        "APELORA_FENCE_PROBE_USER",
        Worker,
        Some("apelora-fence"),
        "Account of the fence probe on each member",
    ),
    s(
        "APELORA_FENCE_PROBE_TARGETS",
        Worker,
        None,
        "Where the fence probe reaches each member (member=host[:port]); default the member's address, port 22",
    ),
    s(
        "APELORA_FENCE_LOCKSPACE",
        Worker,
        None,
        "The shared volume group's lockspace the fence probe reads; default the only one joined",
    ),
    hidden(
        "APELORA_BMC_MAP",
        Worker,
        "File naming each server's BMC (ADR 0014): provider (redfish, ipmi, maas), address, user, the name of its password variable and its hardware UUID (for IPMI the system GUID). No passwords in it; absent: no fencing through a BMC",
    ),
    s(
        "APELORA_MAAS_URL",
        Both,
        None,
        "Address of the MAAS region API (https://host:5240/MAAS); empty: only manual machines. The control plane lists its machines; the worker can fence through it",
    ),
    hidden(
        "APELORA_MAAS_API_KEY",
        Both,
        "MAAS API key (consumer:token:secret) of a dedicated MAAS user; read once at start, never shown",
    ),
    s(
        "APELORA_MAAS_CA",
        Both,
        None,
        "PEM file of the authority that signed MAAS's certificate; default the system roots",
    ),
    s(
        "APELORA_BMC_FENCE_AFTER_SECS",
        Worker,
        Some("30"),
        "A server Offline for this long (at least 20) is switched off through its BMC and fenced",
    ),
    s(
        "APELORA_BMC_ALLOW_LAB",
        Worker,
        None,
        "Allows the lab-only provider that stops a Lima VM (the disposable test cluster); never in production",
    ),
    s(
        "APELORA_BACKUP_POOL",
        Worker,
        None,
        "Pool scheduled backups are copied to",
    ),
    s(
        "APELORA_OUTBOX_SINK",
        Worker,
        Some("log"),
        "Where audit messages are relayed",
    ),
    s(
        "APELORA_ALERT_EVALUATE_SECS",
        Worker,
        Some("30"),
        "How often built-in alert rules are evaluated (0: off)",
    ),
    s(
        "APELORA_RESTORE_DRILL_RECORD",
        Worker,
        None,
        "The restore drill's result file the restore-age alert reads",
    ),
    s(
        "APELORA_API_URL",
        DevServer,
        Some("http://127.0.0.1:8080"),
        "Where the web development server sends /api",
    ),
];

/// The settings reference as a Markdown document (`docs/architecture/SETTINGS.md`, generated from
/// [`SETTINGS`] so that it cannot drift: a test compares it with the committed file).
pub fn markdown() -> String {
    use std::fmt::Write;
    let mut out = String::from(
        "# Settings reference\n\n\
         Generated by `apelora-control-plane settings` (`npm run settings:doc`) from the list the control plane itself \
         reports at `GET /api/v1/settings`. Do not edit by hand.\n\n\
         Every setting is an environment variable. The control plane reads its own at start; the worker reads its \
         own; a setting both read must be given to both. A value marked *never shown* (a secret, or the path of a \
         certificate or key file) is not displayed by the Console or the API. Defaults apply when the variable is not set; \
         *none* means the feature is off or the value is worked out.\n",
    );
    for (reads, title) in [
        (Reads::Both, "Read by the control plane and the worker"),
        (Reads::ControlPlane, "Read by the control plane only"),
        (Reads::Worker, "Read by the worker only"),
        (Reads::DevServer, "Read by the web development server only"),
    ] {
        let mut rows: Vec<&Setting> = SETTINGS.iter().filter(|x| x.reads == reads).collect();
        if rows.is_empty() {
            continue;
        }
        rows.sort_by_key(|x| x.name);
        let _ = write!(
            out,
            "\n## {title}\n\n| Setting | Default | What it does |\n|---|---|---|\n"
        );
        for x in rows {
            let default = x.default.map_or("none".to_owned(), |d| format!("`{d}`"));
            let what = x.what.replace('|', "\\|");
            let note = if x.shown || x.what.contains("never shown") {
                ""
            } else {
                " (never shown)"
            };
            let _ = writeln!(out, "| `{}` | {default} | {what}{note} |", x.name);
        }
    }
    out
}

/// Names that suggest a secret: their values are never shown, whatever [`SETTINGS`] says.
pub fn secret_name(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    n == "DATABASE_URL"
        || n.split('_').any(|w| {
            [
                "SECRET",
                "SECRETS",
                "PASSWORD",
                "PASSWD",
                "PASS",
                "TOKEN",
                "KEY",
                "KEYS",
                "CERT",
                "CERTS",
                "CREDENTIAL",
                "CREDENTIALS",
                "PRIVATE",
                "DSN",
            ]
            .contains(&w)
        })
}

/// Text with anything that looks like a credential replaced: Apelora tokens (`apl_`, `aps_`,
/// `apu_`, `apw_`, `apt_` with their hex), PEM blocks (keys and certificates), bearer values,
/// JSON web tokens and the password in a URL's user information. Used on every value shown here
/// and on everything written into a support bundle.
pub fn scrub(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let rest = &text[i..];
        // PEM block: everything from BEGIN to the end of the matching END line.
        if rest.starts_with("-----BEGIN") {
            let end = rest
                .find("-----END")
                .and_then(|e| rest[e + 8..].find("-----").map(|f| e + 8 + f + 5))
                .unwrap_or(rest.len());
            out.push_str("[redacted PEM block]");
            i += end;
            continue;
        }
        // Apelora tokens.
        if let Some(prefix) = ["apl_", "aps_", "apu_", "apw_", "apt_"]
            .iter()
            .find(|p| rest.starts_with(**p))
            && (i == 0 || !b[i - 1].is_ascii_alphanumeric())
        {
            let hex = rest[4..].bytes().take_while(u8::is_ascii_hexdigit).count();
            if hex >= 32 {
                out.push_str(prefix);
                out.push_str("[redacted]");
                i += 4 + hex;
                continue;
            }
        }
        // Bearer values.
        if rest
            .get(..7)
            .is_some_and(|w| w.eq_ignore_ascii_case("bearer "))
        {
            let v = rest[7..]
                .bytes()
                .take_while(|c| !c.is_ascii_whitespace() && *c != b'"' && *c != b'\'')
                .count();
            if v > 0 {
                out.push_str(&rest[..7]);
                out.push_str("[redacted]");
                i += 7 + v;
                continue;
            }
        }
        // JSON web tokens: eyJ… . … . …
        if rest.starts_with("eyJ") && (i == 0 || !b[i - 1].is_ascii_alphanumeric()) {
            let n = rest
                .bytes()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
                .count();
            if n > 20 && rest[..n].matches('.').count() >= 2 {
                out.push_str("[redacted token]");
                i += n;
                continue;
            }
        }
        // user:password@ after a URL scheme.
        if let Some(after) = rest.strip_prefix("://") {
            let auth = after
                .find(['/', '?', '#', ' ', '"', '\n'])
                .map_or(after, |e| &after[..e]);
            if let Some(at) = auth.rfind('@')
                && let Some(colon) = auth[..at].find(':')
            {
                out.push_str("://");
                out.push_str(&auth[..colon]);
                out.push_str(":[redacted]@");
                i += 3 + at + 1;
                continue;
            }
        }
        let c = rest.chars().next().unwrap_or(' ');
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// The variables a process was started with that this view may describe: `APELORA_*`,
/// `DATABASE_URL` and `RUST_LOG`. Taken once at start (`main.rs`).
pub fn snapshot(vars: impl IntoIterator<Item = (String, String)>) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = vars
        .into_iter()
        .filter(|(k, _)| k.starts_with("APELORA_") || k == "DATABASE_URL" || k == "RUST_LOG")
        .collect();
    v.sort();
    v
}

#[derive(Serialize, ToSchema, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveSetting {
    pub name: String,
    /// The value in use, when it may be shown (the default when not set).
    pub value: Option<String>,
    /// Set in the environment (or saved in the Console).
    pub set: bool,
    /// The default applies (not set, or set to the default).
    pub is_default: bool,
    /// What applies when it is not set; absent when the feature is then off.
    pub default: Option<String>,
    /// `environment`, `default`, `not set` or `console`.
    pub source: String,
    /// Why the value is not shown: `secret` (a secret or a key or certificate file),
    /// `looks like a credential`, `unknown setting`; absent when shown or not set.
    pub hidden: Option<String>,
    /// Which processes read it: `control-plane`, `worker`, `web-dev-server`.
    pub read_by: Vec<String>,
    pub description: String,
    /// A setting this version reads; `false` for an `APELORA_*` variable it ignores (a typo?).
    pub known: bool,
}

#[derive(Serialize, ToSchema, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SettingsWorker {
    pub id: String,
    pub host: String,
    pub version: String,
    /// RFC 3339.
    pub last_seen: String,
}

#[derive(Serialize, ToSchema, Debug)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveSettings {
    /// Environment settings of this control plane, in name order, then unknown `APELORA_*`
    /// variables.
    pub items: Vec<EffectiveSetting>,
    /// Sign-in settings saved in the Console.
    pub saved: Vec<EffectiveSetting>,
    /// Workers seen in the last day. They do not report their settings: the worker's values
    /// are those of its own environment, which Apelora cannot read.
    pub workers: Vec<SettingsWorker>,
    /// RFC 3339: when this control plane read its environment.
    pub read_at: String,
}

/// The settings view for an environment snapshot.
pub fn effective(env: &[(String, String)]) -> Vec<EffectiveSetting> {
    let get = |name: &str| env.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());
    let mut out: Vec<EffectiveSetting> = SETTINGS
        .iter()
        .map(|d| {
            let value = get(d.name).filter(|v| !v.trim().is_empty());
            let set = value.is_some();
            let is_default = match (&value, d.default) {
                (None, _) => true,
                (Some(v), Some(def)) => v.trim() == def,
                (Some(_), None) => false,
            };
            let (shown, hidden) = match &value {
                None => (d.default.map(str::to_owned), None),
                Some(_) if !d.shown || secret_name(d.name) => (None, Some("secret")),
                Some(v) if scrub(v) != *v => (None, Some("looks like a credential")),
                Some(v) => (Some(v.chars().take(512).collect()), None),
            };
            EffectiveSetting {
                name: d.name.into(),
                value: shown,
                set,
                is_default,
                default: d.default.map(str::to_owned),
                source: if set {
                    "environment"
                } else if d.default.is_some() {
                    "default"
                } else {
                    "not set"
                }
                .into(),
                hidden: hidden.map(str::to_owned),
                read_by: d.reads.names(),
                description: d.what.into(),
                known: true,
            }
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    // Variables that look like settings but are not read: listed (a typo is a common cause of
    // "the setting does nothing"), never shown.
    for (k, _) in env {
        if k.starts_with("APELORA_") && !SETTINGS.iter().any(|d| d.name == k) {
            out.push(EffectiveSetting {
                name: k.clone(),
                value: None,
                set: true,
                is_default: false,
                default: None,
                source: "environment".into(),
                hidden: Some("unknown setting".into()),
                read_by: vec![],
                description: "Not a setting this version reads: check the name".into(),
                known: false,
            });
        }
    }
    out
}

/// The settings this control plane runs with: every setting the control plane and the worker
/// read, the value when it may be shown (never a secret, a key or a certificate path), whether
/// it is the default and where it came from; sign-in settings saved in the Console; the workers
/// seen recently. Needs `cluster.view` on the platform.
#[utoipa::path(get, path = "/api/v1/settings", tag = "system", operation_id = "effective_settings",
    responses((status = 200, body = EffectiveSettings), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn effective_settings(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<EffectiveSettings>, ApiError> {
    allow(&p, "cluster.view", &Resource::platform())?;
    Ok(Json(report(&state).await?))
}

/// The report, also written into the support bundle.
pub(super) async fn report(state: &AppState) -> Result<EffectiveSettings, ApiError> {
    let saved = vec![
        saved(
            "auth.local_accounts",
            if state.local_accounts_enabled() {
                "on"
            } else {
                "off"
            },
            "off",
            "Sign-in with local accounts (Identity & Access → Sign-in)",
        ),
        saved(
            "identity.api_tokens",
            if state.api_tokens_accepted() {
                "accepted"
            } else {
                "refused"
            },
            "accepted",
            "Legacy API tokens accepted (also needs APELORA_LEGACY_TOKENS)",
        ),
    ];
    let workers = match &state.auth {
        super::Auth::Tokens(pool) => apelora_persistence::status::workers(pool)
            .await?
            .into_iter()
            .map(|w| SettingsWorker {
                id: w.id,
                host: w.host,
                version: w.version,
                last_seen: super::admin::rfc3339(w.last_seen),
            })
            .collect(),
        super::Auth::DisabledForDevelopment => vec![],
    };
    Ok(EffectiveSettings {
        items: effective(&state.environment),
        saved,
        workers,
        read_at: state.started_at.to_string(),
    })
}

fn saved(name: &str, value: &str, default: &str, what: &str) -> EffectiveSetting {
    EffectiveSetting {
        name: name.into(),
        value: Some(value.into()),
        set: value != default,
        is_default: value == default,
        default: Some(default.into()),
        source: if value == default {
            "default"
        } else {
            "console"
        }
        .into(),
        hidden: None,
        read_by: vec!["control-plane".into()],
        description: what.into(),
        known: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Built at run time, so the repository's secret scan never sees a token-shaped literal.
    fn planted() -> String {
        format!("apl_{}", "0123456789abcdef".repeat(4))
    }

    /// A PEM block, built at run time for the same reason.
    fn pem(body: &str) -> String {
        let label = ["PRIVATE", "KEY"].join(" ");
        format!("-----BEGIN {label}-----\n{body}\n-----END {label}-----")
    }

    fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        snapshot(pairs.iter().map(|(k, v)| ((*k).into(), (*v).into())))
    }

    fn find<'a>(items: &'a [EffectiveSetting], name: &str) -> &'a EffectiveSetting {
        items.iter().find(|s| s.name == name).expect(name)
    }

    #[test]
    fn values_are_shown_only_when_allowed_and_never_for_secrets() {
        let items = effective(&env(&[
            ("APELORA_API_ADDR", "0.0.0.0:8080"),
            ("APELORA_DEPLOY_PROJECTS", "apelora"),
            ("APELORA_TLS_KEY", "/etc/apelora/tls/api.key"),
            ("APELORA_INCUS_CLIENT_CERT", "/etc/apelora/incus/client.crt"),
            (
                "DATABASE_URL",
                "postgres://apelora:hunter2-planted@db.internal/apelora",
            ),
            ("APELORA_OIDC_LABEL", &planted()),
            ("APELORA_WEBHOOK_SECRET", "planted-webhook-secret"),
            ("APELORA_TYPO_ADDR", "planted-unknown-value"),
            ("HOME", "/root"),
        ]));
        let addr = find(&items, "APELORA_API_ADDR");
        assert_eq!(addr.value.as_deref(), Some("0.0.0.0:8080"));
        assert_eq!(
            (addr.source.as_str(), addr.is_default),
            ("environment", false)
        );
        let projects = find(&items, "APELORA_DEPLOY_PROJECTS");
        assert!(projects.is_default && projects.set, "set to its default");
        let unset = find(&items, "APELORA_CONSOLE_IDLE_SECS");
        assert_eq!(
            (unset.value.as_deref(), unset.source.as_str(), unset.set),
            (Some("900"), "default", false)
        );
        assert_eq!(find(&items, "APELORA_VOLUME_POOL").source, "not set");
        for name in [
            "APELORA_TLS_KEY",
            "APELORA_INCUS_CLIENT_CERT",
            "DATABASE_URL",
        ] {
            let s = find(&items, name);
            assert!(s.set && s.value.is_none(), "{name}");
            assert_eq!(s.hidden.as_deref(), Some("secret"), "{name}");
        }
        let label = find(&items, "APELORA_OIDC_LABEL");
        assert_eq!(
            (label.value.as_deref(), label.hidden.as_deref()),
            (None, Some("looks like a credential"))
        );
        let typo = find(&items, "APELORA_TYPO_ADDR");
        assert!(!typo.known && typo.value.is_none());
        assert!(
            items.iter().all(|s| s.name != "HOME"),
            "only Apelora's settings"
        );
        let all = serde_json::to_string(&items).expect("json");
        for planted in [
            planted().as_str(),
            "hunter2-planted",
            "planted-webhook-secret",
            "planted-unknown-value",
            "/etc/apelora/tls/api.key",
        ] {
            assert!(!all.contains(planted), "{planted} leaked");
        }
    }

    #[test]
    fn secret_looking_names_are_never_shown_even_if_listed() {
        for name in [
            "APELORA_TLS_KEY",
            "APELORA_X_PASSWORD",
            "APELORA_X_TOKEN",
            "APELORA_X_SECRET",
            "APELORA_INCUS_SERVER_CERT",
            "DATABASE_URL",
        ] {
            assert!(secret_name(name), "{name}");
        }
        for name in ["APELORA_API_ADDR", "APELORA_REGION", "RUST_LOG"] {
            assert!(!secret_name(name), "{name}");
        }
        for d in SETTINGS {
            if secret_name(d.name) {
                assert!(!d.shown, "{} looks secret but is marked shown", d.name);
            }
        }
    }

    #[test]
    fn scrub_removes_tokens_keys_bearer_values_and_url_passwords() {
        let text = format!(
            "token {t} and aps_{h} and apt_{h}\nAuthorization: Bearer abc.def.ghi\n{key}\njwt eyJhbGciOiJFUzI1NiJ9.eyJzdWIiOiJ4In0.c2lnbmF0dXJlcGxhbnRlZA url postgres://u:pw-planted@h/db keep apl_short and https://example.test/path",
            h = "f".repeat(64),
            t = planted(),
            key = pem("MIIplanted"),
        );
        let s = scrub(&text);
        let token = planted();
        for gone in [
            token.as_str(),
            &"f".repeat(64),
            "abc.def.ghi",
            "MIIplanted",
            "c2lnbmF0dXJlcGxhbnRlZA",
            "pw-planted",
        ] {
            assert!(!s.contains(gone), "{gone} left in {s}");
        }
        for kept in [
            "apl_[redacted]",
            "Bearer [redacted]",
            "[redacted PEM block]",
            "postgres://u:[redacted]@h/db",
            "apl_short",
            "https://example.test/path",
        ] {
            assert!(s.contains(kept), "{kept} missing in {s}");
        }
        assert_eq!(scrub("plain text: ünïcode"), "plain text: ünïcode");
    }

    /// Every `APELORA_*` name the services read is in [`SETTINGS`], so a new setting cannot be
    /// missing from the Administration page.
    #[test]
    fn every_setting_the_services_read_is_listed() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
        let mut missing = Vec::new();
        let mut files = Vec::new();
        for dir in [
            "services/control-plane/src",
            "services/deployment-worker/src",
            "crates/incus-adapter/src",
            "crates/metrics/src",
            "crates/telemetry/src",
        ] {
            let mut stack = vec![std::path::PathBuf::from(root).join(dir)];
            while let Some(d) = stack.pop() {
                for e in std::fs::read_dir(&d).expect("dir").flatten() {
                    let p = e.path();
                    if p.is_dir() {
                        stack.push(p);
                    } else if p.extension().is_some_and(|x| x == "rs")
                        && !p.to_string_lossy().contains("test")
                    {
                        files.push(p);
                    }
                }
            }
        }
        for f in files {
            let text = std::fs::read_to_string(&f).expect("read");
            let text = text.split("#[cfg(test)]").next().unwrap_or("");
            for (i, _) in text.match_indices("\"APELORA_") {
                let name: String = text[i + 1..]
                    .chars()
                    .take_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_')
                    .collect();
                // Prefixes assembled at run time (`APELORA_RATE_LIMIT_{name}_PER_MIN`) and the
                // build-time commit are covered by their full names or are not run-time settings.
                let partial = name.ends_with('_') || text[i + 1 + name.len()..].starts_with('{');
                if partial || name == "APELORA_BUILD_COMMIT" {
                    continue;
                }
                if !SETTINGS.iter().any(|d| d.name == name) {
                    missing.push(format!("{name} ({})", f.display()));
                }
            }
        }
        missing.sort();
        missing.dedup();
        assert!(
            missing.is_empty(),
            "add these to settings::SETTINGS: {missing:#?}"
        );
    }
}

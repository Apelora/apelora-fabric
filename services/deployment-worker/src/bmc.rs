// SPDX-License-Identifier: AGPL-3.0-only
//! Fence providers (ADR 0014): switching a failed member off through its BMC, and reading the
//! power state back, so `Off` can fence it before its modules restart elsewhere.
//!
//! Apelora acts only on members named in a mapping file (`APELORA_BMC_MAP`, JSON, no secrets):
//!
//! ```json
//! { "apelora-test-1": { "provider": "redfish", "url": "https://10.0.0.11",
//!       "user": "apelora-fence", "secretRef": "APELORA_BMC_SECRET_…",
//!       "ca": "/etc/apelora/bmc-ca.pem", "system": "/redfish/v1/Systems/1",
//!       "uuid": "4c4c4544-0042-4c10-8051-b7c04f435331" },
//!   "apelora-test-4": { "provider": "lima", "vm": "apelora-incus-4" } }
//! ```
//!
//! The password is read from the environment variable `secretRef` names, at the moment it is
//! used; it is never stored, logged or put in an error. Redfish needs `https` (verified against
//! the system roots or the BMC's own `ca` file; there is no insecure mode) and the server's
//! hardware `uuid`: the BMC must report exactly that system UUID before anything is read as
//! evidence or sent, so a mapping that points at the wrong BMC refuses instead of switching off
//! (or vouching for) another server. The `maas` provider asks MAAS to read and switch the power
//! (`{ "provider": "maas", "systemId": "abc123", "uuid": "…" }`, with `APELORA_MAAS_URL`,
//! `APELORA_MAAS_API_KEY` and optionally `APELORA_MAAS_CA`): the BMC credentials stay in MAAS, and
//! MAAS's record of the machine must report the mapped hardware UUID. The `ipmi` provider speaks
//! IPMI over the LAN to BMCs without Redfish (`{ "provider": "ipmi", "host": "10.0.0.21",
//! "port": 623, "user": "apelora-fence", "secretRef": "APELORA_BMC_SECRET_…", "uuid": "…" }`):
//! encrypted sessions only (cipher suite 17, else 3), the operator role, and the BMC's system
//! GUID must be the mapped UUID before the power is read or switched ([`ipmi`]). The `lima`
//! provider is for the disposable lab only and needs `APELORA_BMC_ALLOW_LAB=1`.
//!
//! Settings: `APELORA_BMC_MAP` (file; absent: no BMC fencing), `APELORA_BMC_FENCE_AFTER_SECS`
//! (default 30, at least 20: how long a member is Offline before it is switched off) and
//! `APELORA_BMC_ALLOW_LAB`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use apelora_domain::fencing::{MIN_FENCE_AFTER_SECS, PowerOffReading};
use serde::Deserialize;
use serde_json::Value;

mod ipmi;

/// How long one request to a BMC may take.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// After a power-off request, how long and how often the power state is read back.
const SETTLE_BUDGET: Duration = Duration::from_secs(40);
const SETTLE_EVERY: Duration = Duration::from_secs(2);
/// The prefix every BMC secret's variable name must have.
pub const SECRET_PREFIX: &str = "APELORA_BMC_SECRET_";

/// Where a password comes from: the worker's environment (tests: a fixed map).
pub type SecretSource = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PowerState {
    On,
    Off,
    /// Anything else the BMC reports (`PoweringOn`, `PoweringOff`, `Paused`…): not evidence.
    Other(String),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "provider", rename_all = "kebab-case", deny_unknown_fields)]
enum Entry {
    #[serde(rename_all = "camelCase")]
    Redfish {
        url: String,
        user: String,
        secret_ref: String,
        #[serde(default)]
        ca: Option<PathBuf>,
        #[serde(default)]
        system: Option<String>,
        uuid: String,
    },
    #[serde(rename_all = "camelCase")]
    Maas {
        system_id: String,
        uuid: String,
    },
    #[serde(rename_all = "camelCase")]
    Ipmi {
        host: String,
        #[serde(default = "ipmi_port")]
        port: u16,
        user: String,
        secret_ref: String,
        uuid: String,
    },
    Lima {
        vm: String,
    },
}

/// One member's validated BMC entry.
#[derive(Debug, Clone)]
struct Target {
    entry: Entry,
}

/// The mapping and how to use it.
#[derive(Clone, Default)]
pub struct Mapping {
    targets: BTreeMap<String, Target>,
    secrets: Option<SecretSource>,
    /// The MAAS client for `maas` entries (from `APELORA_MAAS_URL` and `APELORA_MAAS_API_KEY`).
    maas: Option<apelora_maas_adapter::MaasClient>,
    /// Plain HTTP to a loopback test server (tests only).
    plain: bool,
    settle: Option<(Duration, Duration)>,
}

impl std::fmt::Debug for Mapping {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mapping")
            .field("members", &self.targets.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// Fixed behaviour per member, for the supervisor's tests; never configured from the environment.
#[derive(Debug, Clone)]
pub struct FixedMember {
    pub on: bool,
    /// The power-off request fails with this.
    pub refuse: Option<String>,
    /// The request is accepted but the power state stays On.
    pub stays_on: bool,
}

/// What a fixed provider was asked, oldest first: `("read" | "off", member)`.
pub type FixedCalls = Vec<(String, String)>;

#[derive(Debug, Clone, Default)]
pub enum Bmc {
    /// No mapping: only the storage lease or a person fences a member.
    #[default]
    Off,
    Mapped(Mapping),
    Fixed(Arc<Mutex<(BTreeMap<String, FixedMember>, FixedCalls)>>),
}

fn valid_secret_name(n: &str) -> bool {
    n.starts_with(SECRET_PREFIX)
        && n.len() > SECRET_PREFIX.len()
        && n.len() <= 80
        && n.bytes()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
}

fn valid_uuid(u: &str) -> bool {
    u.len() == 36
        && u.bytes().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == b'-',
            _ => c.is_ascii_hexdigit(),
        })
}

fn ipmi_port() -> u16 {
    623
}

/// A BMC address for IPMI: an IP address or a host name, nothing else.
fn valid_host(h: &str) -> bool {
    h.parse::<std::net::IpAddr>().is_ok()
        || (!h.is_empty()
            && h.len() <= 253
            && h.split('.').all(|l| {
                !l.is_empty()
                    && l.len() <= 63
                    && !l.starts_with('-')
                    && !l.ends_with('-')
                    && l.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
            }))
}

fn valid_vm(vm: &str) -> bool {
    vm.starts_with("apelora-")
        && vm != "apelora-san"
        && vm.len() <= 40
        && vm
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

impl Mapping {
    /// Parses and checks a mapping file's text. Every problem is named; none is skipped.
    pub fn parse(text: &str, allow_lab: bool) -> Result<Mapping, String> {
        let raw: BTreeMap<String, Value> =
            serde_json::from_str(text).map_err(|e| format!("not a JSON object of members: {e}"))?;
        let mut targets = BTreeMap::new();
        for (member, v) in raw {
            if !apelora_domain::is_valid_name(&member) {
                return Err(format!("{member:?} is not a member name"));
            }
            let entry: Entry = serde_json::from_value(v).map_err(|e| format!("{member}: {e}"))?;
            match &entry {
                Entry::Redfish {
                    url,
                    user,
                    secret_ref,
                    ca,
                    uuid,
                    system,
                } => {
                    let parsed =
                        reqwest::Url::parse(url).map_err(|e| format!("{member}: url: {e}"))?;
                    if parsed.scheme() != "https" || parsed.host_str().is_none() {
                        return Err(format!("{member}: the BMC url must be https://host[:port]"));
                    }
                    if !parsed.username().is_empty()
                        || parsed.password().is_some()
                        || parsed.query().is_some()
                    {
                        return Err(format!(
                            "{member}: the BMC url carries no credentials or query"
                        ));
                    }
                    if user.trim().is_empty() || user.chars().any(char::is_control) {
                        return Err(format!("{member}: user is empty"));
                    }
                    if !valid_secret_name(secret_ref) {
                        return Err(format!(
                            "{member}: secretRef must be an environment variable name starting with {SECRET_PREFIX} (capitals, digits, underscores)"
                        ));
                    }
                    if !valid_uuid(uuid) {
                        return Err(format!(
                            "{member}: uuid is the server's hardware UUID (8-4-4-4-12 hexadecimal)"
                        ));
                    }
                    if let Some(ca) = ca
                        && !ca.is_file()
                    {
                        return Err(format!("{member}: the CA file {} is missing", ca.display()));
                    }
                    if let Some(s) = system
                        && !(s.starts_with("/redfish/v1/Systems/")
                            && !s.contains("..")
                            && !s.contains('?'))
                    {
                        return Err(format!(
                            "{member}: system is a path under /redfish/v1/Systems/"
                        ));
                    }
                }
                Entry::Maas { system_id, uuid } => {
                    if !(3..=16).contains(&system_id.len())
                        || !system_id
                            .bytes()
                            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
                    {
                        return Err(format!(
                            "{member}: systemId is a MAAS system id (lowercase letters and digits)"
                        ));
                    }
                    if !valid_uuid(uuid) {
                        return Err(format!(
                            "{member}: uuid is the server's hardware UUID (8-4-4-4-12 hexadecimal)"
                        ));
                    }
                }
                Entry::Ipmi {
                    host,
                    port,
                    user,
                    secret_ref,
                    uuid,
                } => {
                    if !valid_host(host) {
                        return Err(format!(
                            "{member}: host is the BMC's IP address or host name (no scheme, port or path)"
                        ));
                    }
                    if *port == 0 {
                        return Err(format!("{member}: port is 1 to 65535 (IPMI uses 623)"));
                    }
                    if user.is_empty()
                        || user.len() > ipmi::MAX_USER
                        || !user.bytes().all(|c| c.is_ascii_graphic())
                    {
                        return Err(format!(
                            "{member}: user is 1 to {} printable characters (IPMI's limit)",
                            ipmi::MAX_USER
                        ));
                    }
                    if !valid_secret_name(secret_ref) {
                        return Err(format!(
                            "{member}: secretRef must be an environment variable name starting with {SECRET_PREFIX} (capitals, digits, underscores)"
                        ));
                    }
                    if !valid_uuid(uuid) {
                        return Err(format!(
                            "{member}: uuid is the server's system GUID (8-4-4-4-12 hexadecimal)"
                        ));
                    }
                }
                Entry::Lima { vm } => {
                    if !allow_lab {
                        return Err(format!(
                            "{member}: the lima provider is for the disposable lab: set APELORA_BMC_ALLOW_LAB=1"
                        ));
                    }
                    if !valid_vm(vm) {
                        return Err(format!(
                            "{member}: vm is a lab VM named apelora-… (never apelora-san)"
                        ));
                    }
                }
            }
            targets.insert(member, Target { entry });
        }
        Ok(Mapping {
            targets,
            ..Default::default()
        })
    }
}

/// The BMC settings from the environment: the providers, the secrets' source and how long a
/// member is Offline before it is switched off.
pub fn bmc_from_env(get: impl Fn(&str) -> Option<String>) -> Result<(Bmc, i64), String> {
    let after = match get("APELORA_BMC_FENCE_AFTER_SECS")
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
    {
        None => 30,
        Some(v) => v
            .parse::<i64>()
            .ok()
            .filter(|n| (MIN_FENCE_AFTER_SECS..=3600).contains(n))
            .ok_or_else(|| {
                format!("APELORA_BMC_FENCE_AFTER_SECS is {MIN_FENCE_AFTER_SECS} to 3600 seconds")
            })?,
    };
    let Some(path) = get("APELORA_BMC_MAP")
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
    else {
        return Ok((Bmc::Off, after));
    };
    let allow_lab = get("APELORA_BMC_ALLOW_LAB").as_deref().map(str::trim) == Some("1");
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("APELORA_BMC_MAP ({path}): {e}"))?;
    let mut mapping =
        Mapping::parse(&text, allow_lab).map_err(|e| format!("APELORA_BMC_MAP ({path}): {e}"))?;
    mapping.secrets = Some(Arc::new(|k: &str| std::env::var(k).ok()));
    if mapping
        .targets
        .values()
        .any(|t| matches!(t.entry, Entry::Maas { .. }))
    {
        let need = |k: &str| {
            get(k)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
                .ok_or_else(|| format!("a `maas` entry needs {k}"))
        };
        let key = apelora_maas_adapter::ApiKey::parse(&need("APELORA_MAAS_API_KEY")?)
            .map_err(|e| e.to_string())?;
        let ca = get("APELORA_MAAS_CA").map(PathBuf::from);
        mapping.maas = Some(
            apelora_maas_adapter::MaasClient::new(&need("APELORA_MAAS_URL")?, key, ca.as_deref())
                .map_err(|e| e.to_string())?,
        );
    }
    Ok((Bmc::Mapped(mapping), after))
}

fn http_error(e: reqwest::Error) -> String {
    // The URL is the BMC's address, never a credential; the error carries no password.
    let kind = if e.is_timeout() {
        "timed out"
    } else if e.is_connect() {
        "cannot connect"
    } else {
        "request failed"
    };
    format!("{kind}: {}", e.without_url())
}

impl Mapping {
    fn client(&self, ca: Option<&PathBuf>) -> Result<reqwest::Client, String> {
        let mut b = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none());
        if let Some(ca) = ca {
            let pem = std::fs::read(ca).map_err(|e| format!("CA file {}: {e}", ca.display()))?;
            let cert = reqwest::Certificate::from_pem(&pem)
                .map_err(|e| format!("CA file {}: {e}", ca.display()))?;
            b = b.add_root_certificate(cert);
        }
        b.build().map_err(|e| format!("http client: {e}"))
    }

    fn password(&self, secret_ref: &str) -> Result<String, String> {
        let get = self.secrets.as_ref().ok_or("no secret source")?;
        get(secret_ref).filter(|p| !p.is_empty()).ok_or_else(|| {
            format!("the BMC password is not set (environment variable {secret_ref})")
        })
    }
}

/// One Redfish session's worth of facts about the system, after the identity check.
struct Redfish<'a> {
    client: reqwest::Client,
    base: String,
    user: &'a str,
    password: String,
    system: String,
}

impl Redfish<'_> {
    async fn get(&self, path: &str) -> Result<Value, String> {
        let r = self
            .client
            .get(format!("{}{path}", self.base))
            .basic_auth(self.user, Some(&self.password))
            .send()
            .await
            .map_err(http_error)?;
        status(&r, path)?;
        r.json::<Value>()
            .await
            .map_err(|e| format!("{path}: not JSON: {}", e.without_url()))
    }

    async fn post(&self, path: &str, body: &Value) -> Result<(), String> {
        let r = self
            .client
            .post(format!("{}{path}", self.base))
            .basic_auth(self.user, Some(&self.password))
            .json(body)
            .send()
            .await
            .map_err(http_error)?;
        status(&r, path)
    }

    async fn state(&self) -> Result<PowerState, String> {
        let sys = self.get(&self.system).await?;
        Ok(match sys.get("PowerState").and_then(Value::as_str) {
            Some("On") => PowerState::On,
            Some("Off") => PowerState::Off,
            Some(other) => PowerState::Other(other.to_owned()),
            None => PowerState::Other("not reported".into()),
        })
    }
}

fn status(r: &reqwest::Response, path: &str) -> Result<(), String> {
    match r.status().as_u16() {
        200..=299 => Ok(()),
        401 | 403 => Err(format!(
            "{path}: the BMC refused the credentials ({})",
            r.status().as_u16()
        )),
        code => Err(format!("{path}: the BMC answered {code}")),
    }
}

impl Mapping {
    /// MAAS's power state of the machine, after MAAS's record of it reports the mapped hardware
    /// UUID (a wrong system id refuses instead of vouching for another server).
    async fn maas_state(
        &self,
        member: &str,
        system_id: &str,
        uuid: &str,
    ) -> Result<PowerState, String> {
        let client = self.maas.as_ref().ok_or("MAAS is not configured")?;
        let machine = client.machine(system_id).await.map_err(|e| e.to_string())?;
        match machine.hardware_uuid.as_deref() {
            Some(u) if u.eq_ignore_ascii_case(uuid) => {}
            Some(u) => {
                return Err(format!(
                    "MAAS reports hardware UUID {u:?} for {system_id}, not the mapped {uuid} for {member}: refusing, the mapping may point at another server"
                ));
            }
            None => {
                return Err(format!(
                    "MAAS has no hardware UUID for {system_id} (commission it first), so {member}'s identity cannot be checked"
                ));
            }
        }
        Ok(
            match client
                .query_power_state(system_id)
                .await
                .map_err(|e| e.to_string())?
            {
                apelora_maas_adapter::PowerState::On => PowerState::On,
                apelora_maas_adapter::PowerState::Off => PowerState::Off,
                other => PowerState::Other(format!("{other:?}").to_lowercase()),
            },
        )
    }

    /// Connects, finds the system and checks its UUID against the mapping.
    async fn redfish<'a>(&'a self, member: &str, entry: &'a Entry) -> Result<Redfish<'a>, String> {
        let Entry::Redfish {
            url,
            user,
            secret_ref,
            ca,
            system,
            uuid,
        } = entry
        else {
            return Err("not a Redfish entry".into());
        };
        let password = self.password(secret_ref)?;
        let mut base = url.trim_end_matches('/').to_owned();
        if self.plain {
            base = base.replacen("https://", "http://", 1);
        }
        let mut r = Redfish {
            client: self.client(ca.as_ref())?,
            base,
            user,
            password,
            system: String::new(),
        };
        r.system = match system {
            Some(s) => s.clone(),
            None => {
                let list = r.get("/redfish/v1/Systems").await?;
                let members: Vec<&str> = list
                    .get("Members")
                    .and_then(Value::as_array)
                    .map(|m| {
                        m.iter()
                            .filter_map(|x| x.get("@odata.id").and_then(Value::as_str))
                            .collect()
                    })
                    .unwrap_or_default();
                match members.as_slice() {
                    [one] => (*one).to_owned(),
                    [] => return Err("the BMC lists no system".into()),
                    _ => {
                        return Err(
                            "the BMC lists several systems: name one in the mapping (system)"
                                .into(),
                        );
                    }
                }
            }
        };
        if !r.system.starts_with("/redfish/v1/Systems/") || r.system.contains("..") {
            return Err(format!(
                "the BMC names an unexpected system path {:?}",
                r.system
            ));
        }
        let sys = r.get(&r.system).await?;
        let reported = sys.get("UUID").and_then(Value::as_str).unwrap_or_default();
        if !reported.eq_ignore_ascii_case(uuid) {
            return Err(format!(
                "the BMC for {member} reports system UUID {reported:?}, not the mapped {uuid}: refusing, the mapping may point at another server"
            ));
        }
        Ok(r)
    }
}

impl Mapping {
    /// Opens an IPMI session and checks the system GUID against the mapping before anything else
    /// is asked.
    async fn ipmi(&self, member: &str, entry: &Entry) -> Result<ipmi::Session, String> {
        let Entry::Ipmi {
            host,
            port,
            user,
            secret_ref,
            uuid,
        } = entry
        else {
            return Err("not an IPMI entry".into());
        };
        let password = self.password(secret_ref)?;
        let addr = tokio::time::timeout(
            REQUEST_TIMEOUT,
            tokio::net::lookup_host((host.as_str(), *port)),
        )
        .await
        .map_err(|_| format!("looking up {host} timed out"))?
        .map_err(|e| format!("cannot look up {host}: {e}"))?
        .next()
        .ok_or_else(|| format!("{host} has no address"))?;
        let mut s = ipmi::connect(addr, user, &password).await?;
        let reported = match s.guid().await {
            Ok(g) => g,
            Err(e) => {
                s.close().await;
                return Err(e);
            }
        };
        if !ipmi::guid_matches(&reported, uuid) {
            s.close().await;
            return Err(format!(
                "the BMC for {member} reports system GUID {:?}, not the mapped {uuid}: refusing, the mapping may point at another server",
                ipmi::guid_text(&reported)
            ));
        }
        tracing::debug!(member, suite = s.suite().number(), "IPMI session open");
        Ok(s)
    }
}

/// Reads the power, switches it off when it is on, and reads it back until it is off.
async fn ipmi_fence(
    s: &mut ipmi::Session,
    member: &str,
    settle: Duration,
    every: Duration,
) -> Result<bool, String> {
    let switched = s.power_on().await?;
    if switched {
        s.power_off().await?;
    }
    let deadline = tokio::time::Instant::now() + settle;
    loop {
        if !s.power_on().await? {
            return Ok(switched);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "{member} still does not report Off {} s after the power-off request",
                settle.as_secs()
            ));
        }
        tokio::time::sleep(every).await;
    }
}

async fn lima(args: &[&str]) -> Result<String, String> {
    let out = tokio::time::timeout(
        Duration::from_secs(60),
        tokio::process::Command::new("limactl").args(args).output(),
    )
    .await
    .map_err(|_| "limactl timed out".to_owned())?
    .map_err(|e| format!("limactl: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "limactl {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

impl Bmc {
    pub fn configured(&self) -> bool {
        !matches!(self, Bmc::Off)
    }

    /// Whether the member is in the allowlist.
    pub fn covers(&self, member: &str) -> bool {
        match self {
            Bmc::Off => false,
            Bmc::Mapped(m) => m.targets.contains_key(member),
            Bmc::Fixed(f) => f
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .0
                .contains_key(member),
        }
    }

    /// The provider's name for the evidence (`redfish`, `lima`, `fixed`).
    pub fn provider_of(&self, member: &str) -> String {
        match self {
            Bmc::Mapped(m) => match m.targets.get(member).map(|t| &t.entry) {
                Some(Entry::Redfish { .. }) => "redfish",
                Some(Entry::Maas { .. }) => "maas",
                Some(Entry::Ipmi { .. }) => "ipmi",
                Some(Entry::Lima { .. }) => "lima",
                None => "unknown",
            },
            _ => "fixed",
        }
        .to_owned()
    }

    /// What the provider addresses for the member (a Redfish system path or its configured
    /// override, a MAAS system id, an IPMI system GUID, a lab VM name), for the evidence. Never
    /// the BMC's address.
    pub fn system_of(&self, member: &str) -> String {
        match self {
            Bmc::Mapped(m) => match m.targets.get(member).map(|t| &t.entry) {
                Some(Entry::Redfish { system, .. }) => system
                    .clone()
                    .unwrap_or_else(|| "/redfish/v1/Systems".into()),
                Some(Entry::Maas { system_id, .. }) => system_id.clone(),
                Some(Entry::Ipmi { uuid, .. }) => uuid.to_lowercase(),
                Some(Entry::Lima { vm }) => vm.clone(),
                None => String::new(),
            },
            _ => member.to_owned(),
        }
    }

    /// The members the allowlist names.
    pub fn members(&self) -> Vec<String> {
        match self {
            Bmc::Off => vec![],
            Bmc::Mapped(m) => m.targets.keys().cloned().collect(),
            Bmc::Fixed(f) => f
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .0
                .keys()
                .cloned()
                .collect(),
        }
    }

    /// Reads the member's power state from its BMC (identity checked first).
    pub async fn power_state(&self, member: &str) -> Result<PowerState, String> {
        match self {
            Bmc::Off => Err("no BMC is configured".into()),
            Bmc::Fixed(f) => {
                let mut g = f.lock().unwrap_or_else(|e| e.into_inner());
                g.1.push(("read".into(), member.to_owned()));
                g.0.get(member)
                    .map(|m| {
                        if m.on {
                            PowerState::On
                        } else {
                            PowerState::Off
                        }
                    })
                    .ok_or_else(|| format!("{member} is not in the BMC mapping"))
            }
            Bmc::Mapped(m) => {
                let t = m
                    .targets
                    .get(member)
                    .ok_or_else(|| format!("{member} is not in the BMC mapping"))?;
                match &t.entry {
                    Entry::Redfish { .. } => m.redfish(member, &t.entry).await?.state().await,
                    Entry::Maas { system_id, uuid } => m.maas_state(member, system_id, uuid).await,
                    Entry::Ipmi { .. } => {
                        let mut s = m.ipmi(member, &t.entry).await?;
                        let on = s.power_on().await;
                        s.close().await;
                        Ok(if on? { PowerState::On } else { PowerState::Off })
                    }
                    Entry::Lima { vm } => Ok(
                        match lima(&["list", "--format", "{{.Status}}", vm])
                            .await?
                            .as_str()
                        {
                            "Running" => PowerState::On,
                            "Stopped" => PowerState::Off,
                            other => PowerState::Other(other.to_owned()),
                        },
                    ),
                }
            }
        }
    }

    /// Switches the member off and reads the state back until it is Off, or fails. When it is
    /// already Off nothing is sent. The reading is the evidence; a request that was merely
    /// accepted is not.
    pub async fn fence(&self, member: &str) -> Result<PowerOffReading, String> {
        match self {
            Bmc::Off => Err("no BMC is configured".into()),
            Bmc::Fixed(f) => {
                let (before, refuse, stays) = {
                    let mut g = f.lock().unwrap_or_else(|e| e.into_inner());
                    g.1.push(("read".into(), member.to_owned()));
                    let m =
                        g.0.get(member)
                            .ok_or_else(|| format!("{member} is not in the BMC mapping"))?
                            .clone();
                    (m.on, m.refuse, m.stays_on)
                };
                if before {
                    let mut g = f.lock().unwrap_or_else(|e| e.into_inner());
                    g.1.push(("off".into(), member.to_owned()));
                    if let Some(why) = refuse {
                        return Err(why);
                    }
                    if stays {
                        return Err(format!(
                            "{member} still reports On after the power-off request"
                        ));
                    }
                    if let Some(m) = g.0.get_mut(member) {
                        m.on = false;
                    }
                }
                Ok(PowerOffReading {
                    provider: "fixed".into(),
                    system: member.to_owned(),
                    at: now(),
                    switched_off: before,
                })
            }
            Bmc::Mapped(m) => {
                let t = m
                    .targets
                    .get(member)
                    .ok_or_else(|| format!("{member} is not in the BMC mapping"))?;
                let (settle, every) = m.settle.unwrap_or((SETTLE_BUDGET, SETTLE_EVERY));
                match &t.entry {
                    Entry::Redfish { .. } => {
                        let r = m.redfish(member, &t.entry).await?;
                        let before = r.state().await?;
                        let mut switched = false;
                        if before != PowerState::Off {
                            let sys = r.get(&r.system).await?;
                            let action = sys.pointer("/Actions/#ComputerSystem.Reset");
                            let allowed = action
                                .and_then(|a| a.get("ResetType@Redfish.AllowableValues"))
                                .and_then(Value::as_array);
                            if allowed.is_some_and(|a| !a.iter().any(|v| v == "ForceOff")) {
                                return Err(
                                    "the BMC does not offer ForceOff for this system".into()
                                );
                            }
                            let target = action
                                .and_then(|a| a.get("target"))
                                .and_then(Value::as_str)
                                .filter(|p| p.starts_with(&r.system) && !p.contains(".."))
                                .map_or_else(
                                    || format!("{}/Actions/ComputerSystem.Reset", r.system),
                                    str::to_owned,
                                );
                            r.post(&target, &serde_json::json!({ "ResetType": "ForceOff" }))
                                .await?;
                            switched = true;
                        }
                        let deadline = tokio::time::Instant::now() + settle;
                        loop {
                            if r.state().await? == PowerState::Off {
                                return Ok(PowerOffReading {
                                    provider: "redfish".into(),
                                    system: r.system.clone(),
                                    at: now(),
                                    switched_off: switched,
                                });
                            }
                            if tokio::time::Instant::now() >= deadline {
                                return Err(format!(
                                    "{member} still does not report Off {} s after the power-off request",
                                    settle.as_secs()
                                ));
                            }
                            tokio::time::sleep(every).await;
                        }
                    }
                    Entry::Maas { system_id, uuid } => {
                        let client = m.maas.as_ref().ok_or("MAAS is not configured")?;
                        let before = m.maas_state(member, system_id, uuid).await?;
                        let switched = before != PowerState::Off;
                        if switched {
                            client
                                .power_off(system_id)
                                .await
                                .map_err(|e| e.to_string())?;
                        }
                        let deadline = tokio::time::Instant::now() + settle;
                        loop {
                            if m.maas_state(member, system_id, uuid).await? == PowerState::Off {
                                return Ok(PowerOffReading {
                                    provider: "maas".into(),
                                    system: system_id.clone(),
                                    at: now(),
                                    switched_off: switched,
                                });
                            }
                            if tokio::time::Instant::now() >= deadline {
                                return Err(format!(
                                    "MAAS still does not report {member} Off {} s after the power-off request",
                                    settle.as_secs()
                                ));
                            }
                            tokio::time::sleep(every).await;
                        }
                    }
                    Entry::Ipmi { uuid, .. } => {
                        let mut s = m.ipmi(member, &t.entry).await?;
                        let out = ipmi_fence(&mut s, member, settle, every).await;
                        s.close().await;
                        Ok(PowerOffReading {
                            provider: "ipmi".into(),
                            system: uuid.to_lowercase(),
                            at: now(),
                            switched_off: out?,
                        })
                    }
                    Entry::Lima { vm } => {
                        let before = self.power_state(member).await?;
                        let switched = before != PowerState::Off;
                        if switched {
                            lima(&["stop", "--force", vm]).await?;
                        }
                        let deadline = tokio::time::Instant::now() + settle;
                        loop {
                            if self.power_state(member).await? == PowerState::Off {
                                return Ok(PowerOffReading {
                                    provider: "lima".into(),
                                    system: vm.clone(),
                                    at: now(),
                                    switched_off: switched,
                                });
                            }
                            if tokio::time::Instant::now() >= deadline {
                                return Err(format!(
                                    "{vm} is still not stopped {} s after the request",
                                    settle.as_secs()
                                ));
                            }
                            tokio::time::sleep(every).await;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;

// SPDX-License-Identifier: AGPL-3.0-only
//! A client of the MAAS REST API for Apelora (ADR 0002, `docs/design/provisioner-maas.md`):
//! discover the machines MAAS manages, read one machine's identity and power state, and switch
//! one off. Nothing else is called.
//!
//! Authentication is MAAS's OAuth 1.0 `PLAINTEXT` signature with an API key of the form
//! `consumer:token:secret`. The key is held in a type that prints as `[redacted]`, is sent only
//! in the `Authorization` header, and never appears in an error or a log line. HTTPS is required
//! (verified against the system roots or a CA file); plain HTTP exists only for tests on loopback.

use std::fmt;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How long one request may take.
const TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum MaasError {
    /// The configuration is unusable (named, without any secret).
    #[error("invalid MAAS configuration: {0}")]
    Invalid(String),
    /// MAAS could not be reached, or the request timed out.
    #[error("MAAS is unreachable: {0}")]
    Unreachable(String),
    /// MAAS answered with an error status; `what` is the call.
    #[error("MAAS refused {what}: {status}")]
    Refused { status: u16, what: String },
    /// MAAS answered with something that is not what its API documents.
    #[error("MAAS answered unexpectedly: {0}")]
    Decode(String),
}

/// The API key. Prints as `[redacted]`; the parts are used only to sign requests.
#[derive(Clone)]
pub struct ApiKey {
    consumer: String,
    token: String,
    secret: String,
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey([redacted])")
    }
}

impl ApiKey {
    /// `consumer:token:secret`, as MAAS shows an API key.
    pub fn parse(key: &str) -> Result<ApiKey, MaasError> {
        let mut parts = key.trim().splitn(3, ':');
        match (parts.next(), parts.next(), parts.next()) {
            (Some(c), Some(t), Some(s))
                if !c.is_empty()
                    && !t.is_empty()
                    && !s.is_empty()
                    && key.chars().all(|ch| ch.is_ascii_graphic()) =>
            {
                Ok(ApiKey {
                    consumer: c.to_owned(),
                    token: t.to_owned(),
                    secret: s.to_owned(),
                })
            }
            _ => Err(MaasError::Invalid(
                "the API key is three parts, consumer:token:secret".into(),
            )),
        }
    }

    /// The `Authorization` header value for one request (PLAINTEXT signature: an empty
    /// consumer secret, `&`, the token secret, percent-encoded).
    fn header(&self, nonce: &str, timestamp: i64) -> String {
        format!(
            "OAuth realm=\"\", oauth_version=\"1.0\", oauth_signature_method=\"PLAINTEXT\", oauth_consumer_key=\"{}\", oauth_token=\"{}\", oauth_signature=\"%26{}\", oauth_nonce=\"{nonce}\", oauth_timestamp=\"{timestamp}\"",
            encode(&self.consumer),
            encode(&self.token),
            encode(&self.secret),
        )
    }
}

/// RFC 3986 percent-encoding of everything but the unreserved characters.
fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// What MAAS's last power check of a machine says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PowerState {
    On,
    Off,
    /// MAAS cannot tell (manual power, or no BMC reachable): never evidence.
    Unknown,
    Error,
}

impl PowerState {
    fn parse(s: &str) -> PowerState {
        match s {
            "on" => PowerState::On,
            "off" => PowerState::Off,
            "error" => PowerState::Error,
            _ => PowerState::Unknown,
        }
    }
}

/// One machine as MAAS reports it (only what Apelora uses).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Machine {
    pub system_id: String,
    pub hostname: String,
    /// `New`, `Commissioning`, `Ready`, `Allocated`, `Deploying`, `Deployed`, `Failed …`, …
    pub status: String,
    pub power_state: PowerState,
    /// How MAAS switches it: `ipmi`, `redfish`, `manual`, `virsh`, …
    pub power_type: String,
    /// `amd64/generic`, `arm64/generic`, …
    pub architecture: String,
    pub cpu_count: u32,
    pub memory_mib: u64,
    pub os: String,
    pub addresses: Vec<String>,
    /// The server's hardware UUID, when MAAS has commissioned it.
    pub hardware_uuid: Option<String>,
    pub zone: String,
    pub pool: String,
    pub owner: Option<String>,
}

fn text(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn named(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(|o| o.get("name"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// A MAAS system id: lowercase letters and digits.
fn valid_system_id(id: &str) -> bool {
    (3..=16).contains(&id.len())
        && id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

impl Machine {
    fn from_json(v: &Value) -> Result<Machine, MaasError> {
        let system_id = text(v, "system_id");
        if !valid_system_id(&system_id) {
            return Err(MaasError::Decode(
                "a machine without a valid system_id".into(),
            ));
        }
        let os = match (text(v, "osystem"), text(v, "distro_series")) {
            (o, d) if o.is_empty() && d.is_empty() => String::new(),
            (o, d) => format!("{o} {d}").trim().to_owned(),
        };
        Ok(Machine {
            hostname: text(v, "hostname"),
            status: text(v, "status_name"),
            power_state: PowerState::parse(&text(v, "power_state")),
            power_type: text(v, "power_type"),
            architecture: text(v, "architecture"),
            cpu_count: v
                .get("cpu_count")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                .try_into()
                .unwrap_or(u32::MAX),
            memory_mib: v.get("memory").and_then(Value::as_u64).unwrap_or(0),
            os,
            addresses: v
                .get("ip_addresses")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
            hardware_uuid: v
                .get("hardware_uuid")
                .and_then(Value::as_str)
                .filter(|u| !u.is_empty())
                .map(str::to_owned),
            zone: named(v, "zone"),
            pool: named(v, "pool"),
            owner: v
                .get("owner")
                .and_then(Value::as_str)
                .filter(|o| !o.is_empty())
                .map(str::to_owned),
            system_id,
        })
    }
}

#[derive(Clone)]
pub struct MaasClient {
    base: String,
    key: ApiKey,
    http: reqwest::Client,
}

impl fmt::Debug for MaasClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MaasClient")
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

impl MaasClient {
    /// `url` is the region's address (`https://maas.example:5240/MAAS`); `ca` an optional PEM file
    /// of the authority that signed its certificate.
    pub fn new(url: &str, key: ApiKey, ca: Option<&Path>) -> Result<MaasClient, MaasError> {
        let parsed =
            reqwest::Url::parse(url).map_err(|e| MaasError::Invalid(format!("url: {e}")))?;
        if parsed.scheme() != "https" || parsed.host_str().is_none() {
            return Err(MaasError::Invalid(
                "the MAAS url must be https://host[:port][/MAAS]".into(),
            ));
        }
        if !parsed.username().is_empty() || parsed.password().is_some() || parsed.query().is_some()
        {
            return Err(MaasError::Invalid(
                "the MAAS url carries no credentials or query".into(),
            ));
        }
        let mut b = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .redirect(reqwest::redirect::Policy::none());
        if let Some(ca) = ca {
            let pem = std::fs::read(ca)
                .map_err(|e| MaasError::Invalid(format!("CA file {}: {e}", ca.display())))?;
            let cert = reqwest::Certificate::from_pem(&pem)
                .map_err(|e| MaasError::Invalid(format!("CA file {}: {e}", ca.display())))?;
            b = b.add_root_certificate(cert);
        }
        let http = b
            .build()
            .map_err(|e| MaasError::Invalid(format!("http client: {e}")))?;
        Ok(MaasClient {
            base: parsed.as_str().trim_end_matches('/').to_owned(),
            key,
            http,
        })
    }

    /// Plain HTTP to a loopback test server. Tests only.
    pub fn plain_for_tests(base: &str, key: ApiKey) -> MaasClient {
        MaasClient {
            base: base.trim_end_matches('/').to_owned(),
            key,
            http: reqwest::Client::new(),
        }
    }

    fn authorization(&self) -> String {
        let ts = time::OffsetDateTime::now_utc().unix_timestamp();
        self.key
            .header(&uuid::Uuid::new_v4().simple().to_string(), ts)
    }

    async fn call(
        &self,
        post: bool,
        path: &str,
        form: &[(&str, &str)],
    ) -> Result<Value, MaasError> {
        let url = format!("{}/api/2.0/{path}", self.base);
        let mut r = if post {
            self.http.post(&url)
        } else {
            self.http.get(&url)
        };
        r = r
            .header("Authorization", self.authorization())
            .header("Accept", "application/json");
        if post {
            let body = form
                .iter()
                .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            r = r
                .header("Content-Type", "application/x-www-form-urlencoded")
                .body(body);
        }
        let resp = r.send().await.map_err(|e| {
            let kind = if e.is_timeout() {
                "timed out"
            } else if e.is_connect() {
                "cannot connect"
            } else {
                "request failed"
            };
            MaasError::Unreachable(format!("{kind}: {}", e.without_url()))
        })?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(MaasError::Refused {
                status,
                what: path.trim_end_matches('/').to_owned(),
            });
        }
        resp.json::<Value>()
            .await
            .map_err(|e| MaasError::Decode(format!("{path}: not JSON: {}", e.without_url())))
    }

    /// Every machine MAAS manages.
    pub async fn machines(&self) -> Result<Vec<Machine>, MaasError> {
        let v = self.call(false, "machines/", &[]).await?;
        v.as_array()
            .ok_or_else(|| MaasError::Decode("machines/ is not a list".into()))?
            .iter()
            .map(Machine::from_json)
            .collect()
    }

    /// One machine, with its hardware UUID.
    pub async fn machine(&self, system_id: &str) -> Result<Machine, MaasError> {
        check_id(system_id)?;
        Machine::from_json(
            &self
                .call(false, &format!("machines/{system_id}/"), &[])
                .await?,
        )
    }

    /// What the machine's BMC says now (MAAS asks it; this is not a cached value).
    pub async fn query_power_state(&self, system_id: &str) -> Result<PowerState, MaasError> {
        check_id(system_id)?;
        let v = self
            .call(
                true,
                &format!("machines/{system_id}/op-query_power_state"),
                &[],
            )
            .await?;
        let state = v
            .get("state")
            .and_then(Value::as_str)
            .ok_or_else(|| MaasError::Decode("no power state".into()))?;
        Ok(PowerState::parse(state))
    }

    /// Asks MAAS to switch the machine off at once (`stop_mode=hard`: no waiting for the
    /// operating system). The answer says only that MAAS accepted it; read the state back.
    pub async fn power_off(&self, system_id: &str) -> Result<(), MaasError> {
        check_id(system_id)?;
        self.call(
            true,
            &format!("machines/{system_id}/op-power_off"),
            &[("stop_mode", "hard")],
        )
        .await?;
        Ok(())
    }

    /// Starts commissioning (inventory and testing). MAAS's answer says it accepted the
    /// request; the machine's status is read afterwards.
    pub async fn commission(&self, system_id: &str) -> Result<(), MaasError> {
        check_id(system_id)?;
        self.call(true, &format!("machines/{system_id}/op-commission"), &[])
            .await?;
        Ok(())
    }

    /// Asks MAAS to install `distro_series` and run `user_data` (cloud-init, sent base64-encoded,
    /// the form MAAS documents). The text is not logged. Empty, or larger than 64 KiB, is refused
    /// before anything is sent.
    pub async fn deploy(
        &self,
        system_id: &str,
        distro_series: &str,
        user_data: &str,
    ) -> Result<(), MaasError> {
        check_id(system_id)?;
        if distro_series.is_empty()
            || distro_series.len() > 64
            || !distro_series
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        {
            return Err(MaasError::Invalid(
                "distro series is a short name such as noble".into(),
            ));
        }
        if user_data.trim().is_empty() {
            return Err(MaasError::Invalid("cloud-init is empty".into()));
        }
        if user_data.len() > 64 * 1024 {
            return Err(MaasError::Invalid(
                "cloud-init is larger than 64 KiB".into(),
            ));
        }
        let encoded = base64(user_data.as_bytes());
        self.call(
            true,
            &format!("machines/{system_id}/op-deploy"),
            &[("distro_series", distro_series), ("user_data", &encoded)],
        )
        .await?;
        Ok(())
    }

    /// Returns the machine to the pool. `erase` is sent as MAAS's `erase` flag and is false
    /// unless the caller asked. A secure wipe is not offered.
    pub async fn release(&self, system_id: &str, erase: bool) -> Result<(), MaasError> {
        check_id(system_id)?;
        let flag = if erase { "true" } else { "false" };
        self.call(
            true,
            &format!("machines/{system_id}/op-release"),
            &[("erase", flag)],
        )
        .await?;
        Ok(())
    }
}

/// Base64 (the standard alphabet, padded). MAAS takes cloud-init that way.
fn base64(data: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        let n = (u32::from(data[i]) << 16) | (u32::from(data[i + 1]) << 8) | u32::from(data[i + 2]);
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(T[((n >> 6) & 63) as usize] as char);
        out.push(T[(n & 63) as usize] as char);
        i += 3;
    }
    let rest = data.len() - i;
    if rest == 1 {
        let n = u32::from(data[i]) << 16;
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push('=');
        out.push('=');
    } else if rest == 2 {
        let n = (u32::from(data[i]) << 16) | (u32::from(data[i + 1]) << 8);
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(T[((n >> 6) & 63) as usize] as char);
        out.push('=');
    }
    out
}

fn check_id(id: &str) -> Result<(), MaasError> {
    if valid_system_id(id) {
        Ok(())
    } else {
        Err(MaasError::Invalid(format!(
            "{id:?} is not a MAAS system id"
        )))
    }
}

#[cfg(test)]
mod tests;

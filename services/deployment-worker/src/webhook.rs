// SPDX-License-Identifier: AGPL-3.0-only
//! Webhook delivery of alert notifications (docs/design/alerts.md §2.4).
//!
//! The evaluator writes one outbox message (topic `alert`) per channel; the outbox relay hands
//! it to [`WebhookSink`] through [`Routed`]. The sink looks the channel up, signs the body with
//! the secret its reference names and POSTs it:
//! - HTTPS only (checked again here, whatever the database says), TLS verified against the
//!   system trust store, no redirects, 3 s to connect and 5 s in all;
//! - never to this computer, link-local (where cloud metadata services live), unspecified or
//!   multicast addresses: the receiver's name is resolved here, every address it gives is
//!   checked ([`forbidden_address`]), and the request connects to exactly those addresses (no
//!   second lookup). Private ranges (RFC 1918, IPv6 unique-local) stay allowed: on-premises chat
//!   servers live there. Behind a proxy (`HTTPS_PROXY`) the proxy connects, after the same
//!   check of the name;
//! - `Apelora-Event-Id` (the outbox message id: receivers drop duplicates by it),
//!   `Apelora-Timestamp` (Unix seconds) and `Apelora-Signature: v1=<hex>`, an HMAC-SHA256 with
//!   the channel's secret over `<timestamp>.<body>`;
//! - the secret is read from the environment variable the channel's `secretRef` names (always
//!   `APELORA_ALERT_SECRET_…`) at delivery time; it is never stored, logged or returned.
//!
//! A failure (receiver down, non-2xx, a missing secret) is returned to the relay, which retries
//! with its backoff and keeps the reason on the message, where the channel's status reads it.
//! That reason is coarse (could not connect, TLS failed, the HTTP status), so a channel cannot be
//! used to probe what answers where; the full error goes to the worker's log. Error texts never
//! carry the URL (it may hold a token) or the response body.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use apelora_persistence::alerts::{self as db, ALERT_TOPIC, SECRET_REF_PREFIX};
use apelora_persistence::outbox::Message;
use apelora_persistence::{Pool, StoreError};
use hmac::{Hmac, KeyInit, Mac};
use serde_json::Value;
use sha2::Sha256;
use uuid::Uuid;

use crate::outbox::{Delivery, Sink};

/// Time to connect, and for the whole request.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const TIMEOUT: Duration = Duration::from_secs(5);

/// Reads a secret by its reference (an environment variable name).
pub type SecretSource = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Starts the HTTP client for one delivery (the system trust store; tests trust their own
/// certificate). [`WebhookSink::settings`] and the checked addresses are added to it.
pub type ClientSource = Arc<dyn Fn() -> reqwest::ClientBuilder + Send + Sync>;

/// Addresses a webhook is never sent to: this computer (loopback, and `0.0.0.0/8`, which reaches
/// it), link-local (`169.254.0.0/16`, `fe80::/10`: cloud metadata services), unspecified,
/// multicast and broadcast; IPv4-mapped IPv6 addresses count as their IPv4 address. Private
/// ranges stay allowed.
pub fn forbidden_address(ip: IpAddr) -> bool {
    let ip = match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        v4 => v4,
    };
    match ip {
        IpAddr::V4(a) => {
            a.is_loopback()
                || a.is_link_local()
                || a.octets()[0] == 0
                || a.is_multicast()
                || a.is_broadcast()
        }
        IpAddr::V6(a) => {
            a.is_loopback()
                || a.is_unspecified()
                || a.is_multicast()
                || (a.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

/// The full cause chain of an error, for the log.
fn chain(e: &dyn std::error::Error) -> String {
    let mut out = e.to_string();
    let mut cur = e.source();
    while let Some(c) = cur {
        out.push_str(": ");
        out.push_str(&c.to_string());
        cur = c.source();
    }
    out
}

/// Did a connection fail in TLS (an untrusted certificate, a failed handshake) rather than in
/// reaching the receiver? The TLS stack reports its errors as `InvalidData` I/O errors.
fn tls_failed(e: &(dyn std::error::Error + 'static)) -> bool {
    let mut cur = Some(e);
    while let Some(c) = cur {
        if c.downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::InvalidData)
        {
            return true;
        }
        let text = c.to_string().to_lowercase();
        if text.contains("certificate") || text.contains("tls") || text.contains("handshake") {
            return true;
        }
        cur = c.source();
    }
    false
}

/// The reason the channel's status shows for a request that got no answer: coarse on purpose.
fn coarse(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "the receiver did not answer within 5 s".into()
    } else if e.is_connect() && tls_failed(e) {
        "TLS with the receiver failed (its certificate was not accepted, or the handshake failed)"
            .into()
    } else if e.is_connect() {
        "could not connect to the receiver".into()
    } else {
        "the request to the receiver failed".into()
    }
}

/// `v1=<hex>`: HMAC-SHA256 with `secret` over `<timestamp>.<body>`.
pub fn signature(secret: &[u8], timestamp: i64, body: &[u8]) -> String {
    let mut mac =
        <Hmac<Sha256> as KeyInit>::new_from_slice(secret).expect("HMAC takes keys of any length");
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    let bytes = mac.finalize().into_bytes();
    let mut out = String::with_capacity(3 + bytes.len() * 2);
    out.push_str("v1=");
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Is `url` one a webhook may be sent to? HTTPS with a host, no user name or password in it.
pub fn check_url(url: &str) -> Result<(), String> {
    let u = reqwest::Url::parse(url).map_err(|_| "the URL is not valid".to_owned())?;
    if u.scheme() != "https" {
        return Err("webhooks are sent over HTTPS only (the URL must start with https://)".into());
    }
    if u.host_str().is_none_or(str::is_empty) {
        return Err("the URL has no host".into());
    }
    if !u.username().is_empty() || u.password().is_some() {
        return Err(
            "the URL must not carry a user name or password; sign with the secret instead".into(),
        );
    }
    Ok(())
}

/// Is `name` a secret reference a channel may use?
pub fn check_secret_ref(name: &str) -> Result<(), String> {
    let rest = name.strip_prefix(SECRET_REF_PREFIX).unwrap_or("");
    if !rest.is_empty()
        && rest.len() <= 64
        && rest
            .bytes()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
    {
        Ok(())
    } else {
        Err(format!(
            "the secret reference must be the name of an environment variable on the worker that starts with {SECRET_REF_PREFIX} (then up to 64 capital letters, digits or _), e.g. {SECRET_REF_PREFIX}OPS"
        ))
    }
}

/// The body a receiver gets: the message without its routing (`channel`), with its id.
pub fn body_of(m: &Message) -> Value {
    let mut body = m.payload.clone();
    if let Some(o) = body.as_object_mut() {
        o.remove("channel");
        o.insert("eventId".into(), Value::from(m.id));
    }
    body
}

/// Delivers `alert` messages to webhook channels.
pub struct WebhookSink {
    pool: Pool,
    clients: ClientSource,
    secrets: SecretSource,
    /// Tests only: their receiver listens on this computer.
    allow_loopback: bool,
}

/// The reason given when a receiver's address is refused.
const ADDRESS_REFUSED: &str = "the receiver's address is not allowed: webhooks are not sent to this computer, link-local, unspecified, multicast or broadcast addresses";

impl WebhookSink {
    /// With the system trust store and secrets from the process environment. Builds a client
    /// once, so a worker whose TLS set-up is broken says so when it starts.
    pub fn new(pool: Pool) -> Result<Self, String> {
        Self::settings(reqwest::Client::builder())
            .build()
            .map_err(|e| format!("cannot build the webhook client: {e}"))?;
        Ok(Self::with(
            pool,
            Arc::new(reqwest::Client::builder),
            Arc::new(|k: &str| std::env::var(k).ok()),
        ))
    }

    /// The settings every webhook request uses: timeouts and no redirects.
    pub fn settings(b: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
        b.connect_timeout(CONNECT_TIMEOUT)
            .timeout(TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("apelora-alerts/", env!("CARGO_PKG_VERSION")))
    }

    pub fn with(pool: Pool, clients: ClientSource, secrets: SecretSource) -> Self {
        WebhookSink {
            pool,
            clients,
            secrets,
            allow_loopback: false,
        }
    }

    /// Lets deliveries reach this computer, where a test's receiver listens. Never in shipping
    /// code.
    #[cfg(test)]
    pub(crate) fn allowing_loopback(mut self) -> Self {
        self.allow_loopback = true;
        self
    }

    fn allowed(&self, ip: IpAddr) -> bool {
        !forbidden_address(ip) || (self.allow_loopback && ip.is_loopback())
    }

    /// The addresses the URL's host stands for, every one checked. `Some((host, addresses))` for
    /// a name (the request then connects to exactly these), `None` for an address written in the
    /// URL.
    async fn checked_addresses(
        &self,
        url: &reqwest::Url,
        channel: &str,
    ) -> Result<Option<(String, Vec<SocketAddr>)>, String> {
        let host = url.host_str().ok_or("the URL has no host")?;
        let port = url.port_or_known_default().unwrap_or(443);
        let literal = host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            .ok();
        let addrs: Vec<SocketAddr> = match literal {
            Some(ip) => vec![SocketAddr::new(ip, port)],
            None => {
                let looked_up =
                    tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::lookup_host((host, port)))
                        .await
                        .map_err(|_| "could not resolve the receiver's name in time".to_owned())?;
                match looked_up {
                    Ok(found) => found.collect(),
                    Err(e) => {
                        tracing::warn!(channel, error = %e, "alert: the webhook receiver's name did not resolve");
                        return Err("could not resolve the receiver's name".into());
                    }
                }
            }
        };
        if addrs.is_empty() {
            return Err("could not resolve the receiver's name".into());
        }
        if let Some(a) = addrs.iter().find(|a| !self.allowed(a.ip())) {
            tracing::warn!(channel, address = %a.ip(), "alert: webhook receiver address refused");
            return Err(ADDRESS_REFUSED.into());
        }
        Ok(literal.is_none().then(|| (host.to_owned(), addrs)))
    }

    async fn send(&self, m: &Message) -> Result<(), String> {
        let Some(id) = m.payload["channel"]
            .as_str()
            .and_then(|s| Uuid::parse_str(s).ok())
        else {
            tracing::warn!(
                id = m.id,
                "alert: a notification without a channel; dropped"
            );
            return Ok(());
        };
        let channel = match db::channel(&self.pool, id).await {
            Ok(c) => c,
            Err(StoreError::NotFound(_)) => {
                tracing::info!(id = m.id, channel = %id, "alert: the channel was deleted; notification dropped");
                return Ok(());
            }
            Err(e) => return Err(format!("the channel could not be read: {e}")),
        };
        if !channel.enabled {
            tracing::info!(id = m.id, channel = %channel.name, "alert: the channel is turned off; notification dropped");
            return Ok(());
        }
        check_url(&channel.url)?;
        check_secret_ref(&channel.secret_ref)?;
        let secret = (self.secrets)(&channel.secret_ref)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("secret {} is not set on the worker", channel.secret_ref))?;
        let url = reqwest::Url::parse(&channel.url).map_err(|_| "the URL is not valid")?;
        let mut builder = Self::settings((self.clients)());
        if let Some((host, addrs)) = self.checked_addresses(&url, &channel.name).await? {
            builder = builder.resolve_to_addrs(&host, &addrs);
        }
        let client = builder.build().map_err(|e| {
            tracing::warn!(channel = %channel.name, error = %e, "alert: the webhook client could not be built");
            "the webhook client could not be built".to_owned()
        })?;
        let body = serde_json::to_vec(&body_of(m))
            .map_err(|e| format!("the message could not be encoded: {e}"))?;
        let timestamp = time::OffsetDateTime::now_utc().unix_timestamp();
        let res = client
            .post(url)
            .header("content-type", "application/json")
            .header("apelora-event-id", m.id.to_string())
            .header("apelora-timestamp", timestamp.to_string())
            .header(
                "apelora-signature",
                signature(secret.as_bytes(), timestamp, &body),
            )
            .body(body)
            .send()
            .await
            .map_err(|e| {
                let e = e.without_url();
                tracing::warn!(id = m.id, channel = %channel.name, error = %chain(&e), "alert: webhook delivery failed");
                coarse(&e)
            })?;
        let status = res.status();
        if status.is_success() {
            tracing::info!(id = m.id, channel = %channel.name, status = status.as_u16(), "alert: notification delivered");
            Ok(())
        } else if status.is_redirection() {
            Err(format!(
                "the receiver answered {} (a redirect; webhooks do not follow redirects)",
                status.as_u16()
            ))
        } else {
            Err(format!("the receiver answered {}", status.as_u16()))
        }
    }
}

impl Sink for WebhookSink {
    fn name(&self) -> &'static str {
        "webhook"
    }

    fn deliver<'a>(&'a self, m: &'a Message) -> Delivery<'a> {
        Box::pin(self.send(m))
    }
}

/// Sends `alert` messages to the webhook sink and everything else to `other`.
pub struct Routed {
    pub alerts: Arc<dyn Sink>,
    pub other: Arc<dyn Sink>,
}

impl Sink for Routed {
    fn name(&self) -> &'static str {
        self.other.name()
    }

    fn deliver<'a>(&'a self, m: &'a Message) -> Delivery<'a> {
        if m.topic == ALERT_TOPIC {
            self.alerts.deliver(m)
        } else {
            self.other.deliver(m)
        }
    }
}

#[cfg(test)]
mod tests;

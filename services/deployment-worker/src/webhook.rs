// SPDX-License-Identifier: AGPL-3.0-only
//! Webhook delivery of alert notifications (docs/design/alerts.md §2.4).
//!
//! The evaluator writes one outbox message (topic `alert`) per channel; the outbox relay hands
//! it to [`WebhookSink`] through [`Routed`]. The sink looks the channel up, signs the body with
//! the secret its reference names and POSTs it:
//! - HTTPS only (checked again here, whatever the database says), TLS verified against the
//!   system trust store, no redirects, 3 s to connect and 5 s in all;
//! - `Apelora-Event-Id` (the outbox message id: receivers drop duplicates by it),
//!   `Apelora-Timestamp` (Unix seconds) and `Apelora-Signature: v1=<hex>`, an HMAC-SHA256 with
//!   the channel's secret over `<timestamp>.<body>`;
//! - the secret is read from the environment variable the channel's `secretRef` names (always
//!   `APELORA_ALERT_SECRET_…`) at delivery time; it is never stored, logged or returned.
//!
//! A failure (receiver down, non-2xx, a missing secret) is returned to the relay, which retries
//! with its backoff and keeps the reason on the message, where the channel's status reads it.
//! Error texts never carry the URL (it may hold a token) or the response body.

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
    client: reqwest::Client,
    secrets: SecretSource,
}

impl WebhookSink {
    /// With the system trust store and secrets from the process environment.
    pub fn new(pool: Pool) -> Result<Self, String> {
        let client = Self::client(reqwest::Client::builder())?;
        Ok(Self::with(
            pool,
            client,
            Arc::new(|k: &str| std::env::var(k).ok()),
        ))
    }

    /// The client settings every webhook uses: timeouts and no redirects.
    pub fn client(b: reqwest::ClientBuilder) -> Result<reqwest::Client, String> {
        b.connect_timeout(CONNECT_TIMEOUT)
            .timeout(TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("apelora-alerts/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| format!("cannot build the webhook client: {e}"))
    }

    pub fn with(pool: Pool, client: reqwest::Client, secrets: SecretSource) -> Self {
        WebhookSink {
            pool,
            client,
            secrets,
        }
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
        let body = serde_json::to_vec(&body_of(m))
            .map_err(|e| format!("the message could not be encoded: {e}"))?;
        let timestamp = time::OffsetDateTime::now_utc().unix_timestamp();
        let res = self
            .client
            .post(&channel.url)
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
                if e.is_timeout() {
                    "the receiver did not answer within 5 s".to_owned()
                } else if e.is_connect() {
                    format!("could not connect to the receiver: {e}")
                } else {
                    format!("the request failed: {e}")
                }
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

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
mod tests {
    use std::sync::Mutex;

    use apelora_persistence::outbox;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    #[test]
    fn the_signature_is_hmac_sha256_over_timestamp_and_body() {
        // The primitive: RFC 4231 test case 2 (key "Jefe", data "what do ya want for nothing?").
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(b"Jefe").expect("key");
        mac.update(b"what do ya want for nothing?");
        let hex: String = mac
            .finalize()
            .into_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(
            hex,
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        // Our format: the same primitive over "<timestamp>.<body>".
        let s = signature(b"Jefe", 1_700_000_000, b"{}");
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(b"Jefe").expect("key");
        mac.update(b"1700000000.{}");
        let want: String = mac
            .finalize()
            .into_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(s, format!("v1={want}"));
        assert_ne!(signature(b"other", 1_700_000_000, b"{}"), s);
        assert_ne!(
            signature(b"Jefe", 1_700_000_001, b"{}"),
            s,
            "the timestamp is signed"
        );
    }

    #[test]
    fn urls_and_secret_references_are_checked() {
        assert!(check_url("https://hooks.example/a?b=1").is_ok());
        for bad in [
            "http://hooks.example/a",
            "ftp://x",
            "https://",
            "https://u:p@hooks.example/",
            "hooks.example",
        ] {
            assert!(check_url(bad).is_err(), "{bad}");
        }
        assert!(check_secret_ref("APELORA_ALERT_SECRET_OPS_1").is_ok());
        for bad in [
            "APELORA_ALERT_SECRET_",
            "DATABASE_URL",
            "APELORA_ALERT_SECRET_ops",
            "APELORA_ALERT_SECRET_A-B",
            "apelora_alert_secret_x",
        ] {
            assert!(check_secret_ref(bad).is_err(), "{bad}");
        }
    }

    /// The requests a receiver got: head (lowercased) and body.
    type Seen = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

    /// A local HTTPS receiver with a self-signed certificate. Records each request and answers
    /// `status`.
    struct Receiver {
        url: String,
        cert: CertificateDer<'static>,
        seen: Seen,
    }

    async fn receiver(status: u16) -> Receiver {
        let key = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).expect("cert");
        let cert = CertificateDer::from(key.cert.der().to_vec());
        let pk = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.signing_key.serialize_der()));
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .expect("versions")
        .with_no_client_auth()
        .with_single_cert(vec![cert.clone()], pk)
        .expect("config");
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = seen.clone();
        tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let Ok(mut tls) = acceptor.accept(tcp).await else {
                    continue;
                };
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                // Read the head, then the body by its length.
                let (head, body) = loop {
                    let n = tls.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        break (String::from_utf8_lossy(&buf).into_owned(), Vec::new());
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..end]).to_lowercase();
                        let len: usize = head
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .and_then(|v| v.trim().parse().ok())
                            .unwrap_or(0);
                        while buf.len() < end + 4 + len {
                            let n = tls.read(&mut chunk).await.unwrap_or(0);
                            if n == 0 {
                                break;
                            }
                            buf.extend_from_slice(&chunk[..n]);
                        }
                        break (head, buf[end + 4..].to_vec());
                    }
                };
                record.lock().expect("lock").push((head, body));
                let reply = format!(
                    "HTTP/1.1 {status} X\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                );
                let _ = tls.write_all(reply.as_bytes()).await;
                let _ = tls.shutdown().await;
            }
        });
        Receiver {
            url: format!("https://localhost:{port}/hook"),
            cert,
            seen,
        }
    }

    fn trusting(cert: &CertificateDer<'static>) -> reqwest::Client {
        let c = reqwest::Certificate::from_der(cert).expect("cert");
        WebhookSink::client(reqwest::Client::builder().tls_certs_only([c])).expect("client")
    }

    async fn channel_to(pool: &Pool, url: &str) -> Uuid {
        db::create_channel(
            pool,
            &db::ChannelInput {
                name: "ops",
                url,
                secret_ref: "APELORA_ALERT_SECRET_OPS",
                enabled: true,
                min_severity: "warning",
            },
            "user:1",
        )
        .await
        .expect("channel")
        .id
    }

    async fn message_for(pool: &Pool, channel: Uuid) -> Message {
        let mut tx = apelora_persistence::begin(pool).await.expect("begin");
        db::notify(
            &mut tx,
            &[channel],
            &json!({ "version": 1, "event": "test", "alert": { "rule": "job-failures" } }),
        )
        .await
        .expect("notify");
        apelora_persistence::commit(tx).await.expect("commit");
        outbox::due(pool, 10)
            .await
            .expect("due")
            .pop()
            .expect("message")
    }

    fn secrets(value: Option<&'static str>) -> SecretSource {
        Arc::new(move |k: &str| {
            (k == "APELORA_ALERT_SECRET_OPS")
                .then_some(value)
                .flatten()
                .map(str::to_owned)
        })
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn a_signed_notification_reaches_an_https_receiver_once_through_the_relay(pool: Pool) {
        let r = receiver(204).await;
        let channel = channel_to(&pool, &r.url).await;
        let m = message_for(&pool, channel).await;
        let sink = Routed {
            alerts: Arc::new(WebhookSink::with(
                pool.clone(),
                trusting(&r.cert),
                secrets(Some("s3cret")),
            )),
            other: Arc::new(crate::outbox::LogSink),
        };
        let pass = crate::outbox::relay_once(&pool, &sink, None)
            .await
            .expect("pass");
        assert_eq!((pass.delivered, pass.failed), (1, 0));
        let seen = r.seen.lock().expect("lock").clone();
        assert_eq!(seen.len(), 1);
        let (head, body) = &seen[0];
        assert!(head.starts_with("post /hook "), "{head}");
        let header = |name: &str| {
            head.lines()
                .find_map(|l| l.strip_prefix(&format!("{name}:")))
                .map(|v| v.trim().to_owned())
                .unwrap_or_default()
        };
        assert_eq!(header("apelora-event-id"), m.id.to_string());
        let ts: i64 = header("apelora-timestamp").parse().expect("timestamp");
        assert_eq!(header("apelora-signature"), signature(b"s3cret", ts, body));
        let json: Value = serde_json::from_slice(body).expect("json");
        assert_eq!(json["eventId"], json!(m.id));
        assert_eq!(json["event"], "test");
        assert!(
            json.get("channel").is_none(),
            "routing stays inside Apelora"
        );
        assert!(!String::from_utf8_lossy(body).contains("s3cret"));
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn failures_are_retried_with_a_reason_that_never_names_the_url_or_the_secret(pool: Pool) {
        // An untrusted certificate: TLS is verified, so the delivery fails.
        let r = receiver(200).await;
        let channel = channel_to(&pool, &format!("{}?token=hidden-in-url", r.url)).await;
        message_for(&pool, channel).await;
        let system_trust = WebhookSink::with(
            pool.clone(),
            WebhookSink::client(reqwest::Client::builder()).expect("client"),
            secrets(Some("s3cret")),
        );
        let pass = crate::outbox::relay_once(&pool, &system_trust, None)
            .await
            .expect("pass");
        assert_eq!(pass.failed, 1);
        assert!(
            r.seen.lock().expect("lock").is_empty(),
            "nothing reached an untrusted receiver"
        );
        let d = db::channel_deliveries(&pool).await.expect("deliveries");
        let error = d[0].last_error.clone().unwrap_or_default();
        assert!(
            !error.contains("hidden-in-url") && !error.contains("localhost:"),
            "{error}"
        );
        assert_eq!((d[0].pending, d[0].failing), (1, 1));

        // A missing secret is a visible failure; nothing is sent.
        sqlx::query("UPDATE outbox SET next_attempt_at = now()")
            .execute(&pool)
            .await
            .expect("due");
        let no_secret = WebhookSink::with(pool.clone(), trusting(&r.cert), secrets(None));
        crate::outbox::relay_once(&pool, &no_secret, None)
            .await
            .expect("pass");
        let d = db::channel_deliveries(&pool).await.expect("deliveries");
        assert_eq!(
            d[0].last_error.as_deref(),
            Some("secret APELORA_ALERT_SECRET_OPS is not set on the worker")
        );
        assert!(r.seen.lock().expect("lock").is_empty());

        // A receiver that answers 500 is a failure with its status.
        let failing = receiver(500).await;
        sqlx::query("UPDATE alert_channels SET url = $1")
            .bind(&failing.url)
            .execute(&pool)
            .await
            .expect("url");
        sqlx::query("UPDATE outbox SET next_attempt_at = now()")
            .execute(&pool)
            .await
            .expect("due");
        let sink = WebhookSink::with(
            pool.clone(),
            trusting(&failing.cert),
            secrets(Some("s3cret")),
        );
        crate::outbox::relay_once(&pool, &sink, None)
            .await
            .expect("pass");
        let d = db::channel_deliveries(&pool).await.expect("deliveries");
        assert_eq!(
            d[0].last_error.as_deref(),
            Some("the receiver answered 500")
        );
        assert_eq!(failing.seen.lock().expect("lock").len(), 1);

        // A deleted channel: the message is dropped (delivered), not retried forever.
        db::delete_channel(&pool, channel).await.expect("delete");
        sqlx::query("UPDATE outbox SET next_attempt_at = now()")
            .execute(&pool)
            .await
            .expect("due");
        let pass = crate::outbox::relay_once(&pool, &sink, None)
            .await
            .expect("pass");
        assert_eq!((pass.delivered, pass.pending), (1, 0));
    }
}

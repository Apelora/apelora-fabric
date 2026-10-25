// Test fixtures use fixed secrets and keys on purpose; this file is left out of code scanning.

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
            let reply =
                format!("HTTP/1.1 {status} X\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
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

/// Clients that trust the receiver's certificate only.
fn trusting(cert: &CertificateDer<'static>) -> ClientSource {
    let c = reqwest::Certificate::from_der(cert).expect("cert");
    Arc::new(move || reqwest::Client::builder().tls_certs_only([c.clone()]))
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
        alerts: Arc::new(
            WebhookSink::with(pool.clone(), trusting(&r.cert), secrets(Some("s3cret")))
                .allowing_loopback(),
        ),
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
        Arc::new(reqwest::Client::builder),
        secrets(Some("s3cret")),
    )
    .allowing_loopback();
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
    // Coarse: that TLS failed, not what the certificate said.
    assert_eq!(
        error,
        "TLS with the receiver failed (its certificate was not accepted, or the handshake failed)"
    );
    assert_eq!((d[0].pending, d[0].failing), (1, 1));

    // A missing secret is a visible failure; nothing is sent.
    sqlx::query("UPDATE outbox SET next_attempt_at = now()")
        .execute(&pool)
        .await
        .expect("due");
    let no_secret =
        WebhookSink::with(pool.clone(), trusting(&r.cert), secrets(None)).allowing_loopback();
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
    )
    .allowing_loopback();
    crate::outbox::relay_once(&pool, &sink, None)
        .await
        .expect("pass");
    let d = db::channel_deliveries(&pool).await.expect("deliveries");
    assert_eq!(
        d[0].last_error.as_deref(),
        Some("the receiver answered 500")
    );
    assert_eq!(failing.seen.lock().expect("lock").len(), 1);

    // Nothing listening: only that the connection failed, never the system's own words.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = closed.local_addr().expect("addr").port();
    drop(closed);
    sqlx::query("UPDATE alert_channels SET url = $1")
        .bind(format!("https://localhost:{port}/hook"))
        .execute(&pool)
        .await
        .expect("url");
    sqlx::query("UPDATE outbox SET next_attempt_at = now()")
        .execute(&pool)
        .await
        .expect("due");
    crate::outbox::relay_once(&pool, &sink, None)
        .await
        .expect("pass");
    let d = db::channel_deliveries(&pool).await.expect("deliveries");
    assert_eq!(
        d[0].last_error.as_deref(),
        Some("could not connect to the receiver")
    );

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

/// A webhook is never sent to this computer, by name or by address (a receiver that only listens
/// on loopback stands in for the control plane's own services and the metadata service).
#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn a_channel_that_points_at_this_computer_is_refused(pool: Pool) {
    let r = receiver(204).await;
    let port = r.url.trim_start_matches("https://localhost:");
    let channel = channel_to(&pool, &r.url).await;
    message_for(&pool, channel).await;
    let sink = WebhookSink::with(pool.clone(), trusting(&r.cert), secrets(Some("s3cret")));
    for url in [
        r.url.clone(),
        format!("https://127.0.0.1:{port}"),
        format!("https://[::1]:{port}"),
        format!("https://[::ffff:127.0.0.1]:{port}"),
    ] {
        sqlx::query("UPDATE alert_channels SET url = $1")
            .bind(&url)
            .execute(&pool)
            .await
            .expect("url");
        sqlx::query("UPDATE outbox SET next_attempt_at = now()")
            .execute(&pool)
            .await
            .expect("due");
        let pass = crate::outbox::relay_once(&pool, &sink, None)
            .await
            .expect("pass");
        assert_eq!(pass.failed, 1, "{url}");
        let d = db::channel_deliveries(&pool).await.expect("deliveries");
        assert_eq!(d[0].last_error.as_deref(), Some(ADDRESS_REFUSED), "{url}");
    }
    assert!(r.seen.lock().expect("lock").is_empty(), "nothing was sent");
}

#[test]
fn only_this_computer_link_local_unspecified_and_multicast_are_refused() {
    let ip = |s: &str| s.parse::<IpAddr>().expect("ip");
    for bad in [
        "127.0.0.1",
        "127.8.9.10",
        "0.0.0.0",
        "0.1.2.3",
        "169.254.169.254",
        "224.0.0.1",
        "255.255.255.255",
        "::1",
        "::",
        "fe80::1",
        "febf::1",
        "ff02::1",
        "::ffff:127.0.0.1",
        "::ffff:169.254.169.254",
    ] {
        assert!(forbidden_address(ip(bad)), "{bad}");
    }
    for good in [
        "10.0.0.5",
        "172.16.0.1",
        "192.168.1.20",
        "100.64.0.1",
        "203.0.113.9",
        "fd00::5",
        "fec0::1",
        "2001:db8::1",
        "::ffff:10.0.0.5",
    ] {
        assert!(!forbidden_address(ip(good)), "{good}");
    }
}

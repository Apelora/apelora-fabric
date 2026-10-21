use super::*;
use std::collections::HashMap;

fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let m: HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    move |k| m.get(k).cloned()
}

fn addr(s: &str) -> SocketAddr {
    s.parse().expect("addr")
}

#[test]
fn loopback_serves_plain_http_as_before() {
    assert_eq!(
        listening(addr("127.0.0.1:8080"), env(&[])),
        Ok(Listening::Loopback)
    );
    assert_eq!(
        listening(addr("[::1]:8080"), env(&[])),
        Ok(Listening::Loopback)
    );
}

#[test]
fn a_reachable_address_needs_tls_or_a_declared_proxy() {
    for a in ["0.0.0.0:8080", "10.0.0.5:8080", "[::]:8080"] {
        let err = listening(addr(a), env(&[])).expect_err(a);
        assert!(
            err.contains("APELORA_TLS_CERT") && err.contains("APELORA_BEHIND_PROXY=1"),
            "{err}"
        );
    }
    assert_eq!(
        listening(addr("0.0.0.0:8080"), env(&[("APELORA_BEHIND_PROXY", "1")])),
        Ok(Listening::BehindProxy)
    );
    assert_eq!(
        listening(
            addr("0.0.0.0:8443"),
            env(&[
                ("APELORA_TLS_CERT", "/etc/a.pem"),
                ("APELORA_TLS_KEY", "/etc/a.key")
            ])
        ),
        Ok(Listening::Tls {
            cert: "/etc/a.pem".into(),
            key: "/etc/a.key".into()
        })
    );
    assert!(listening(addr("0.0.0.0:8080"), env(&[("APELORA_BEHIND_PROXY", "0")])).is_err());
}

#[test]
fn half_a_tls_setting_or_an_unclear_proxy_flag_is_an_error() {
    let e = listening(
        addr("127.0.0.1:8080"),
        env(&[("APELORA_TLS_CERT", "/a.pem")]),
    )
    .expect_err("cert only");
    assert!(e.contains("APELORA_TLS_KEY is not"), "{e}");
    let e = listening(
        addr("127.0.0.1:8080"),
        env(&[("APELORA_TLS_KEY", "/a.key")]),
    )
    .expect_err("key only");
    assert!(e.contains("APELORA_TLS_CERT is not"), "{e}");
    let e = listening(
        addr("0.0.0.0:8080"),
        env(&[("APELORA_BEHIND_PROXY", "maybe")]),
    )
    .expect_err("flag");
    assert!(e.starts_with("APELORA_BEHIND_PROXY must be"), "{e}");
}

#[test]
fn unreadable_or_mismatched_tls_files_are_named_in_the_error() {
    let e = tls_acceptor("/nonexistent/cert.pem", "/nonexistent/key.pem")
        .err()
        .expect("missing files");
    assert!(
        e.starts_with("APELORA_TLS_CERT (/nonexistent/cert.pem)"),
        "{e}"
    );
    let dir = scratch_dir("mismatch");
    let (a, b) = (self_signed(), self_signed());
    let (cert, key) = (dir.join("a.pem"), dir.join("b.key"));
    std::fs::write(&cert, a.cert.pem()).expect("write");
    std::fs::write(&key, b.signing_key.serialize_pem()).expect("write");
    let e = tls_acceptor(cert.to_str().expect("path"), key.to_str().expect("path"))
        .err()
        .expect("mismatched pair");
    assert!(e.contains("do not form a usable pair"), "{e}");
    let _ = std::fs::remove_dir_all(dir);
}

fn self_signed() -> rcgen::CertifiedKey<rcgen::KeyPair> {
    rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).expect("certificate")
}

/// A fresh directory under the system's temporary directory (never in the repository).
fn scratch_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "apelora-serve-test-{name}-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

/// The real API behind the TLS listener: a client that trusts the generated certificate gets
/// an answer with HSTS; a plain-HTTP client gets nothing.
#[tokio::test]
async fn the_api_answers_over_tls_with_hsts_and_not_in_clear_text() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let generated = self_signed();
    let dir = scratch_dir("tls");
    let (cert, key) = (dir.join("cert.pem"), dir.join("key.pem"));
    std::fs::write(&cert, generated.cert.pem()).expect("write");
    std::fs::write(&key, generated.signing_key.serialize_pem()).expect("write");
    let acceptor =
        tls_acceptor(cert.to_str().expect("path"), key.to_str().expect("path")).expect("tls");
    let _ = std::fs::remove_dir_all(&dir);

    let state = crate::api::AppState::new(
        apelora_incus_adapter::Provider::Mock(apelora_incus_adapter::MockIncus::default()),
        crate::api::Auth::DisabledForDevelopment,
    )
    .with_tls(true);
    let tcp = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let listener = TlsListener::new(tcp, acceptor).expect("listener");
    let local = axum::serve::Listener::local_addr(&listener).expect("addr");
    tokio::spawn(async move {
        axum::serve(
            axum::serve::ListenerExt::tap_io(listener, |_| {}),
            crate::api::router(state).into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .expect("serve");
    });

    // Plain HTTP to the TLS port: no HTTP answer.
    let mut plain = TcpStream::connect(local).await.expect("connect");
    plain
        .write_all(b"GET /api/v1/health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .expect("write");
    let mut answer = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), plain.read_to_end(&mut answer)).await;
    assert!(!String::from_utf8_lossy(&answer).contains("HTTP/1.1 200"));

    let mut roots = rustls::RootCertStore::empty();
    roots.add(generated.cert.der().clone()).expect("root");
    let client = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("versions")
    .with_root_certificates(roots)
    .with_no_client_auth();
    let connector = tokio_rustls::TlsConnector::from(Arc::new(client));
    let tcp = TcpStream::connect(local).await.expect("connect");
    let name = rustls_pki_types::ServerName::try_from("localhost").expect("name");
    let mut tls = connector.connect(name, tcp).await.expect("handshake");
    tls.write_all(b"GET /api/v1/health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .expect("write");
    let mut answer = Vec::new();
    tls.read_to_end(&mut answer).await.expect("read");
    let text = String::from_utf8_lossy(&answer).to_lowercase();
    assert!(text.starts_with("http/1.1 200"), "{text}");
    assert!(
        text.contains("strict-transport-security: max-age=31536000"),
        "{text}"
    );
    assert!(text.contains("\"connected\":false"), "{text}");
}

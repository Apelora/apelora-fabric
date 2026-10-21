// SPDX-License-Identifier: AGPL-3.0-only
//! How the API listens: plain HTTP on loopback, TLS of its own, or plain HTTP behind a declared
//! TLS-terminating proxy.
//!
//! - **Loopback** (`127.0.0.1`, `::1`): plain HTTP unless TLS is configured. Development and a
//!   proxy on the same host.
//! - **Any other address** needs either TLS (`APELORA_TLS_CERT` and `APELORA_TLS_KEY`, PEM files)
//!   or `APELORA_BEHIND_PROXY=1`, the operator's statement that only a proxy that terminates TLS
//!   can reach this address. Otherwise the process refuses to start: bearer tokens would cross
//!   the network in clear text.
//!
//! TLS uses rustls (already used to reach Incus), TLS 1.2 and 1.3, HTTP/1.1 (websockets for
//! terminal sessions). Handshakes run concurrently, at most [`MAX_HANDSHAKES`] at a time, each
//! bounded by [`HANDSHAKE_TIMEOUT`], so a slow or silent client cannot hold up others.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, mpsc};
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;

pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
pub const MAX_HANDSHAKES: usize = 256;

/// How to serve, decided from the address and the settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Listening {
    /// Plain HTTP on a loopback address.
    Loopback,
    /// TLS terminated here, with these PEM files.
    Tls { cert: String, key: String },
    /// Plain HTTP on a reachable address, behind a TLS-terminating proxy (declared).
    BehindProxy,
}

impl Listening {
    pub fn tls(&self) -> bool {
        matches!(self, Listening::Tls { .. })
    }
}

/// Decides how to listen on `addr`, or explains why it must not.
pub fn listening(
    addr: SocketAddr,
    get: impl Fn(&str) -> Option<String>,
) -> Result<Listening, String> {
    let set = |k: &str| {
        get(k)
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    let behind_proxy = match set("APELORA_BEHIND_PROXY").as_deref() {
        None | Some("0" | "false" | "no") => false,
        Some("1" | "true" | "yes") => true,
        Some(other) => {
            return Err(format!(
                "APELORA_BEHIND_PROXY must be 1 (a TLS-terminating proxy is in front) or 0; got {other:?}"
            ));
        }
    };
    let tls = match (set("APELORA_TLS_CERT"), set("APELORA_TLS_KEY")) {
        (Some(cert), Some(key)) => Some(Listening::Tls { cert, key }),
        (None, None) => None,
        (Some(_), None) => return Err(
            "APELORA_TLS_CERT is set but APELORA_TLS_KEY is not; set both (PEM files) or neither"
                .into(),
        ),
        (None, Some(_)) => return Err(
            "APELORA_TLS_KEY is set but APELORA_TLS_CERT is not; set both (PEM files) or neither"
                .into(),
        ),
    };
    if let Some(t) = tls {
        if behind_proxy {
            tracing::info!(
                "APELORA_BEHIND_PROXY is set, but TLS is configured here too: serving TLS"
            );
        }
        return Ok(t);
    }
    if addr.ip().is_loopback() {
        return Ok(Listening::Loopback);
    }
    if behind_proxy {
        return Ok(Listening::BehindProxy);
    }
    Err(format!(
        "refusing to serve plain HTTP on {addr}: bearer tokens would cross the network unencrypted. \
         Either set APELORA_TLS_CERT and APELORA_TLS_KEY (PEM certificate chain and private key), \
         or put a TLS-terminating proxy in front and set APELORA_BEHIND_PROXY=1 \
         (and APELORA_TRUSTED_PROXIES to its address), or listen on 127.0.0.1"
    ))
}

/// Reads the certificate chain and key and builds the TLS settings (HTTP/1.1 only).
pub fn tls_acceptor(cert: &str, key: &str) -> Result<TlsAcceptor, String> {
    use rustls_pki_types::pem::PemObject;
    use rustls_pki_types::{CertificateDer, PrivateKeyDer};
    let chain: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(cert)
        .and_then(|it| it.collect::<Result<_, _>>())
        .map_err(|e| {
            format!("APELORA_TLS_CERT ({cert}): cannot read a PEM certificate chain: {e}")
        })?;
    if chain.is_empty() {
        return Err(format!(
            "APELORA_TLS_CERT ({cert}): no certificate found (expected PEM \"BEGIN CERTIFICATE\")"
        ));
    }
    let key = PrivateKeyDer::from_pem_file(key)
        .map_err(|e| format!("APELORA_TLS_KEY ({key}): cannot read a PEM private key: {e}"))?;
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("TLS settings: {e}"))?
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .map_err(|e| format!("APELORA_TLS_CERT/APELORA_TLS_KEY do not form a usable pair: {e}"))?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(TlsAcceptor::from(Arc::new(config)))
}

/// A listener that hands out connections after their TLS handshake.
pub struct TlsListener {
    ready: mpsc::Receiver<(TlsStream<TcpStream>, SocketAddr)>,
    local: SocketAddr,
}

impl TlsListener {
    pub fn new(tcp: TcpListener, acceptor: TlsAcceptor) -> std::io::Result<Self> {
        let local = tcp.local_addr()?;
        let (tx, ready) = mpsc::channel(64);
        let slots = Arc::new(Semaphore::new(MAX_HANDSHAKES));
        tokio::spawn(async move {
            loop {
                let (stream, peer) = match tcp.accept().await {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::debug!(error = %e, "tls: accept failed");
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        continue;
                    }
                };
                if tx.is_closed() {
                    break;
                }
                let Ok(slot) = slots.clone().try_acquire_owned() else {
                    tracing::warn!(%peer, "tls: too many handshakes in progress; connection dropped");
                    continue;
                };
                let (acceptor, tx) = (acceptor.clone(), tx.clone());
                tokio::spawn(async move {
                    let _slot = slot;
                    match tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(stream)).await {
                        Ok(Ok(tls)) => {
                            let _ = tx.send((tls, peer)).await;
                        }
                        Ok(Err(e)) => tracing::debug!(%peer, error = %e, "tls: handshake failed"),
                        Err(_) => tracing::debug!(%peer, "tls: handshake timed out"),
                    }
                });
            }
        });
        Ok(Self { ready, local })
    }
}

impl axum::serve::Listener for TlsListener {
    type Io = TlsStream<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        match self.ready.recv().await {
            Some(c) => c,
            // The accept loop ended: nothing more will come.
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        Ok(self.local)
    }
}

#[cfg(test)]
mod tests {
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
            .write_all(
                b"GET /api/v1/health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            )
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
        tls.write_all(
            b"GET /api/v1/health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
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
}

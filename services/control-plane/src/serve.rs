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
mod tests;

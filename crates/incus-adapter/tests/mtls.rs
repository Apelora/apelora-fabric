// SPDX-License-Identifier: AGPL-3.0-only
//! Real mutual-TLS handshakes against a local rustls server that requires a client certificate.
//! Certificates are shaped like the ones Incus generates (self-signed ECDSA P-384, not a CA,
//! server-auth/client-auth usage, server name "unspecified"). Evidence: CONTRACT VERIFIED for
//! the TLS layer; not a real Incus server.

mod common;

use std::sync::Arc;

use apelora_incus_adapter::{AdapterError, IncusClient, Provider};
use common::{Pem, cfg, incus_like};
use rcgen::{CertificateParams, ExtendedKeyUsagePurpose, KeyPair, PKCS_ECDSA_P384_SHA384};
use rustls::RootCertStore;
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::server::WebPkiClientVerifier;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_rustls::TlsAcceptor;

const SERVER_JSON: &str = r#"{"type":"sync","status":"Success","status_code":200,"error_code":0,"error":"","metadata":{"api_extensions":["clustering"],"api_status":"stable","api_version":"1.0","auth":"trusted","environment":{"server_clustered":false,"server_version":"7.0.2"}}}"#;

/// Serves `/1.0` over TLS, accepting only clients presenting `trusted_client`.
async fn tls_server(server: &Pem, trusted_client: &Pem) -> u16 {
    let mut roots = RootCertStore::empty();
    roots.add(trusted_client.der.clone()).expect("root");
    let verifier = WebPkiClientVerifier::builder(Arc::new(roots))
        .build()
        .expect("verifier");
    let config = rustls::ServerConfig::builder()
        .with_client_cert_verifier(verifier)
        .with_single_cert(
            vec![server.der.clone()],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(server.key_der.clone())),
        )
        .expect("server config");
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else {
                return;
            };
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let mut buf = vec![0u8; 4096];
                let _ = tls.read(&mut buf).await;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{SERVER_JSON}",
                    SERVER_JSON.len()
                );
                let _ = tls.write_all(resp.as_bytes()).await;
                let _ = tls.shutdown().await;
            });
        }
    });
    port
}

#[tokio::test]
async fn pinned_incus_style_certificate_named_unspecified_connects_by_ip() {
    let server = incus_like(&["unspecified"], false);
    let client = incus_like(&[], true);
    let port = tls_server(&server, &client).await;
    let p = Provider::Incus(
        IncusClient::mtls(&cfg(port, "127.0.0.1", &client, &server, None)).expect("client"),
    );
    let info = p
        .server_info()
        .await
        .expect("handshake and request succeed");
    assert_eq!(info.server_version.as_deref(), Some("7.0.2"));
}

#[tokio::test]
async fn certificate_naming_the_host_needs_no_override() {
    let server = incus_like(&["localhost"], false);
    let client = incus_like(&[], true);
    let port = tls_server(&server, &client).await;
    let c = IncusClient::mtls(&cfg(port, "localhost", &client, &server, None)).expect("client");
    assert!(c.server_info().await.is_ok());
}

#[tokio::test]
async fn a_different_server_certificate_is_rejected() {
    let server = incus_like(&["unspecified"], false);
    let impostor_pin = incus_like(&["unspecified"], false);
    let client = incus_like(&[], true);
    let port = tls_server(&server, &client).await;
    let c =
        IncusClient::mtls(&cfg(port, "127.0.0.1", &client, &impostor_pin, None)).expect("client");
    let err = c
        .server_info()
        .await
        .expect_err("must not trust an unpinned certificate");
    assert!(
        matches!(&err, AdapterError::Connectivity(m) if m.to_lowercase().contains("certificate")),
        "{err:?}"
    );
}

#[tokio::test]
async fn a_client_the_server_does_not_trust_is_rejected() {
    let server = incus_like(&["unspecified"], false);
    let trusted = incus_like(&[], true);
    let stranger = incus_like(&[], true);
    let port = tls_server(&server, &trusted).await;
    let c = IncusClient::mtls(&cfg(port, "127.0.0.1", &stranger, &server, None)).expect("client");
    assert!(matches!(
        c.server_info().await,
        Err(AdapterError::Connectivity(_))
    ));
}

#[test]
fn an_explicit_name_the_certificate_does_not_cover_is_refused() {
    let server = incus_like(&["unspecified"], false);
    let client = incus_like(&[], true);
    let err = IncusClient::mtls(&cfg(
        8443,
        "127.0.0.1",
        &client,
        &server,
        Some("incus.example.test"),
    ))
    .expect_err("must refuse");
    assert!(
        matches!(&err, AdapterError::InvalidConfig(m) if m.contains("incus.example.test")),
        "{err:?}"
    );
}

/// An X.509 v1 certificate (public part only), as `openssl req -x509` without extensions makes.
const V1_CERT: &str = "-----BEGIN CERTIFICATE-----
MIIBGDCBwAIJANDltaFVvjsDMAoGCCqGSM49BAMCMBUxEzARBgNVBAMMCnYxLWZp
eHR1cmUwHhcNMjYxMDIwMDAxNDMxWhcNMzYxMDE3MDAxNDMxWjAVMRMwEQYDVQQD
DAp2MS1maXh0dXJlMFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEY7F5JBaf3JKv
feE1YKCMo6MIOiaREi77va27MqH7rpgK60npZpQVOmZoolvuewS6EEpPsSAOypgE
SfkyU0Gy2zAKBggqhkjOPQQDAgNHADBEAiAaLgTLvjB+4ZQq9ra5gXgs5TlM0Pvp
/gWbWvQ5pDTAwgIgTnOk8GtesUkJw/B0atIOG1z0TM+cd49HeJ3zIXG2iYQ=
-----END CERTIFICATE-----
";

#[test]
fn a_version_1_client_certificate_is_named_as_the_problem() {
    let server = incus_like(&["unspecified"], false);
    let client = incus_like(&[], true);
    let mut c = cfg(8443, "127.0.0.1", &client, &server, None);
    c.client_cert_pem = V1_CERT.as_bytes().to_vec();
    let err = IncusClient::mtls(&c).expect_err("v1 is refused");
    assert!(
        matches!(&err, AdapterError::InvalidConfig(m) if m.contains("client certificate: X.509 v3 is required")),
        "{err:?}"
    );
}

#[test]
fn the_client_knows_when_its_certificates_expire() {
    let server = incus_like(&["unspecified"], false);
    let mut p = CertificateParams::new(Vec::<String>::new()).expect("params");
    p.not_after = rcgen::date_time_ymd(2027, 1, 15);
    p.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    let key = KeyPair::generate_for(&PKCS_ECDSA_P384_SHA384).expect("key");
    let cert = p.self_signed(&key).expect("cert");
    let client = Pem {
        cert: cert.pem(),
        key: key.serialize_pem(),
        der: cert.der().clone(),
        key_der: key.serialize_der(),
    };
    let c = IncusClient::mtls(&cfg(8443, "127.0.0.1", &client, &server, None)).expect("client");
    let expiry = c.certificate_expiry().expect("expiry");
    // 2027-01-15T00:00:00Z
    assert_eq!(expiry.client, 1_799_971_200);
    assert_eq!(
        Some(expiry.server),
        apelora_incus_adapter::certificate_not_after(server.cert.as_bytes()).ok()
    );
    assert!(apelora_incus_adapter::certificate_not_after(b"not a certificate").is_err());
}

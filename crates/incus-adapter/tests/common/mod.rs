// SPDX-License-Identifier: AGPL-3.0-only
//! Certificates shaped like the ones Incus generates, shared by the TLS contract tests.

use apelora_incus_adapter::IncusConfig;
use rcgen::{
    CertificateParams, DistinguishedName, ExtendedKeyUsagePurpose, IsCa, KeyPair, KeyUsagePurpose,
    PKCS_ECDSA_P384_SHA384,
};
use rustls::pki_types::CertificateDer;

pub struct Pem {
    pub cert: String,
    pub key: String,
    pub der: CertificateDer<'static>,
    pub key_der: Vec<u8>,
}

pub fn incus_like(names: &[&str], client: bool) -> Pem {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P384_SHA384).expect("key");
    let mut p = CertificateParams::new(names.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>())
        .expect("params");
    p.distinguished_name = DistinguishedName::new();
    p.is_ca = IsCa::ExplicitNoCa;
    p.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyEncipherment,
    ];
    p.extended_key_usages = vec![if client {
        ExtendedKeyUsagePurpose::ClientAuth
    } else {
        ExtendedKeyUsagePurpose::ServerAuth
    }];
    let cert = p.self_signed(&key).expect("cert");
    Pem {
        cert: cert.pem(),
        key: key.serialize_pem(),
        der: cert.der().clone(),
        key_der: key.serialize_der(),
    }
}

pub fn cfg(
    port: u16,
    host: &str,
    client: &Pem,
    pinned_server: &Pem,
    tls_name: Option<&str>,
) -> IncusConfig {
    IncusConfig {
        url: format!("https://{host}:{port}"),
        client_cert_pem: client.cert.clone().into_bytes(),
        client_key_pem: client.key.clone().into_bytes(),
        server_cert_pem: pinned_server.cert.clone().into_bytes(),
        tls_server_name: tls_name.map(str::to_owned),
    }
}

// SPDX-License-Identifier: AGPL-3.0-only
//! Chooses the Incus provider from environment variables (shared by the control plane and
//! the deployment worker).

use crate::{IncusClient, IncusConfig, MockIncus, Provider};

pub const URL: &str = "APELORA_INCUS_URL";
pub const CLIENT_CERT: &str = "APELORA_INCUS_CLIENT_CERT";
pub const CLIENT_KEY: &str = "APELORA_INCUS_CLIENT_KEY";
pub const SERVER_CERT: &str = "APELORA_INCUS_SERVER_CERT";
/// Optional: name to verify the server certificate against (default: URL host, else "unspecified").
pub const TLS_SERVER_NAME: &str = "APELORA_INCUS_TLS_SERVER_NAME";

/// No `APELORA_INCUS_URL`: labelled mock data. URL set: all three certificate paths are
/// required and must be readable, otherwise startup fails with a clear message.
pub fn provider_from_env(get: impl Fn(&str) -> Option<String>) -> Result<Provider, String> {
    let Some(url) = get(URL).filter(|u| !u.trim().is_empty()) else {
        return Ok(Provider::Mock(MockIncus::default()));
    };
    let read = |key: &str| -> Result<Vec<u8>, String> {
        let path = get(key)
            .filter(|p| !p.trim().is_empty())
            .ok_or_else(|| format!("{URL} is set, so {key} is required"))?;
        std::fs::read(&path).map_err(|e| format!("{key}: cannot read {path}: {e}"))
    };
    let cfg = IncusConfig {
        url,
        client_cert_pem: read(CLIENT_CERT)?,
        client_key_pem: read(CLIENT_KEY)?,
        server_cert_pem: read(SERVER_CERT)?,
        tls_server_name: get(TLS_SERVER_NAME).filter(|n| !n.trim().is_empty()),
    };
    IncusClient::mtls(&cfg)
        .map(Provider::Incus)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use apelora_domain::ProviderMode;

    #[test]
    fn no_url_means_mock() {
        assert_eq!(
            provider_from_env(|_| None).expect("provider").mode(),
            ProviderMode::Mock
        );
    }

    #[test]
    fn url_without_certificates_refuses_to_start() {
        let err =
            provider_from_env(|k| (k == URL).then(|| "https://incus.invalid:8443".to_owned()))
                .expect_err("must fail");
        assert!(err.contains(CLIENT_CERT), "{err}");
    }

    #[test]
    fn unreadable_certificate_paths_are_reported() {
        let err = provider_from_env(|k| {
            Some(if k == URL {
                "https://incus.invalid:8443".to_owned()
            } else {
                "/nonexistent/apelora.pem".to_owned()
            })
        })
        .expect_err("must fail");
        assert!(
            err.contains("cannot read /nonexistent/apelora.pem"),
            "{err}"
        );
    }
}

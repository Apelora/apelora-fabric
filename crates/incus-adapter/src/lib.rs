// SPDX-License-Identifier: AGPL-3.0-only
//! Boundary between Apelora and the Incus REST API.
//!
//! Phase 01 ships only the provider trait and a fixture-backed mock. The real mTLS client
//! (version discovery, `api_extensions`, read-only inventory) arrives in Phase 03.

use apelora_domain::ProviderMode;

/// What an Incus server reports about itself (`GET /1.0`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInfo {
    pub api_version: String,
    /// `None` when the provider cannot know it (e.g. the mock).
    pub server_version: Option<String>,
    pub api_extensions: Vec<String>,
    pub clustered: bool,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AdapterError {
    #[error("not connected to an Incus server")]
    NotConfigured,
    #[error("the Incus server does not support the `{0}` API extension")]
    UnsupportedCapability(String),
}

pub trait IncusProvider: Send + Sync {
    fn mode(&self) -> ProviderMode;
    fn server_info(&self) -> Result<ServerInfo, AdapterError>;

    /// Fails with [`AdapterError::UnsupportedCapability`] instead of assuming a feature exists.
    fn require_extension(&self, name: &str) -> Result<(), AdapterError> {
        let info = self.server_info()?;
        if info.api_extensions.iter().any(|e| e == name) {
            Ok(())
        } else {
            Err(AdapterError::UnsupportedCapability(name.to_owned()))
        }
    }
}

/// Fixture data for development and tests. Nothing it returns comes from a real cluster.
#[derive(Debug, Clone)]
pub struct MockIncus {
    info: ServerInfo,
}

impl MockIncus {
    pub fn with_extensions(extensions: &[&str]) -> Self {
        Self {
            info: ServerInfo {
                api_version: "1.0".to_owned(),
                server_version: None,
                api_extensions: extensions.iter().map(|e| (*e).to_owned()).collect(),
                clustered: false,
            },
        }
    }
}

impl Default for MockIncus {
    fn default() -> Self {
        Self::with_extensions(&[])
    }
}

impl IncusProvider for MockIncus {
    fn mode(&self) -> ProviderMode {
        ProviderMode::Mock
    }

    fn server_info(&self) -> Result<ServerInfo, AdapterError> {
        Ok(self.info.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_says_it_is_a_mock_and_claims_no_version() {
        let mock = MockIncus::default();
        assert_eq!(mock.mode(), ProviderMode::Mock);
        assert_eq!(mock.server_info().expect("info").server_version, None);
    }

    #[test]
    fn missing_extension_is_reported_not_assumed() {
        let mock = MockIncus::with_extensions(&["instances"]);
        assert_eq!(mock.require_extension("instances"), Ok(()));
        assert_eq!(
            mock.require_extension("clustering"),
            Err(AdapterError::UnsupportedCapability("clustering".to_owned()))
        );
    }
}

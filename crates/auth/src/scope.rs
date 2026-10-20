// SPDX-License-Identifier: AGPL-3.0-only
//! Where a role applies, and what a request is about.
//!
//! ```text
//! platform ─┬─ organisation ── project ── application      (tenant tree)
//!           └─ hardware pool                                (infrastructure only)
//! ```

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// How wide a scope is. Lower is wider. `Pool` is a separate branch under the platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum Level {
    Platform,
    Organization,
    Project,
    Application,
    Pool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Scope {
    Platform,
    Organization {
        id: Uuid,
    },
    Project {
        id: Uuid,
    },
    Application {
        id: Uuid,
    },
    /// A named group of physical machines (a MAAS resource pool, if MAAS is adopted).
    Pool {
        name: String,
    },
}

impl Scope {
    pub fn level(&self) -> Level {
        match self {
            Scope::Platform => Level::Platform,
            Scope::Organization { .. } => Level::Organization,
            Scope::Project { .. } => Level::Project,
            Scope::Application { .. } => Level::Application,
            Scope::Pool { .. } => Level::Pool,
        }
    }
}

/// Kind of environment; policies such as "production needs MFA" refer to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum EnvKind {
    Production,
    Staging,
    Development,
    Other,
}

impl EnvKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EnvKind::Production => "production",
            EnvKind::Staging => "staging",
            EnvKind::Development => "development",
            EnvKind::Other => "other",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        [
            EnvKind::Production,
            EnvKind::Staging,
            EnvKind::Development,
            EnvKind::Other,
        ]
        .into_iter()
        .find(|k| k.as_str() == s)
    }
}

/// What a request is about, with every ancestor filled in. Build it from the database
/// (an application knows its project and organisation); never from client-supplied ids.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Resource {
    pub organization: Option<Uuid>,
    pub project: Option<Uuid>,
    pub application: Option<Uuid>,
    pub environment: Option<EnvKind>,
    pub pool: Option<String>,
}

impl Resource {
    pub fn platform() -> Self {
        Self::default()
    }
    pub fn organization(org: Uuid) -> Self {
        Self {
            organization: Some(org),
            ..Self::default()
        }
    }
    pub fn project(org: Uuid, project: Uuid) -> Self {
        Self {
            organization: Some(org),
            project: Some(project),
            ..Self::default()
        }
    }
    pub fn application(org: Uuid, project: Uuid, app: Uuid, env: Option<EnvKind>) -> Self {
        Self {
            organization: Some(org),
            project: Some(project),
            application: Some(app),
            environment: env,
            pool: None,
        }
    }
    pub fn pool(name: &str) -> Self {
        Self {
            pool: Some(name.to_owned()),
            ..Self::default()
        }
    }
}

/// Does a grant at `scope` reach `resource`? A broader scope covers everything beneath it.
pub fn covers(scope: &Scope, r: &Resource) -> bool {
    match scope {
        Scope::Platform => true,
        Scope::Organization { id } => r.organization == Some(*id),
        Scope::Project { id } => r.project == Some(*id),
        Scope::Application { id } => r.application == Some(*id),
        Scope::Pool { name } => r.pool.as_deref() == Some(name.as_str()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> (Uuid, Uuid, Uuid) {
        (Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(3))
    }

    #[test]
    fn broader_scopes_cover_narrower_resources_and_not_the_reverse() {
        let (o, p, a) = ids();
        let app = Resource::application(o, p, a, None);
        assert!(covers(&Scope::Platform, &app));
        assert!(covers(&Scope::Organization { id: o }, &app));
        assert!(covers(&Scope::Project { id: p }, &app));
        assert!(covers(&Scope::Application { id: a }, &app));
        let project = Resource::project(o, p);
        assert!(!covers(&Scope::Application { id: a }, &project));
        assert!(!covers(
            &Scope::Project { id: p },
            &Resource::organization(o)
        ));
    }

    #[test]
    fn siblings_and_other_organisations_are_not_covered() {
        let (o, p, a) = ids();
        let other = Uuid::from_u128(99);
        let app = Resource::application(o, p, a, None);
        assert!(!covers(&Scope::Organization { id: other }, &app));
        assert!(!covers(&Scope::Project { id: other }, &app));
        assert!(!covers(&Scope::Application { id: other }, &app));
        assert!(!covers(
            &Scope::Pool {
                name: "rack-1".into()
            },
            &app
        ));
        assert!(covers(
            &Scope::Pool {
                name: "rack-1".into()
            },
            &Resource::pool("rack-1")
        ));
        assert!(!covers(
            &Scope::Pool {
                name: "rack-1".into()
            },
            &Resource::pool("rack-2")
        ));
    }
}

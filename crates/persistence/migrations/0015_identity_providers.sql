-- Sign-in configured in the Fabric Console (docs/architecture/IDENTITY-AND-AUTHORIZATION.md):
-- OpenID Connect identity providers whose tokens the control plane accepts, and platform
-- settings such as whether API tokens are accepted. Providers set by environment variables are
-- not stored here; they are shown as read-only beside these.
CREATE TABLE identity_providers (
    id                  uuid PRIMARY KEY,
    -- What the sign-in button says after "Sign in with".
    name                text NOT NULL UNIQUE CHECK (length(name) BETWEEN 1 AND 60),
    issuer              text NOT NULL UNIQUE,
    audiences           text[] NOT NULL CHECK (cardinality(audiences) > 0),
    -- The public client the Console and the Studio sign in with; none: tokens are accepted
    -- (e.g. from automation) but no sign-in button is shown.
    client_id           text,
    scopes              text[] NOT NULL DEFAULT ARRAY['openid', 'profile', 'email'],
    groups_claim        text NOT NULL DEFAULT 'groups',
    username_claim      text NOT NULL DEFAULT 'preferred_username',
    mfa_amr             text[] NOT NULL DEFAULT ARRAY['mfa', 'otp', 'hwk'],
    mfa_acr             text[] NOT NULL DEFAULT '{}',
    authorized_parties  text[] NOT NULL DEFAULT '{}',
    enabled             boolean NOT NULL DEFAULT true,
    created_at          timestamptz NOT NULL DEFAULT now(),
    created_by          text NOT NULL,
    updated_at          timestamptz NOT NULL DEFAULT now(),
    updated_by          text NOT NULL
);

CREATE TABLE platform_settings (
    key         text PRIMARY KEY,
    value       jsonb NOT NULL,
    updated_at  timestamptz NOT NULL DEFAULT now(),
    updated_by  text NOT NULL
);

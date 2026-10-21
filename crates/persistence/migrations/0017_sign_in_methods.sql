-- Sign-in with a username and password (ADR 0007, docs/design/sign-in-methods.md).
-- Local accounts are rows in `users` with the issuer 'apelora:local' and the username as their
-- subject; their password (an Argon2id hash, never the password) and setup link live here.
-- Apelora sessions are issued after a password or directory sign-in; only the SHA-256 hash of
-- a session token or setup token is stored.

-- A local account's username is lowercase letters, digits, dots, underscores and hyphens.
ALTER TABLE users ADD CONSTRAINT users_local_username
    CHECK (issuer <> 'apelora:local' OR subject ~ '^[a-z0-9._-]{2,64}$');

CREATE TABLE local_credentials (
    user_id             uuid PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    -- PHC string ($argon2id$...); NULL until the person has chosen a password.
    password_hash       text,
    -- One-time setup link (create or reset): SHA-256 of the token, valid until setup_expires_at.
    setup_token_hash    bytea UNIQUE,
    setup_expires_at    timestamptz,
    failed_attempts     integer NOT NULL DEFAULT 0 CHECK (failed_attempts >= 0),
    locked_until        timestamptz,
    password_changed_at timestamptz,
    created_by          text NOT NULL,
    created_at          timestamptz NOT NULL DEFAULT now(),
    CHECK ((setup_token_hash IS NULL) = (setup_expires_at IS NULL))
);

CREATE TABLE user_sessions (
    id              uuid PRIMARY KEY,
    user_id         uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash      bytea NOT NULL UNIQUE,
    method          text NOT NULL CHECK (method IN ('password', 'directory')),
    -- Directory group names seen at sign-in; mapped onto Apelora groups by external name.
    external_groups text[] NOT NULL DEFAULT '{}',
    created_at      timestamptz NOT NULL DEFAULT now(),
    last_used_at    timestamptz NOT NULL DEFAULT now(),
    expires_at      timestamptz NOT NULL,
    revoked_at      timestamptz
);
CREATE INDEX user_sessions_live ON user_sessions (user_id) WHERE revoked_at IS NULL;
CREATE INDEX user_sessions_expiry ON user_sessions (expires_at);

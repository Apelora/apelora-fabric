-- API tokens. Only a SHA-256 hash of each token is stored; the token itself is shown once.
CREATE TABLE api_tokens (
    id          uuid PRIMARY KEY,
    name        text NOT NULL CHECK (length(name) BETWEEN 1 AND 80),
    token_hash  bytea NOT NULL UNIQUE,
    role        text NOT NULL CHECK (role IN ('fabric-admin', 'operator', 'studio-designer', 'auditor')),
    created_at  timestamptz NOT NULL DEFAULT now(),
    expires_at  timestamptz,
    revoked_at  timestamptz,
    last_used_at timestamptz
);

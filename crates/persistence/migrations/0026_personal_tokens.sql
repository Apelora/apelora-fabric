-- SPDX-License-Identifier: AGPL-3.0-only
-- Personal tokens (Console plan, Identity and access, wave 3): a person makes tokens for their
-- own scripts. A token acts as that person, with the permissions they hold at the moment of each
-- request (nothing is copied into the token), so losing a role or being disabled takes effect at
-- once. Only the SHA-256 hash of a token (`apt_` + 64 hex characters) is stored; the token is
-- shown once. Every token expires within a year.
--
-- Rotation makes a successor (`rotated_from` points back) and shortens the old token's expiry to
-- a short overlap window (`rotated_at` records when); a token is rotated at most once.
CREATE TABLE personal_tokens (
    id            uuid PRIMARY KEY,
    user_id       uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name          text NOT NULL CHECK (length(name) BETWEEN 1 AND 64),
    token_hash    bytea NOT NULL UNIQUE,
    created_at    timestamptz NOT NULL DEFAULT now(),
    expires_at    timestamptz NOT NULL CHECK (expires_at <= created_at + interval '366 days'),
    last_used_at  timestamptz,
    revoked_at    timestamptz,
    -- Who revoked it (`user:…`, or `disabled` when the person was disabled).
    revoked_by    text CHECK (length(revoked_by) BETWEEN 1 AND 200),
    rotated_from  uuid REFERENCES personal_tokens(id) ON DELETE SET NULL,
    rotated_at    timestamptz,
    CHECK ((revoked_at IS NULL) = (revoked_by IS NULL))
);
CREATE INDEX personal_tokens_user ON personal_tokens (user_id, created_at);
CREATE UNIQUE INDEX personal_tokens_one_successor ON personal_tokens (rotated_from) WHERE rotated_from IS NOT NULL;

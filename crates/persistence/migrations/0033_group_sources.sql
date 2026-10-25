-- A group mapped to a group outside Apelora names the source that may assert membership of it
-- (ADR 0016): an OpenID Connect issuer, or `directory:<id>` for a directory (whose groups are
-- named by their full DN). Only credentials from that source map onto it, and an external name
-- is unique per source, no longer across all sources.
ALTER TABLE groups
    ADD COLUMN external_source text
        CHECK (external_source IS NULL OR length(external_source) BETWEEN 1 AND 300),
    ADD CONSTRAINT groups_external_source_has_a_name
        CHECK (external_source IS NULL OR external_name IS NOT NULL);
DROP INDEX groups_external_name;
CREATE UNIQUE INDEX groups_external_source_name ON groups (external_source, external_name)
    NULLS NOT DISTINCT WHERE external_name IS NOT NULL;

-- The source of a session's external groups (`directory:<id>` for a directory sign-in). Sessions
-- issued before this migration have none, so their groups map onto nothing until the person
-- signs in again.
ALTER TABLE user_sessions
    ADD COLUMN group_source text CHECK (group_source IS NULL OR length(group_source) <= 300);

-- Mappings made before this migration have no source and match nothing. The control plane binds
-- them once, at its first start after this migration, when exactly one identity provider is
-- configured (it alone sees providers set by environment variables); otherwise an administrator
-- sets each one's source in the Console. This row records whether that has happened.
INSERT INTO platform_settings (key, value, updated_by)
VALUES ('identity.group_sources_backfill', '"pending"'::jsonb, 'migration 0033')
ON CONFLICT (key) DO NOTHING;

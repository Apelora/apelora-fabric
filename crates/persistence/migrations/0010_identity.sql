-- Identity, tenancy and authorisation (docs/architecture/IDENTITY-AND-AUTHORIZATION.md).
-- Apelora's own state only: people are authenticated by the identity provider, who may do
-- what is decided here, and credentials for Incus and MAAS are referenced by name, never stored.

CREATE TABLE organizations (
    id          uuid PRIMARY KEY,
    slug        text NOT NULL UNIQUE CHECK (slug ~ '^[a-z][a-z0-9-]{0,38}[a-z0-9]$'),
    name        text NOT NULL CHECK (length(name) BETWEEN 1 AND 80),
    created_at  timestamptz NOT NULL DEFAULT now(),
    disabled_at timestamptz
);

CREATE TABLE projects (
    id              uuid PRIMARY KEY,
    organization_id uuid NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    slug            text NOT NULL CHECK (slug ~ '^[a-z][a-z0-9-]{0,38}[a-z0-9]$'),
    name            text NOT NULL CHECK (length(name) BETWEEN 1 AND 80),
    created_at      timestamptz NOT NULL DEFAULT now(),
    UNIQUE (organization_id, slug)
);

-- An environment belongs to one project and maps to at most one restricted Incus project.
CREATE TABLE environments (
    id            uuid PRIMARY KEY,
    project_id    uuid NOT NULL REFERENCES projects(id) ON DELETE RESTRICT,
    slug          text NOT NULL CHECK (slug ~ '^[a-z][a-z0-9-]{0,38}[a-z0-9]$'),
    name          text NOT NULL CHECK (length(name) BETWEEN 1 AND 80),
    kind          text NOT NULL CHECK (kind IN ('production', 'staging', 'development', 'other')),
    incus_project text UNIQUE CHECK (incus_project ~ '^[a-z][a-z0-9-]{0,62}$'),
    created_at    timestamptz NOT NULL DEFAULT now(),
    UNIQUE (project_id, slug)
);

-- Existing applications land in a default organisation, project and environment (mapped to the
-- Incus project Apelora has always deployed into), so nothing that works today stops working.
INSERT INTO organizations (id, slug, name) VALUES ('00000000-0000-0000-0000-000000000001', 'default', 'Default');
INSERT INTO projects (id, organization_id, slug, name)
    VALUES ('00000000-0000-0000-0000-000000000002', '00000000-0000-0000-0000-000000000001', 'default', 'Default');
INSERT INTO environments (id, project_id, slug, name, kind, incus_project)
    VALUES ('00000000-0000-0000-0000-000000000003', '00000000-0000-0000-0000-000000000002', 'default', 'Default', 'other', 'apelora');

ALTER TABLE applications
    ADD COLUMN project_id uuid NOT NULL DEFAULT '00000000-0000-0000-0000-000000000002' REFERENCES projects(id) ON DELETE RESTRICT,
    ADD COLUMN environment_id uuid REFERENCES environments(id) ON DELETE RESTRICT;
UPDATE applications SET environment_id = '00000000-0000-0000-0000-000000000003' WHERE environment_id IS NULL;
CREATE INDEX applications_project ON applications (project_id);

CREATE TABLE users (
    id           uuid PRIMARY KEY,
    issuer       text NOT NULL,
    subject      text NOT NULL,
    username     text,
    email        text,
    display_name text,
    created_at   timestamptz NOT NULL DEFAULT now(),
    last_seen_at timestamptz,
    disabled_at  timestamptz,
    UNIQUE (issuer, subject)
);

-- organization_id NULL = a platform-level group.
CREATE TABLE groups (
    id              uuid PRIMARY KEY,
    organization_id uuid REFERENCES organizations(id) ON DELETE RESTRICT,
    name            text NOT NULL CHECK (length(name) BETWEEN 1 AND 80),
    -- The group name as the identity provider spells it in tokens; members are then implicit.
    external_name   text CHECK (external_name IS NULL OR length(external_name) BETWEEN 1 AND 255),
    description     text NOT NULL DEFAULT '',
    created_at      timestamptz NOT NULL DEFAULT now(),
    UNIQUE NULLS NOT DISTINCT (organization_id, name)
);
CREATE UNIQUE INDEX groups_external_name ON groups (external_name) WHERE external_name IS NOT NULL;

CREATE TABLE group_members (
    group_id uuid NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    user_id  uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    added_by text NOT NULL,
    added_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (group_id, user_id)
);

CREATE TABLE custom_roles (
    id          uuid PRIMARY KEY,
    name        text NOT NULL UNIQUE CHECK (length(name) BETWEEN 1 AND 60),
    description text NOT NULL DEFAULT '',
    permissions text[] NOT NULL CHECK (cardinality(permissions) BETWEEN 1 AND 100),
    created_by  text NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now()
);

-- Identities Apelora uses itself. A row names a credential held in the backend's secret store
-- (credential_ref); the secret is never in this table. Only `automation` identities can call
-- the API, with a token whose hash is stored here.
CREATE TABLE service_identities (
    id             uuid PRIMARY KEY,
    name           text NOT NULL UNIQUE CHECK (length(name) BETWEEN 1 AND 80),
    kind           text NOT NULL CHECK (kind IN ('workload', 'network', 'storage', 'provisioning', 'automation')),
    credential_ref text,
    restrictions   jsonb NOT NULL DEFAULT '{}'::jsonb,
    token_hash     bytea UNIQUE,
    created_by     text NOT NULL,
    created_at     timestamptz NOT NULL DEFAULT now(),
    expires_at     timestamptz,
    disabled_at    timestamptz,
    last_used_at   timestamptz,
    CHECK (token_hash IS NULL OR kind = 'automation')
);

CREATE TABLE role_assignments (
    id             uuid PRIMARY KEY,
    principal_type text NOT NULL CHECK (principal_type IN ('user', 'group', 'service')),
    principal_id   uuid NOT NULL,
    builtin_role   text,
    custom_role_id uuid REFERENCES custom_roles(id) ON DELETE RESTRICT,
    scope_type     text NOT NULL CHECK (scope_type IN ('platform', 'organization', 'project', 'application', 'pool')),
    organization_id uuid REFERENCES organizations(id) ON DELETE RESTRICT,
    project_id     uuid REFERENCES projects(id) ON DELETE RESTRICT,
    application_id uuid REFERENCES applications(id) ON DELETE RESTRICT,
    pool           text CHECK (pool IS NULL OR length(pool) BETWEEN 1 AND 80),
    conditions     jsonb NOT NULL DEFAULT '{}'::jsonb,
    kind           text NOT NULL DEFAULT 'standard' CHECK (kind IN ('standard', 'emergency')),
    expires_at     timestamptz,
    reason         text,
    created_by     text NOT NULL,
    created_at     timestamptz NOT NULL DEFAULT now(),
    revoked_at     timestamptz,
    revoked_by     text,
    CHECK ((builtin_role IS NULL) <> (custom_role_id IS NULL)),
    CHECK (
        (scope_type = 'platform'     AND organization_id IS NULL     AND project_id IS NULL     AND application_id IS NULL     AND pool IS NULL) OR
        (scope_type = 'organization' AND organization_id IS NOT NULL AND project_id IS NULL     AND application_id IS NULL     AND pool IS NULL) OR
        (scope_type = 'project'      AND organization_id IS NULL     AND project_id IS NOT NULL AND application_id IS NULL     AND pool IS NULL) OR
        (scope_type = 'application'  AND organization_id IS NULL     AND project_id IS NULL     AND application_id IS NOT NULL AND pool IS NULL) OR
        (scope_type = 'pool'         AND organization_id IS NULL     AND project_id IS NULL     AND application_id IS NULL     AND pool IS NOT NULL)
    ),
    CHECK (kind <> 'emergency' OR (expires_at IS NOT NULL AND reason IS NOT NULL AND scope_type = 'platform'))
);
CREATE INDEX role_assignments_principal ON role_assignments (principal_type, principal_id) WHERE revoked_at IS NULL;

-- Rules that apply to everyone: kind 'mfa' {"environments": ["production"], "infrastructure": true}
-- and kind 'approval' {"actions": ["application.delete"]}.
CREATE TABLE access_policies (
    id          uuid PRIMARY KEY,
    name        text NOT NULL UNIQUE CHECK (length(name) BETWEEN 1 AND 80),
    kind        text NOT NULL CHECK (kind IN ('mfa', 'approval')),
    parameters  jsonb NOT NULL DEFAULT '{}'::jsonb,
    enabled     boolean NOT NULL DEFAULT true,
    updated_by  text NOT NULL,
    updated_at  timestamptz NOT NULL DEFAULT now()
);

-- A request for a second person to approve a risky action on a named target.
CREATE TABLE approvals (
    id           uuid PRIMARY KEY,
    action       text NOT NULL,
    target       text NOT NULL CHECK (length(target) BETWEEN 1 AND 200),
    requester    text NOT NULL,
    approver     text,
    state        text NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'approved', 'rejected', 'cancelled', 'consumed')),
    reason       text NOT NULL DEFAULT '',
    created_at   timestamptz NOT NULL DEFAULT now(),
    expires_at   timestamptz NOT NULL,
    decided_at   timestamptz,
    consumed_at  timestamptz
);
CREATE INDEX approvals_open ON approvals (state, expires_at);

-- The audit trail records who (and what kind of actor), what, on what, and the outcome. Denied
-- attempts are recorded too. Rows are append-only: they can be neither changed nor deleted.
ALTER TABLE audit_events
    ADD COLUMN actor_type text NOT NULL DEFAULT 'system',
    ADD COLUMN outcome    text NOT NULL DEFAULT 'success' CHECK (outcome IN ('success', 'denied', 'failed')),
    ADD COLUMN scope      jsonb;
CREATE INDEX audit_events_actor ON audit_events (actor);

CREATE FUNCTION audit_events_append_only() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'audit_events is append-only';
END;
$$;
CREATE TRIGGER audit_events_no_change BEFORE UPDATE OR DELETE ON audit_events
    FOR EACH ROW EXECUTE FUNCTION audit_events_append_only();

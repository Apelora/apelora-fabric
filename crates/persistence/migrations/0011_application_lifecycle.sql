-- Application management: metadata, server-side drafts, published versions, and lifecycle
-- operations (start, stop, restart) tracked from request to confirmed result.

ALTER TABLE applications
    ADD COLUMN description text NOT NULL DEFAULT '' CHECK (length(description) <= 200),
    ADD COLUMN icon        text NOT NULL DEFAULT 'box' CHECK (icon ~ '^[a-z0-9-]{1,40}$'),
    ADD COLUMN created_by  text,
    ADD COLUMN updated_at  timestamptz NOT NULL DEFAULT now(),
    ADD COLUMN archived_at timestamptz;

-- The working document of an application. Saving needs the version it was based on, so two
-- editors can never silently overwrite each other. `validation_errors` is what the server's
-- structural check found in this version (empty = valid).
CREATE TABLE application_drafts (
    application_id    uuid PRIMARY KEY REFERENCES applications(id) ON DELETE CASCADE,
    document          jsonb NOT NULL,
    version           integer NOT NULL CHECK (version > 0),
    validation_errors jsonb NOT NULL DEFAULT '[]'::jsonb,
    updated_by        text NOT NULL,
    updated_at        timestamptz NOT NULL DEFAULT now()
);

-- Revisions stay immutable. A *published* revision is a version a person released from the draft
-- on purpose; earlier revisions were created by deployments and count as published.
ALTER TABLE application_revisions
    ADD COLUMN published     boolean NOT NULL DEFAULT false,
    ADD COLUMN label         text CHECK (label IS NULL OR length(label) <= 80),
    ADD COLUMN draft_version integer;
UPDATE application_revisions SET published = true;

CREATE TABLE application_operations (
    id                   uuid PRIMARY KEY,
    application_id       uuid NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
    project              text NOT NULL,
    kind                 text NOT NULL CHECK (kind IN ('start', 'stop', 'restart')),
    state                text NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'running', 'succeeded', 'failed')),
    requested_by         text NOT NULL,
    confirmed_disruption boolean NOT NULL DEFAULT false,
    created_at           timestamptz NOT NULL DEFAULT now(),
    started_at           timestamptz,
    finished_at          timestamptz,
    -- Ordered phases, each with the instances it acts on and their job and result.
    phases               jsonb NOT NULL DEFAULT '[]'::jsonb,
    error                text,
    idempotency_key      text UNIQUE
);
-- One operation at a time per application: a second request while one is active is refused.
CREATE UNIQUE INDEX application_one_active_operation ON application_operations (application_id) WHERE state IN ('pending', 'running');
CREATE INDEX application_operations_recent ON application_operations (application_id, created_at DESC);

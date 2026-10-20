-- Persistent volumes Apelora created (docs/architecture/STORAGE.md). The worker records each
-- one once it exists in Incus; whether it is attached is read from Incus, not stored, so the
-- record cannot drift. A volume outlives its instance, and an application with recorded
-- volumes cannot be deleted until they are: its data is never forgotten silently.
CREATE TABLE volumes (
    id              uuid PRIMARY KEY,
    application_id  uuid NOT NULL REFERENCES applications(id) ON DELETE RESTRICT,
    project         text NOT NULL,
    appliance       text NOT NULL,
    instance        text NOT NULL,
    -- The design's name (`data`) and the Incus volume (`<instance>-<name>`).
    name            text NOT NULL,
    incus_name      text NOT NULL,
    storage_pool    text NOT NULL,
    -- Member of a member-local pool; '' on a standalone server.
    member          text NOT NULL DEFAULT '',
    size_mib        integer NOT NULL CHECK (size_mib > 0),
    content         text NOT NULL CHECK (content IN ('filesystem', 'block')),
    class           text NOT NULL,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now(),
    UNIQUE (project, storage_pool, incus_name)
);
CREATE INDEX volumes_application ON volumes (application_id);

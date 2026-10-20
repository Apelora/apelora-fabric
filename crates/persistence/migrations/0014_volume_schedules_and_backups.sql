-- Scheduled snapshots and backups of volumes (docs/architecture/STORAGE.md, step 5). The worker
-- claims due volumes by `next_*_at` (FOR UPDATE SKIP LOCKED, so several workers never run the
-- same volume twice) and records the outcome here.
ALTER TABLE volumes
    ADD COLUMN snapshot_every_minutes integer CHECK (snapshot_every_minutes BETWEEN 15 AND 10080),
    ADD COLUMN snapshot_keep          integer CHECK (snapshot_keep BETWEEN 1 AND 50),
    ADD COLUMN next_snapshot_at       timestamptz,
    ADD COLUMN last_snapshot_at       timestamptz,
    ADD COLUMN snapshot_error         text,
    ADD COLUMN backup_every_minutes   integer CHECK (backup_every_minutes BETWEEN 15 AND 10080),
    ADD COLUMN backup_keep            integer CHECK (backup_keep BETWEEN 1 AND 50),
    ADD COLUMN next_backup_at         timestamptz,
    ADD COLUMN last_backup_at         timestamptz,
    ADD COLUMN backup_error           text;

-- Backup copies of volumes, in a separate pool and on another member where there is one. Not
-- tied to the volume's row: a backup outlives the volume it protects until someone deletes it.
CREATE TABLE volume_backups (
    id              uuid PRIMARY KEY,
    application_id  uuid NOT NULL REFERENCES applications(id) ON DELETE RESTRICT,
    project         text NOT NULL,
    -- The volume it is a copy of (Incus name) and the snapshot it was copied from.
    source          text NOT NULL,
    snapshot        text NOT NULL,
    name            text NOT NULL,
    storage_pool    text NOT NULL,
    member          text NOT NULL DEFAULT '',
    size_mib        integer NOT NULL CHECK (size_mib > 0),
    scheduled       boolean NOT NULL DEFAULT false,
    created_at      timestamptz NOT NULL DEFAULT now(),
    UNIQUE (project, storage_pool, name)
);
CREATE INDEX volume_backups_source ON volume_backups (project, source, created_at);

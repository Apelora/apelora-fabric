-- SPDX-License-Identifier: AGPL-3.0-only
-- ADR 0011: one correlation id end to end, one supervisor leader, and workers woken when a job
-- is queued.

-- Correlation (EVERY-SIZE.md §2.8). `request_id` is the API request that queued the job (also
-- in the `requestId` of that request's audit rows); jobs queued by the supervisor or the
-- scheduler have none. `incus_operations` lists every Incus operation the job started, oldest
-- first. It is history: `external_operation` stays the in-flight marker that the next attempt
-- waits on and that is cleared once the wait ends.
ALTER TABLE jobs ADD COLUMN request_id text CHECK (length(request_id) BETWEEN 1 AND 64);
ALTER TABLE jobs ADD COLUMN incus_operations text[] NOT NULL DEFAULT '{}';
CREATE INDEX jobs_request_id ON jobs (request_id) WHERE request_id IS NOT NULL;

-- Job wake-up (EVERY-SIZE.md §2.4). Every statement that inserts jobs sends one notification on
-- `apelora_jobs`, inside the inserting transaction, so it is delivered at commit (never for a
-- rolled-back insert) and no code path that queues work can forget it. Workers LISTEN and claim
-- at once; their periodic check stays the fallback when a notification is missed.
CREATE FUNCTION jobs_notify_queued() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM pg_notify('apelora_jobs', '');
    RETURN NULL;
END $$;
CREATE TRIGGER jobs_queued AFTER INSERT ON jobs
    FOR EACH STATEMENT EXECUTE FUNCTION jobs_notify_queued();

-- Leader leases: one row per role that only one process may play at a time (today the replica
-- supervisor). Taken by a conditional update when free or expired, renewed every few seconds by
-- the holder, lost on expiry. `generation` grows with every new holder; each write the leader
-- makes first checks, in its own transaction, that it still holds this generation (fencing),
-- the same rule as job leases.
CREATE TABLE leader_leases (
    name         text PRIMARY KEY CHECK (length(name) BETWEEN 1 AND 64),
    holder       text NOT NULL CHECK (length(holder) BETWEEN 1 AND 200),
    generation   bigint NOT NULL CHECK (generation > 0),
    expires_at   timestamptz NOT NULL,
    acquired_at  timestamptz NOT NULL DEFAULT now()
);

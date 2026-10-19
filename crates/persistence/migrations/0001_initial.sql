-- Apelora's own state. Incus keeps its own database; nothing here mirrors it.

CREATE TABLE applications (
    id          uuid PRIMARY KEY,
    name        text NOT NULL UNIQUE CHECK (length(name) BETWEEN 1 AND 80),
    created_at  timestamptz NOT NULL DEFAULT now()
);

-- Every saved Studio document is an immutable revision; deployments refer to exact revisions.
CREATE TABLE application_revisions (
    id              uuid PRIMARY KEY,
    application_id  uuid NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
    revision        integer NOT NULL CHECK (revision > 0),
    document        jsonb NOT NULL,
    created_by      text NOT NULL,
    created_at      timestamptz NOT NULL DEFAULT now(),
    UNIQUE (application_id, revision)
);

-- Durable work for the deployment worker. A job is owned by one worker at a time through an
-- expiring lease; `generation` increases on every claim so a worker whose lease expired cannot
-- commit a stale result (fencing).
CREATE TABLE jobs (
    id               uuid PRIMARY KEY,
    kind             text NOT NULL,
    application_id   uuid REFERENCES applications(id) ON DELETE SET NULL,
    payload          jsonb NOT NULL DEFAULT '{}'::jsonb,
    status           text NOT NULL DEFAULT 'pending'
                     CHECK (status IN ('pending', 'running', 'succeeded', 'failed', 'cancelled')),
    attempts         integer NOT NULL DEFAULT 0,
    max_attempts     integer NOT NULL DEFAULT 5 CHECK (max_attempts > 0),
    run_after        timestamptz NOT NULL DEFAULT now(),
    lease_owner      text,
    lease_until      timestamptz,
    generation       bigint NOT NULL DEFAULT 0,
    idempotency_key  text UNIQUE,
    last_error       text,
    created_at       timestamptz NOT NULL DEFAULT now(),
    updated_at       timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX jobs_claimable ON jobs (run_after) WHERE status IN ('pending', 'running');

CREATE TABLE audit_events (
    id      bigserial PRIMARY KEY,
    at      timestamptz NOT NULL DEFAULT now(),
    actor   text NOT NULL,
    action  text NOT NULL,
    target  text NOT NULL,
    detail  jsonb NOT NULL DEFAULT '{}'::jsonb
);
CREATE INDEX audit_events_at ON audit_events (at);

-- SPDX-License-Identifier: AGPL-3.0-only
-- Job control (Console wave 1): who queued a job, retrying a failed job as a new job linked to
-- it, cancelling, and a record of every attempt.

-- Who asked for the job (`user:…`, `service:…`, `token:…`, as on audit rows). Jobs the
-- supervisor or the scheduler queued have none.
ALTER TABLE jobs ADD COLUMN queued_by text CHECK (length(queued_by) BETWEEN 1 AND 200);

-- A retry is a new job, never the failed one re-queued: the failed job keeps its attempts,
-- error and Incus operations, the new one starts with a fresh fencing generation, and the link
-- says where it came from. A failed job is retried at most once; retry the newer job instead.
ALTER TABLE jobs ADD COLUMN retry_of uuid REFERENCES jobs(id) ON DELETE SET NULL;
CREATE UNIQUE INDEX jobs_one_retry ON jobs (retry_of) WHERE retry_of IS NOT NULL;

-- Cancellation. A pending job with nothing in flight is cancelled at once; otherwise the request
-- is recorded here and the worker stops at its next safe point (docs/runbooks/troubleshooting.md).
ALTER TABLE jobs ADD COLUMN cancel_requested_at timestamptz;
ALTER TABLE jobs ADD COLUMN cancel_requested_by text CHECK (length(cancel_requested_by) BETWEEN 1 AND 200);

-- Instance jobs are looked up by their target (instance detail, retry's "superseded" check).
CREATE INDEX jobs_instance_target ON jobs ((payload->>'project'), (payload->>'name'), created_at DESC)
    WHERE payload ? 'name';

-- One row per attempt: written when a worker claims the job and closed, fenced by the claim's
-- generation, when the attempt ends. A coordinating job that only waits is claimed many times
-- under one attempt number; those claims share the row. `lost`: the worker stopped renewing its
-- lease and another worker took the job over.
CREATE TABLE job_attempts (
    job_id           uuid NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    attempt          integer NOT NULL CHECK (attempt > 0),
    generation       bigint NOT NULL,
    worker           text NOT NULL CHECK (length(worker) BETWEEN 1 AND 200),
    started_at       timestamptz NOT NULL DEFAULT now(),
    ended_at         timestamptz,
    outcome          text CHECK (outcome IN ('succeeded', 'failed', 'rejected', 'waiting', 'cancelled', 'lost')),
    error            text,
    incus_operations text[] NOT NULL DEFAULT '{}',
    PRIMARY KEY (job_id, attempt)
);

-- An application operation can be cancelled between phases.
ALTER TABLE application_operations DROP CONSTRAINT application_operations_state_check;
ALTER TABLE application_operations ADD CONSTRAINT application_operations_state_check
    CHECK (state IN ('pending', 'running', 'succeeded', 'failed', 'cancelled'));

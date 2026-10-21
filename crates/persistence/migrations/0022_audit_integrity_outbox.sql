-- SPDX-License-Identifier: AGPL-3.0-only
-- ADR 0011 (EVERY-SIZE.md §2.5, "Auditing", §5 items 1 and 9): audit integrity, the read side's
-- filters, audited provider sign-ins, and the transactional outbox.

-- Append-only, completely. Row triggers do not fire for TRUNCATE, so a statement-level trigger
-- refuses it too (with the same message as UPDATE and DELETE).
CREATE TRIGGER audit_events_no_truncate BEFORE TRUNCATE ON audit_events
    FOR EACH STATEMENT EXECUTE FUNCTION audit_events_append_only();

-- The name the actor had when they acted (a person's username, a token's or service identity's
-- name), so the trail can be searched by the name people know. Older rows have none: they are
-- found by the actor's stable id (`user:<uuid>`), and rows cannot be updated to fill it in.
ALTER TABLE audit_events ADD COLUMN actor_name text CHECK (length(actor_name) <= 200);

-- The read side filters by actor, target, action prefix, outcome and time, newest first, and
-- pages by id. `audit_events_at` (time) already exists.
DROP INDEX audit_events_actor;
CREATE INDEX audit_events_actor_id ON audit_events (actor, id);
CREATE INDEX audit_events_actor_name_id ON audit_events (lower(actor_name), id) WHERE actor_name IS NOT NULL;
CREATE INDEX audit_events_target_id ON audit_events (target, id);
CREATE INDEX audit_events_action_id ON audit_events (action text_pattern_ops, id);
-- Refusals and failures are few and the ones people look for.
CREATE INDEX audit_events_not_success_id ON audit_events (id) WHERE outcome <> 'success';

-- A sign-in with an identity provider's token is audited on the first request of each period
-- (an hour) rather than on every request: this records when that was last done for the person.
ALTER TABLE users ADD COLUMN sign_in_audited_at timestamptz;

-- The transactional outbox (EVERY-SIZE.md §2.5). A change writes, in its own transaction, the
-- messages it must cause; a relay in the worker delivers them at least once, oldest first, with
-- backoff, and receivers ignore duplicates by `id`. Delivered rows are kept for a while for
-- troubleshooting, then removed by the relay; undelivered rows are never removed.
CREATE TABLE outbox (
    id              bigserial PRIMARY KEY,
    topic           text NOT NULL CHECK (topic ~ '^[a-z][a-z0-9_.-]{0,63}$'),
    payload         jsonb NOT NULL,
    created_at      timestamptz NOT NULL DEFAULT now(),
    delivered_at    timestamptz,
    attempts        integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    last_error      text CHECK (length(last_error) <= 1000),
    -- When the relay may try again after a failed attempt (backoff).
    next_attempt_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX outbox_due ON outbox (next_attempt_at, id) WHERE delivered_at IS NULL;
CREATE INDEX outbox_delivered ON outbox (delivered_at) WHERE delivered_at IS NOT NULL;

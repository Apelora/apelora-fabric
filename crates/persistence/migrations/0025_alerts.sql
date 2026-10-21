-- SPDX-License-Identifier: AGPL-3.0-only
-- Alerts (docs/design/alerts.md): built-in rules evaluated by the worker under the
-- `alert-evaluator` lease, their state and history, acknowledgements, silences and webhook
-- channels. Notifications go through the outbox (topic `alert`, ADR 0011).
--
-- Alerts are machine observations that change over time (opened, refreshed, acknowledged,
-- resolved, pruned), so they live here and not in the append-only audit trail. What people do
-- with them (acknowledge, silence, change rules and channels) is audited in the same transaction.

-- What an operator changed about a built-in rule. The rules themselves are code
-- (`apelora_domain::alerts::RULES`); a missing row means the defaults.
CREATE TABLE alert_rule_settings (
    rule         text PRIMARY KEY CHECK (rule ~ '^[a-z][a-z0-9-]{0,63}$'),
    enabled      boolean NOT NULL,
    threshold    double precision NOT NULL,
    for_secs     integer NOT NULL CHECK (for_secs BETWEEN 0 AND 86400),
    severity     text NOT NULL CHECK (severity IN ('critical', 'warning', 'info')),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    updated_by   text NOT NULL CHECK (length(updated_by) BETWEEN 1 AND 200)
);

-- One row per occurrence of a condition on a subject. `pending`: the condition holds but not
-- yet for the rule's duration (deleted if it clears first). `firing`: notified. `resolved`:
-- history. At most one open (pending or firing) row per rule and subject.
CREATE TABLE alerts (
    id               bigserial PRIMARY KEY,
    rule             text NOT NULL CHECK (rule ~ '^[a-z][a-z0-9-]{0,63}$'),
    subject          text NOT NULL CHECK (length(subject) BETWEEN 1 AND 300),
    severity         text NOT NULL CHECK (severity IN ('critical', 'warning', 'info')),
    state            text NOT NULL CHECK (state IN ('pending', 'firing', 'resolved')),
    summary          text NOT NULL CHECK (length(summary) <= 500),
    value            double precision NOT NULL,
    threshold        double precision NOT NULL,
    -- When the condition was first seen, when it fired, when it was last seen holding, when
    -- it was first seen clear while firing (reset if it holds again), when it resolved.
    started_at       timestamptz NOT NULL DEFAULT now(),
    fired_at         timestamptz,
    last_seen_at     timestamptz NOT NULL DEFAULT now(),
    cleared_at       timestamptz,
    resolved_at      timestamptz,
    acknowledged_at  timestamptz,
    acknowledged_by  text CHECK (length(acknowledged_by) BETWEEN 1 AND 200),
    acknowledged_by_name text CHECK (length(acknowledged_by_name) <= 200),
    ack_note         text CHECK (length(ack_note) <= 500),
    CHECK ((state = 'pending') = (fired_at IS NULL)),
    CHECK ((state = 'resolved') = (resolved_at IS NOT NULL))
);
CREATE UNIQUE INDEX alerts_one_open ON alerts (rule, subject) WHERE state <> 'resolved';
CREATE INDEX alerts_history ON alerts (id) WHERE state = 'resolved';
CREATE INDEX alerts_resolved_at ON alerts (resolved_at) WHERE state = 'resolved';

-- Silences: no notification is sent for state changes of the rule (one subject, or all of the
-- rule's subjects when `subject` is NULL) between `starts_at` and `ends_at`. Ending one early
-- sets `ended_at`. Evaluation and history are not affected.
CREATE TABLE alert_silences (
    id              uuid PRIMARY KEY,
    rule            text NOT NULL CHECK (rule ~ '^[a-z][a-z0-9-]{0,63}$'),
    subject         text CHECK (length(subject) BETWEEN 1 AND 300),
    starts_at       timestamptz NOT NULL DEFAULT now(),
    ends_at         timestamptz NOT NULL,
    note            text NOT NULL DEFAULT '' CHECK (length(note) <= 500),
    created_by      text NOT NULL CHECK (length(created_by) BETWEEN 1 AND 200),
    created_by_name text CHECK (length(created_by_name) <= 200),
    created_at      timestamptz NOT NULL DEFAULT now(),
    ended_at        timestamptz,
    ended_by        text CHECK (length(ended_by) BETWEEN 1 AND 200),
    CHECK (ends_at > starts_at)
);
CREATE INDEX alert_silences_active ON alert_silences (rule, ends_at) WHERE ended_at IS NULL;

-- Notification channels. Only webhooks need a row (the Console is always a channel). The
-- signing secret is a reference: the name of an environment variable the worker reads at
-- delivery time, never the value (docs/design/alerts.md §2.4).
CREATE TABLE alert_channels (
    id            uuid PRIMARY KEY,
    name          text NOT NULL UNIQUE CHECK (length(name) BETWEEN 1 AND 80),
    kind          text NOT NULL CHECK (kind IN ('webhook')),
    url           text NOT NULL CHECK (url LIKE 'https://%' AND length(url) <= 2000),
    secret_ref    text NOT NULL CHECK (secret_ref ~ '^APELORA_ALERT_SECRET_[A-Z0-9_]{1,64}$'),
    enabled       boolean NOT NULL DEFAULT true,
    min_severity  text NOT NULL DEFAULT 'warning' CHECK (min_severity IN ('critical', 'warning', 'info')),
    created_at    timestamptz NOT NULL DEFAULT now(),
    created_by    text NOT NULL CHECK (length(created_by) BETWEEN 1 AND 200),
    updated_at    timestamptz NOT NULL DEFAULT now()
);

-- The evaluator's last pass (one row): so the Console can say when alerts were last evaluated
-- and warn when no worker evaluates them, and what each rule's source said.
CREATE TABLE alert_evaluator (
    id             smallint PRIMARY KEY CHECK (id = 1),
    last_pass_at   timestamptz NOT NULL,
    duration_secs  double precision NOT NULL,
    worker         text NOT NULL CHECK (length(worker) BETWEEN 1 AND 200),
    every_secs     integer NOT NULL CHECK (every_secs > 0),
    -- { "<rule>": "ok" | "not-configured" | "unavailable" | "disabled", ... }
    sources        jsonb NOT NULL DEFAULT '{}'::jsonb
);

-- A channel's delivery state is read from its outbox messages (no writes from the relay).
CREATE INDEX outbox_alert_channel ON outbox ((payload->>'channel'), id) WHERE topic = 'alert';

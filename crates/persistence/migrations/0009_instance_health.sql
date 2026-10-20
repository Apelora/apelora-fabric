-- Health of deployed instances as the worker's supervisor decided it, with hysteresis (Phase
-- 08). One row per instance it checks; the status API reports it. The supervisor is the only
-- writer, so the decision is made in one place.
CREATE TABLE instance_health (
    project         text NOT NULL,
    instance        text NOT NULL,
    application_id  uuid NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
    appliance       text NOT NULL,
    health          text NOT NULL CHECK (health IN ('unknown', 'healthy', 'unhealthy')),
    -- Since when the instance has had this health.
    since           timestamptz NOT NULL,
    checked_at      timestamptz NOT NULL DEFAULT now(),
    -- What the decision rests on, e.g. "port check failing".
    detail          text NOT NULL DEFAULT '',
    restarted_at    timestamptz,
    PRIMARY KEY (project, instance)
);

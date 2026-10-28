-- Replicas taken out of their service addresses before Incus moves them (evacuating or
-- restoring a member), so no request goes to an instance that is about to stop. A drain holds
-- only while the instance is still on `from_member`; once it has moved it serves again. Rows
-- older than the drain's lifetime are ignored and removed.
CREATE TABLE instance_drains (
    project      text NOT NULL,
    instance     text NOT NULL,
    from_member  text NOT NULL,
    requested_by text NOT NULL,
    requested_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (project, instance)
);

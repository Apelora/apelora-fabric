-- What each application should run in each project, as last deployed. The replica
-- supervisor compares this with Incus and restores lost stateless replicas.
CREATE TABLE application_deployments (
    application_id  uuid NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
    project         text NOT NULL,
    revision        integer NOT NULL,
    desired         jsonb NOT NULL,
    connections     jsonb,
    uplink          text,
    updated_at      timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (application_id, project)
);

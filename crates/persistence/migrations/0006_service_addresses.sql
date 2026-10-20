-- Service addresses: the listen address of each stateless appliance's load balancer, taken
-- from the routed range the deployment used. One address per (application, project,
-- appliance); an address belongs to one appliance at a time across all applications.
ALTER TABLE application_deployments ADD COLUMN service_range text;

CREATE TABLE service_addresses (
    address         text PRIMARY KEY,
    application_id  uuid NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
    project         text NOT NULL,
    appliance       text NOT NULL,
    created_at      timestamptz NOT NULL DEFAULT now(),
    UNIQUE (application_id, project, appliance)
);

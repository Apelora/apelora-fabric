-- Ports an application publishes outside itself, as last deployed (JSON list of
-- {appliance, transport, port}); the replica supervisor needs them to rebuild network syncs.
ALTER TABLE application_deployments ADD COLUMN published jsonb;

-- "Appliance" is now called "module" everywhere (ADR 0006). The columns follow; earlier
-- migrations are left as they were applied. JSON written before the rename (desired state,
-- published ports, job payloads, operation steps) still says `appliance`, and the code reads both.
ALTER TABLE service_addresses RENAME COLUMN appliance TO module;
ALTER TABLE instance_health RENAME COLUMN appliance TO module;
ALTER TABLE volumes RENAME COLUMN appliance TO module;

-- Two more application operations (ADR 0015): `shutdown` (graceful, what `stop` was until now)
-- and `migrate` (move every instance to another server). `stop` is now an immediate power-off;
-- rows recorded earlier keep saying `stop` and were graceful.
ALTER TABLE application_operations DROP CONSTRAINT application_operations_kind_check;
ALTER TABLE application_operations ADD CONSTRAINT application_operations_kind_check
    CHECK (kind IN ('start', 'stop', 'shutdown', 'restart', 'migrate'));

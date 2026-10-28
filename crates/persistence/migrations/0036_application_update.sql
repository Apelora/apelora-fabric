-- A rolling update (docs/design/rolling-updates.md): deploying a version whose image or
-- settings changed replaces a stateless module's replicas one at a time, as an application
-- operation, so it is one at a time per application and resumes after a worker restart.
ALTER TABLE application_operations DROP CONSTRAINT application_operations_kind_check;
ALTER TABLE application_operations ADD CONSTRAINT application_operations_kind_check
    CHECK (kind IN ('start', 'stop', 'shutdown', 'restart', 'migrate', 'update'));

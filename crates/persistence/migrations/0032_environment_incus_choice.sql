-- An environment uses an Incus project that already exists, and that Apelora did not create for
-- it, only when a platform administrator (cluster.manage) named that project when creating the
-- environment (docs/design/tenancy-isolation.md). A project whose name merely matches the name
-- derived from the environment's identifiers is no longer adopted, and a project that carries
-- another environment's marker never is.
ALTER TABLE environments ADD COLUMN incus_project_chosen boolean NOT NULL DEFAULT false;

-- Existing rows: environments a platform administrator mapped explicitly (recorded in the audit
-- trail since the first identity release), the first installation's default environment (the
-- platform's own project), and environments already adopted, which keep working as they are
-- (review them on the Tenancy page).
UPDATE environments e SET incus_project_chosen = true
 WHERE e.incus_state = 'adopted'
    OR e.id = '00000000-0000-0000-0000-000000000003'
    OR EXISTS (
        SELECT 1 FROM audit_events a
         WHERE a.action = 'environment.create' AND a.outcome = 'success'
           AND a.detail->>'id' = e.id::text AND a.detail->>'chosenByPlatformAdmin' = 'true');

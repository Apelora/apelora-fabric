-- Tenancy isolation and quotas (docs/design/tenancy-isolation.md, ADR 0012).

-- What Apelora knows about each environment's Incus project: pending (not prepared by Apelora:
-- created before this migration, no admin identity, or the last attempt failed, `incus_detail`
-- says why), created (by Apelora, with the baseline restrictions) or adopted (it existed and is
-- restricted). `incus_limits` is the set of limit keys Apelora last wrote, so it only ever
-- removes limits it set itself.
ALTER TABLE environments
    ADD COLUMN incus_state text NOT NULL DEFAULT 'pending'
        CHECK (incus_state IN ('pending', 'created', 'adopted')),
    ADD COLUMN incus_detail text NOT NULL DEFAULT '' CHECK (length(incus_detail) <= 2000),
    ADD COLUMN incus_checked_at timestamptz,
    ADD COLUMN incus_limits jsonb NOT NULL DEFAULT '{}'::jsonb
        CHECK (jsonb_typeof(incus_limits) = 'object');

-- One row per scope that has a quota: exactly one of the three scope columns is set. A null
-- limit means "no limit in that dimension". Memory and disk in GiB, CPU in CPUs.
CREATE TABLE tenancy_quotas (
    id              uuid PRIMARY KEY,
    organization_id uuid REFERENCES organizations(id) ON DELETE CASCADE,
    project_id      uuid REFERENCES projects(id) ON DELETE CASCADE,
    environment_id  uuid REFERENCES environments(id) ON DELETE CASCADE,
    instances       bigint CHECK (instances BETWEEN 0 AND 100000),
    cpu             bigint CHECK (cpu BETWEEN 0 AND 100000),
    memory_gib      bigint CHECK (memory_gib BETWEEN 0 AND 10000000),
    disk_gib        bigint CHECK (disk_gib BETWEEN 0 AND 100000000),
    updated_by      text NOT NULL,
    updated_at      timestamptz NOT NULL DEFAULT now(),
    CHECK (num_nonnulls(organization_id, project_id, environment_id) = 1),
    CHECK (num_nonnulls(instances, cpu, memory_gib, disk_gib) >= 1)
);
CREATE UNIQUE INDEX tenancy_quotas_organization ON tenancy_quotas (organization_id) WHERE organization_id IS NOT NULL;
CREATE UNIQUE INDEX tenancy_quotas_project ON tenancy_quotas (project_id) WHERE project_id IS NOT NULL;
CREATE UNIQUE INDEX tenancy_quotas_environment ON tenancy_quotas (environment_id) WHERE environment_id IS NOT NULL;

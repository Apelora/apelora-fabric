---
name: storage-recovery
description: Use for Incus storage pools, persistent volumes, backups, disaster recovery and stateful HA.
---

# Persistent Storage and Data Safety

## When to use

Use for Incus storage pools, persistent volumes, backups, disaster recovery and stateful HA.

## Required workflow and guardrails

- Classify each volume: ephemeral root, persistent exclusive, shared read-only, backup, secret. Record attachment, ownership and relocation policy.
- Distinguish a local pool (btrfs `local` in the lab; dir, LVM or ZFS elsewhere) from true shared storage. A local pool across multiple members is not shared data.
- Shared storage options: clustered LVM (`lvmcluster`) on a SAN, which the lab runs as pool `shared` on an iSCSI LUN (`scripts/test-cluster/shared-storage.sh`), or distributed storage that Incus supports (evaluate through the official docs). Each needs its own components and monitoring. On `lvmcluster` Incus activates container file systems exclusively (`-aey`) but VM disks and block volumes **shared** (`-asy`): the LV lock does not stop a second writer of a VM disk; only the dead host lease (sanlock + watchdog) does.
- Today's pools: `local` for root disks and `data-local` volumes (`<instance>-<volume>`), `backup` (dir) for backup copies on another member, `shared` (`APELORA_SHARED_POOL`) for `data-replicated` volumes and the root disks of modules that use them. Such volumes belong to no member (recorded with an empty `member`; created and read without `target`) and are not copied by moves. After a failed member is fenced they restart elsewhere (slice B, ADR 0013; see `reconciliation-ha`).
- **Backups and restore (built 2026-10-21, MOCK VERIFIED; the restore not yet REAL INCUS TESTED):** backups are `volume.backup` jobs (scheduled or `POST …/volumes/{volume}/backups`) copying through a short-lived snapshot into `APELORA_BACKUP_POOL` on another member, recorded in `volume_backups`. A restore is a `volume.restore` job queued with its audit row after a dry run (`apelora_domain::storage::plan_restore`) and the volume's name typed: refused while an instance using the volume runs; a safety backup first (recorded with the job, `before_restore`, migration 0024, so a retry does not take a second); a safe point; an Incus `refresh` copy of the backup over the volume (deletes the volume's snapshots; Incus would accept it under a running instance, so Apelora checks first and again after the safety backup). Failed restores are not retried as they are. Orphans: custom volumes in Apelora's projects that neither `volumes` nor `volume_backups` records (`GET /api/v1/storage/orphans`, read-only). Restoring onto another member when the volume's member is lost is not built. Runbook `docs/runbooks/volumes.md`.
- Never promise stateful host failover without tested shared/replicated storage AND fencing/single-writer assurance.
- Require explicit RPO/RTO, backup schedule, independent backup copies and restore verification.
- Snapshots are not backups; test data consistency and restores, including upgrade rollback.
- Present a capability/eligibility error instead of silently choosing a non-HA pool.
- Test loss of one storage node, stale mount, network split and restart ownership.
- **Apelora's own database** (ADR 0011 decision 6; runbook `docs/runbooks/apelora-database-backup.md`): `scripts/ops/db-backup.sh` dumps it (`pg_dump --format=custom`) to a directory **outside the managed cluster** (refuses missing, relative and checkout paths), with a `.sha256`, a `.json` manifest (migrations fingerprint, row counts, audit trigger), retention and optional `age`/`openssl cms` encryption to public material only. `scripts/ops/db-restore-drill.sh` restores into a new scratch database (never the live one), verifies it (checksum, migrations, row counts, audit trigger refuses UPDATE/DELETE), drops it and writes `restore-drill-last.json` / `restore-drill-last-verified.json`. `scripts/ops/db-restore-new-host.sh` restores onto an empty database on another PostgreSQL and keeps it; it refuses the live server. `scripts/ops/wal-archive.sh` is the `archive_command`, `scripts/ops/db-basebackup.sh` takes a base backup with `--wal-method=none` and checks the required segment is in `APELORA_WAL_DIR`, and `scripts/ops/wal-restore.sh` is the `restore_command`. After changing any of these, or a migration that touches `audit_events` or `_sqlx_migrations`, run `scripts/ops/test-db-backup.sh`, `scripts/ops/test-db-archive.sh` and `scripts/ops/test-db-restore-host.sh` (CI runs them in the Rust job; locally set `APELORA_PG_CLIENT_IMAGE=postgres:18-alpine` when the client tools are not installed). `scripts/ops/test-db-units.sh` starts the example backup and drill units; it skips where systemd is not running. The Console reads the last verified restore from the backup directory. None of this has been run on a production host.

## Evidence required before calling a feature complete

Explain what changed, actual tests and commands run, source/API reference, remaining safety constraints, and whether tested against real Incus or only mocks.

## Primary references

- https://linuxcontainers.org/incus/docs/main/
- https://github.com/lxc/incus
- https://doc.rust-lang.org/cargo/reference/workspaces.html

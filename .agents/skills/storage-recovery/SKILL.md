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
- Shared storage options: clustered LVM (`lvmcluster`) on a SAN, which the lab runs as pool `shared` on an iSCSI LUN (`scripts/test-cluster/shared-storage.sh`; Apelora places nothing on it yet), or distributed storage that Incus supports (evaluate through the official docs). Each needs its own components and monitoring.
- Today's pools: `local` for root disks and `data-local` volumes (`<instance>-<volume>`), `backup` (dir) for backup copies on another member. Restarting instances from `shared` after a failed member is fenced is slice B of `docs/design/mobility-and-ha.md`, not built.
- Never promise stateful host failover without tested shared/replicated storage AND fencing/single-writer assurance.
- Require explicit RPO/RTO, backup schedule, independent backup copies and restore verification.
- Snapshots are not backups; test data consistency and restores, including upgrade rollback.
- Present a capability/eligibility error instead of silently choosing a non-HA pool.
- Test loss of one storage node, stale mount, network split and restart ownership.

## Evidence required before calling a feature complete

Explain what changed, actual tests and commands run, source/API reference, remaining safety constraints, and whether tested against real Incus or only mocks.

## Primary references

- https://linuxcontainers.org/incus/docs/main/
- https://github.com/lxc/incus
- https://doc.rust-lang.org/cargo/reference/workspaces.html

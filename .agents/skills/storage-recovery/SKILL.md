---
name: storage-recovery
description: Use for Incus storage pools, persistent volumes, backups, disaster recovery and stateful HA.
---

# Persistent Storage and Data Safety

## When to use

Use for Incus storage pools, persistent volumes, backups, disaster recovery and stateful HA.

## Required workflow and guardrails

- Classify each volume: ephemeral root, persistent exclusive, shared read-only, backup, secret. Record attachment, ownership and relocation policy.
- Distinguish local ZFS and true shared/replicated storage. A local pool across multiple members is not shared data.
- Evaluate Incus Ceph RBD and LINSTOR through official docs; both need separate operating systems/components and monitoring.
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

---
name: reconciliation-ha
description: Use for replicas, health checks, deployment worker, failure detection and failover decisions.
---

# Desired State and HA Reconciliation

## When to use

Use for replicas, health checks, deployment worker, failure detection and failover decisions.

## Required workflow and guardrails

- Define Desired, Observed and LastKnown states separately, with timestamps, source evidence and operation correlation.
- Classify node heartbeat, Incus instance state, app readiness, ingress health and storage integrity separately. Running != healthy.
- For stateless replicas: rollout gates, known stable IDs, bounded retry, distinct node preference, cooldown and healthy ingress update.
- Avoid duplicate mutations with durable per-resource lease/lock/fencing token and safe idempotency.
- For stateful single writers: block automated failover unless old writer is fenced, shared storage safe, data ownership exclusive and database plan verified.
- Incus `cluster.healing_threshold` is disabled by default; never enable automatically in new environments.
- Record audited timeline for failure detection, decision, request, operation completion and post-checks.
- Test worker crashes, stale probes, duplicate requests, network partitions, storage outages, recovery/failback.

## How Apelora does it today (Phase 08 slice 1)

- **Availability is a class property** (`runtime.availability`: `stateless` | `singleton` (default) | `stateful`). Only stateless appliances get replicas (1–9; `DesiredAppliance::validate`, `MAX_REPLICAS`); names `<app>-<appliance>`, `-r2`…; label `user.apelora.replica`.
- **Desired state is recorded** per (application, project) in `application_deployments` by the deploy API; an empty deployment deletes the record (nothing left to supervise).
- **The supervisor** (`services/deployment-worker/src/supervisor.rs`) is level-triggered: plan the record against Incus, and for each missing *stateless* replica queue an ordinary `instance.create` job — skipped while `job_in_flight`, and for `BACKOFF_SECS` after `job_failed_recently`. It never creates or deletes anything itself, never touches singleton/stateful instances (warns only), audits `application.replica.recreate`, and queues `network.sync` so address sets include the new address. Unreadable inventory → skip the pass, never guess.
- **Stopped replicas:** a stateless replica that is `Stopped` while its spec says `start` is restarted (`instance.start`, audited `application.replica.restart`) — unless its latest lifecycle job is `instance.stop` (`last_lifecycle_job`): a stop made through Apelora is the operator's intent.
- **Ingress during failure:** service addresses are OVN load balancers with health checks (see `fabric-networking`); they drain a failed replica in seconds, independently of the supervisor.
- **Anti-affinity is Incus's placement**, steered by `infra/incus/placement.star` (installed by the Incus operator; cluster-wide). Call scriptlet helpers with keyword arguments (positional `get_instances` returned nothing on 7.0.1). Never set targets from Apelora.
- **Tests:** mock (`supervisor::tests`, API `stateless_replicas_are_deployed_recorded_and_restored`) and REAL (`real_incus.rs` `replicas_spread_across_members_and_lost_ones_return`; prove the scriptlet matters with a control run without it under skewed load).

## Evidence required before calling a feature complete

Explain what changed, actual tests and commands run, source/API reference, remaining safety constraints, and whether tested against real Incus or only mocks.

## Primary references

- https://linuxcontainers.org/incus/docs/main/
- https://github.com/lxc/incus
- https://doc.rust-lang.org/cargo/reference/workspaces.html

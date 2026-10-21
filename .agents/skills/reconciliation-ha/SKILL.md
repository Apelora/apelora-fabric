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

- **Availability is a class property** (`runtime.availability`: `stateless` | `singleton` (default) | `stateful`). Only stateless modules get replicas (1–9; `DesiredModule::validate`, `MAX_REPLICAS`); names `<app>-<module>`, `-r2`…; label `user.apelora.replica`.
- **Desired state is recorded** per (application, project) in `application_deployments` by the deploy API; an empty deployment deletes the record (nothing left to supervise).
- **The supervisor** (`services/deployment-worker/src/supervisor.rs`) is level-triggered: plan the record against Incus, and for each missing *stateless* replica queue an ordinary `instance.create` job — skipped while `job_in_flight`, and for `BACKOFF_SECS` after `job_failed_recently`. It never creates or deletes anything itself, never touches singleton/stateful instances (warns only), audits `application.replica.recreate`, and queues `network.sync` so address sets include the new address. Unreadable inventory → skip the pass, never guess.
- **One supervisor at a time (leader lease, ADR 0011).** `supervise` runs passes only while this worker holds the `replica-supervisor` row of `leader_leases` (`apelora_persistence::coordination`: `acquire_leadership` takes a free or expired lease and bumps `generation`; `renew_leadership` every `LEADER_RENEW_EVERY` 5 s on its own task; lost after `LEADER_LEASE_SECS` 15 s). Every supervisor write — a queued job with its audit row (`queue_audited`), a `network.sync`, the health records — runs in `begin_fenced(pool, lease)`, which checks holder, generation and expiry and holds the row `FOR SHARE` until commit; a stale leader gets `StoreError::Conflict` and writes nothing. A new leadership (`pass_if_leader` sees a new generation) clears the in-memory state (`offline_since`, reports, health trackers), so recovery delays start over after a takeover. Leadership changes are logged at info (`leader: lease acquired|lost`). Any new supervisor write must go through the fenced transaction, and tests use `pass_if_leader`/`LeaderLease` (`two_supervisors_on_one_database_only_the_leader_acts`, `a_leader_that_stops_renewing_is_replaced_and_its_late_writes_are_refused`). A second role that must be a singleton gets its own lease name, not a new mechanism.
- **Stopped replicas:** a stateless replica that is `Stopped` while its spec says `start` is restarted (`instance.start`, audited `application.replica.restart`) — unless its latest lifecycle job is `instance.stop` (`last_lifecycle_job`): a stop made through Apelora is the operator's intent.
- **Failed members (replace, then retire):** slots are matched by labels (`plan::slot_instances`); substitutes are `<replica>-s<n>` with the same labels. The `Supervisor` keeps `offline_since` per member (hysteresis, `APELORA_RECOVER_AFTER_SECS`); a stateless slot whose instances are all on failed members gets a substitute; once every instance of a slot is reachable again **and running** (and no retirement is already in flight for it — never decide a slot twice), `keeper` keeps the running one on the member with the fewest siblings and the others are deleted (`application.replica.retire`). Never delete an instance whose member is unreachable; never act on an inventory that failed to decode.
- **Incus/OVN realities** (TEST-CLUSTER.md): instances on an offline member come back with `config: null` (`Instance.unreachable`); OVN health checks cannot see a dead member, so the load balancer only recovers when Apelora replaces the backend; LB updates may be refused while a member is going down (sync retried after 60 s); a lost health check shows as `unknown` in the LB state and is re-applied by `network.sync`.
- **Evacuation is Incus's**, previewed by Apelora: `apelora_domain::evacuation::plan_evacuation` (pure; inventory + recorded deployments) explains each instance (mode → outcome, availability → effect), blocks when nothing can take an instance, warns about replicas sharing the last member. The API re-runs it before `member_state` and refuses while blocked. The supervisor skips instances on members that are not Online. Evacuated OVN instances keep their addresses (REAL INCUS TESTED), so address sets and load balancers stay valid.
- **Health with hysteresis** (`apelora_domain::health`, `services/deployment-worker/src/health.rs`): the signal is the OVN load balancer's per-backend port check (`load_balancer_health`: `online`/`offline`/`unknown`; backend `r<n>[-s<k>]` → instance via `backend_instance`) — a TCP check, not an application request; instances without a service address are not checked. A `Tracker` per running instance on an online member turns unhealthy after failing for `fail_after` (20 s, `APELORA_HEALTH_FAIL_AFTER_SECS`) and healthy after passing for `pass_after` (20 s); `unknown` neither confirms nor clears. A stateless replica unhealthy for `restart_after` (60 s, `APELORA_HEALTH_RESTART_AFTER_SECS`) gets an `instance.restart` job (a one-shot goal: done once one restart operation finished, also after a crash), audited `application.replica.restart-unhealthy`; at most one replica per module per pass, least recently restarted first, and not the same instance again within `restart_cooldown` (300 s) — a replica that stays broken is reported, not restarted in a loop. Singleton/stateful modules are never restarted. The worker records the decision in `instance_health`; the status API reports it for running instances only (`ModuleStatus.health`) and the Studio treats unhealthy replicas as not serving (`Degraded`/`Unhealthy`). Trackers live in the worker's memory: a worker restart starts over (conservative).
- **Ingress during failure:** service addresses are OVN load balancers with health checks (see `fabric-networking`); they drain a failed replica in seconds, independently of the supervisor.
- **Anti-affinity is Incus's placement**, steered by `infra/incus/placement.star` (installed by the Incus operator; cluster-wide). Call scriptlet helpers with keyword arguments (positional `get_instances` returned nothing on 7.0.1). Never set targets from Apelora.
- **Tests:** mock (`supervisor::tests`, API `stateless_replicas_are_deployed_recorded_and_restored`) and REAL (`real_incus.rs` `replicas_spread_across_members_and_lost_ones_return`; prove the scriptlet matters with a control run without it under skewed load).

## Evidence required before calling a feature complete

Explain what changed, actual tests and commands run, source/API reference, remaining safety constraints, and whether tested against real Incus or only mocks.

## Primary references

- https://linuxcontainers.org/incus/docs/main/
- https://github.com/lxc/incus
- https://doc.rust-lang.org/cargo/reference/workspaces.html

## Moves and live migration (docs/design/mobility-and-ha.md)

- Planned moves: `instance.move` job, planned by `apelora_domain::mobility::plan_move`. VMs with `migration.stateful=true` move live (measured: under one second, no reboot); containers and VMs with `data-local` volumes stop, their volumes are copied to the target member, the old copies are deleted only once the instance runs there.
- The deploy project needs `restricted.cluster.target=allow`; without it Incus refuses targeting for the restricted identity.
- Unplanned failures: live migration cannot help. Stateless replicas cover them today. Planned (slice B, not built): instances on a shared pool, containers or VMs, restart on another member once the failed one is fenced; the lab's `shared` pool exists for this, but Apelora places nothing there yet. Never enable Incus's `cluster.healing_threshold` without an automatic fence.

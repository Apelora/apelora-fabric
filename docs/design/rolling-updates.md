# Updating running modules (rolling replacement and rollback)

Status: draft · Date: 2026-10-25 · Related: [application deployment](application-deployment.md) (lists "replacing an instance whose image changed, rollback" as out of scope), [ADR 0015](../adr/0015-application-lifecycle-verbs.md), [application readiness](application-readiness.md)

## 1. Problem and scope

A deployment plan knows five actions: create, keep, resize, remove, conflict (`crates/domain/src/plan.rs`). When a new version of a design changes a module's **image** or its **settings** (`environment.*`, deploy-time fields, live migration), every running replica is **kept**: the plan says "already deployed" and nothing changes. Deploying an earlier version from the Versions tab (a rollback) has the same result. The person deploying is not told.

**Goals**

1. A plan says, per instance, when what runs differs from what the design asks for (image, settings), and what deploying will do about it.
2. Deploying replaces changed instances: stateless replicas one at a time, each new one healthy before the next goes, so the service address keeps answering.
3. Deploying an earlier version is a real rollback, through the same path.
4. A replacement that fails stops the rollout, keeps the replicas that still run the previous version, and says so.

**Non-goals (first slices):** replacing modules with data in place (volumes are kept and re-attached only in slice 3), canary percentages, traffic shifting by weight, scheduled maintenance windows.

**Assumptions:** replicas of a stateless module are interchangeable behind its service address (true today: OVN load balancer with health checks, ADR 0004); a single-replica module has an outage while it is replaced, and the plan must say so and ask, as Stop and Restart already do (ADR 0015).

## 2. Design

**What an instance was built from.** At creation the worker writes `user.apelora.spec` = SHA-256 of the canonical JSON of the parts of the spec that a replacement must apply: image source (server, protocol, alias, fingerprint), kind, caller config except resizable limits, volumes' paths and classes. Limits stay out (they are resized in place). Apelora's own discovery keys (`environment.APELORA_*`) stay out (they are re-synced by `network.sync`).

**Planning.** `plan()` reads the label (exposed on `Instance` like `limits`) and compares it with the hash of the desired spec:

| Observed | Action |
|---|---|
| no label (created before this change) | `keep`, with the reason "built before Apelora recorded its settings; redeploy to replace" |
| label equal | `keep` / `resize` as today |
| label differs | `replace`, `changes` naming what differs (image, settings) |

**Applying (stateless, slice 2).** The deployment queues one `instance.replace` job per module (not per replica): for each replica in turn, create a replacement beside it with the new spec, wait until the load balancer reports it healthy (same hysteresis as the supervisor), then delete the old instance. A replacement is named `<replica>-u<n>`, and the replica's slot recognises it the way it already recognises a substitute (`-s<n>`), so no instance is renamed. A replica that does not become healthy within the module's readiness window stops the job: the new instance is deleted, the old ones keep serving, the job fails with the reason, and the application's status says "update stopped".

**Single replica or stateful (slice 3).** Stop, recreate with the same volumes attached, start; needs `confirmDisruption`, like Stop.

## 3. Slices

1. **Visible** (small): record `user.apelora.spec` at creation; the plan shows `replace` (not yet applied: the deploy dialog says the running instance keeps its previous image/settings, so nobody is misled). Tests: plan unit tests for each row of the table above.
2. **Stateless rolling replacement** with health gating and stop-on-failure; REAL INCUS TESTED with `demo.sh` extended (change the NGINX image digest, probe the service address throughout: no failed request).
3. **Single replica and stateful** replacement with confirmation.

## 4. Failure handling

| Failure | Behaviour |
|---|---|
| New replica never healthy | delete it, keep the old ones, job fails with the reason |
| Worker dies mid-replacement | the job's lease expires; the next worker resumes from the recorded step (both instances exist: keep the healthy one) |
| Member of the new replica fails | the supervisor's substitute logic applies to the new replica |
| Two deployments race | the second refuses while an `instance.replace` job of the application is running |

# Updating running modules (rolling replacement and rollback)

Status: slice 1 built (2026-10-25); slices 2 and 3 to do · Date: 2026-10-25 · Related: [application deployment](application-deployment.md) (lists "replacing an instance whose image changed, rollback" as out of scope), [ADR 0015](../adr/0015-application-lifecycle-verbs.md), [application readiness](application-readiness.md)

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

**What an instance was built from.** At creation the adapter writes `user.apelora.spec` (`apelora_domain::plan::spec_fingerprint`): a short SHA-256 per part, `image=…,settings=…,storage=…`, so the plan can say *which* part differs. `image`: kind, image source (protocol, server, alias, fingerprint) and architectures. `settings`: instance config and profiles. `storage`: the root disk pool and each volume's name, class, content and path. Left out: resource limits (resized in place), Apelora's discovery keys (`environment.APELORA_*`, re-synced by `network.sync`) and labels, volume sizes (grown in place) and pools (resolved from the class). The fingerprint is taken from the spec as designed, before an image mirror rewrites the source, so a mirror does not make everything look outdated. Callers cannot set it (`user.apelora.*` is reserved).

**Planning.** `plan()` reads the label (exposed on `Instance` like `limits`) and compares it with the hash of the desired spec:

| Observed | Action |
|---|---|
| no label (created before this change) | `keep`, with the reason "built before Apelora recorded its settings; redeploy to replace" |
| label equal | `keep` / `resize` as today |
| label differs | slice 1: `keep`/`resize` with `outdated` naming what differs (image, settings, storage); slice 2: `replace` |

**Applying (stateless, slice 2).** The deployment queues one `instance.replace` job per module (not per replica): for each replica in turn, create a replacement beside it with the new spec, wait until the load balancer reports it healthy (same hysteresis as the supervisor), then delete the old instance. A replacement is named `<replica>-u<n>`, and the replica's slot recognises it the way it already recognises a substitute (`-s<n>`), so no instance is renamed. A replica that does not become healthy within the module's readiness window stops the job: the new instance is deleted, the old ones keep serving, the job fails with the reason, and the application's status says "update stopped".

**Single replica or stateful (slice 3).** Stop, recreate with the same volumes attached, start; needs `confirmDisruption`, like Stop.

## 3. Slices

1. **Visible** (built 2026-10-25): record `user.apelora.spec` at creation; the plan keeps the instance (`keep` or `resize`) and lists the differing parts in `outdated`, with the reason and a warning that deploying does not update running modules yet. A `replace` action is deliberately not added before slice 2: an action that deploying does not carry out would be counted as a change and mislead. The Studio's review marks such rows *Keep · outdated*. Tests: planner unit tests for each row of the table above and for what must not count (limits, discovery keys, volume size); the lab test through the screens publishes a version with another NGINX channel and sees both running replicas marked outdated.
2. **Stateless rolling replacement** with health gating and stop-on-failure; REAL INCUS TESTED with `demo.sh` extended (change the NGINX image digest, probe the service address throughout: no failed request).
3. **Single replica and stateful** replacement with confirmation.

## 4. Failure handling

| Failure | Behaviour |
|---|---|
| New replica never healthy | delete it, keep the old ones, job fails with the reason |
| Worker dies mid-replacement | the job's lease expires; the next worker resumes from the recorded step (both instances exist: keep the healthy one) |
| Member of the new replica fails | the supervisor's substitute logic applies to the new replica |
| Two deployments race | the second refuses while an `instance.replace` job of the application is running |

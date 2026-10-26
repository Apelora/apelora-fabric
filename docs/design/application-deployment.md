# Deploying an application from the Studio

Status: agreed (Phase 06, slice 1 implements the backend, slice 2 the Studio panel) · Date: 2026-10-20

## 1. Problem and scope

A designer has a graph of modules in the Studio and wants it running on the cluster, see what will change before anything changes, and see afterwards which modules are running.

**In scope:** one instance per deployable module; create what is missing, keep what exists, optionally remove instances of the application that are no longer in the design; immutable revisions; live status per module.
**Out of scope (later phases):** replicas and placement (08), networks and connections between modules (07), readiness probes beyond "Running", replacing an instance whose image changed, rollback.

**Assumptions:** the catalog (and so the choice of image per module) lives in TypeScript; the Studio turns each module into an `InstanceSpec` with `instanceRequestFor`. The server does not trust that choice more than any other request with the `deploy` permission: it validates every spec as it validates `POST /api/v1/instances`.

## 2. Design

![From a design to running instances](../assets/diagrams/design-to-cluster.svg)


```text
Studio ──(document + one InstanceSpec per deployable module)──▶ API
API: validate → plan (pure: desired vs observed instances) ──▶ preview
API (apply): recompute plan → refuse on conflicts → save revision → queue jobs ──▶ worker ──▶ Incus
Studio ◀── GET /applications/{name}/status (instances carrying the application's labels)
```

- **Names:** a module's instance is `<application>-<module id>` (e.g. `commerce-stack-nginx-01`), at most 63 characters, so a design maps to instances by stable ids, never by labels or order.
- **Ownership labels:** besides `user.apelora.managed=true`, Apelora writes `user.apelora.application` and `user.apelora.module`. Only Apelora sets `user.apelora.*` keys.
- **Plan** (`apelora_domain::plan`), per desired module: no instance → **create**; an instance with the same name that carries this application's and module's labels → **keep**; an instance that is this module's but whose own `limits.cpu`, `limits.cpu.allowance` or `limits.memory` differ from the design → **resize** (the plan item lists each change; nothing else is compared in place); any other instance with that name → **conflict** (not Apelora's, or another application's). Instances labelled with the application but no longer in the design → **remove** (applied only when the caller asks for it).
- **Apply** recomputes the plan from current inventory (the preview may be stale), refuses the whole deployment on any conflict, saves the document as an immutable revision, then queues `instance.create` / `instance.resize` / `instance.delete` jobs (a resize sets the changed limits with one instance update and does not stop the instance; quotas count only its increase) (idempotency keys derived from the caller's key), audits `application.deploy.requested`, and returns the job ids. The worker's convergence rules (ownership marker, image pins, architectures) apply unchanged.
- **Replicas** (Phase 08): a stateless module runs 1–9 interchangeable instances (`<application>-<module>`, then `-r2`…, labelled `user.apelora.replica`); singletons and stateful modules run one. The deployment's desired state is recorded per application and project; the worker's supervisor recreates a lost stateless replica through the same job queue (never while a job for it is in flight; five-minute back-off after a failure; audited as `application.replica.recreate`) and re-syncs the network address sets. Spreading replicas is Incus's job, steered by the placement scriptlet `infra/incus/placement.star` (fewest replicas of the module per member, then fewest project instances); Apelora never picks a target itself.
- **Service addresses:** with `APELORA_SERVICE_ADDRESSES`, a stateless module that others connect to gets an OVN load balancer on the application network (listen address from that routed range, allocated in PostgreSQL; backends are its replicas; health checks on), so consumers use one address while replicas come and go. A replica stopped outside Apelora is restarted by the supervisor; one stopped through Apelora stays stopped.
- **Status** lists the application's instances from the inventory with their module id, so the Studio can overlay state on the saved design without rewriting it.

## 3. Failure handling

| Failure | Behaviour |
|---|---|
| Name already used by something else | Plan shows a conflict; apply refuses with 409 and changes nothing |
| Inventory changes between preview and apply | Apply recomputes; the response carries the plan actually applied |
| A create job fails (image, architecture, Incus) | Job retries or fails as today; status shows the module as missing; the next deploy creates it again |
| Repeated click / network retry | Same `Idempotency-Key` returns the same jobs |
| Control plane restarts after queuing | Jobs are durable; the worker carries them out |
| A stateless replica disappears | The supervisor queues its recreation once; Incus places it away from its siblings |
| A stateless replica stops (crash) | The load balancer stops sending it traffic within seconds; the supervisor starts it again |
| A singleton or stateful instance disappears | Reported (worker log, status shows it missing); never recreated automatically |

## 4. Security

`deploy` permission and the project allow-list as for single instances; labels are written only by Apelora; the saved document is the caller's own design (no secrets: secret properties hold references only).

## 5. Next steps

Slice 1: domain types and planner, adapter labels, API (plan, deploy, status, list), tests including a real Incus run. Slice 2: Studio deploy panel (sign-in, project, plan preview, approve, follow jobs, status overlay), browser tests, real-browser check.

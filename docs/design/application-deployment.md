# Deploying an application from the Studio

Status: agreed (Phase 06, slice 1 implements the backend, slice 2 the Studio panel) · Date: 2026-10-20

## 1. Problem and scope

A designer has a graph of appliances in the Studio and wants it running on the cluster, see what will change before anything changes, and see afterwards which appliances are running.

**In scope:** one instance per deployable appliance; create what is missing, keep what exists, optionally remove instances of the application that are no longer in the design; immutable revisions; live status per appliance.
**Out of scope (later phases):** replicas and placement (08), networks and connections between appliances (07), readiness probes beyond "Running", replacing an instance whose image changed, rollback.

**Assumptions:** the catalog (and so the choice of image per appliance) lives in TypeScript; the Studio turns each appliance into an `InstanceSpec` with `instanceRequestFor`. The server does not trust that choice more than any other request with the `deploy` permission: it validates every spec as it validates `POST /api/v1/instances`.

## 2. Design

```text
Studio ──(document + one InstanceSpec per deployable appliance)──▶ API
API: validate → plan (pure: desired vs observed instances) ──▶ preview
API (apply): recompute plan → refuse on conflicts → save revision → queue jobs ──▶ worker ──▶ Incus
Studio ◀── GET /applications/{name}/status (instances carrying the application's labels)
```

- **Names:** an appliance's instance is `<application>-<appliance id>` (e.g. `commerce-stack-nginx-01`), at most 63 characters, so a design maps to instances by stable ids, never by labels or order.
- **Ownership labels:** besides `user.apelora.managed=true`, Apelora writes `user.apelora.application` and `user.apelora.appliance`. Only Apelora sets `user.apelora.*` keys.
- **Plan** (`apelora_domain::plan`), per desired appliance: no instance → **create**; an instance with the same name that carries this application's and appliance's labels → **keep**; any other instance with that name → **conflict** (not Apelora's, or another application's). Instances labelled with the application but no longer in the design → **remove** (applied only when the caller asks for it).
- **Apply** recomputes the plan from current inventory (the preview may be stale), refuses the whole deployment on any conflict, saves the document as an immutable revision, then queues `instance.create` / `instance.delete` jobs (idempotency keys derived from the caller's key), audits `application.deploy.requested`, and returns the job ids. The worker's convergence rules (ownership marker, image pins, architectures) apply unchanged.
- **Status** lists the application's instances from the inventory with their appliance id, so the Studio can overlay state on the saved design without rewriting it.

## 3. Failure handling

| Failure | Behaviour |
|---|---|
| Name already used by something else | Plan shows a conflict; apply refuses with 409 and changes nothing |
| Inventory changes between preview and apply | Apply recomputes; the response carries the plan actually applied |
| A create job fails (image, architecture, Incus) | Job retries or fails as today; status shows the appliance as missing; the next deploy creates it again |
| Repeated click / network retry | Same `Idempotency-Key` returns the same jobs |
| Control plane restarts after queuing | Jobs are durable; the worker carries them out |

## 4. Security

`deploy` permission and the project allow-list as for single instances; labels are written only by Apelora; the saved document is the caller's own design (no secrets: secret properties hold references only).

## 5. Next steps

Slice 1: domain types and planner, adapter labels, API (plan, deploy, status, list), tests including a real Incus run. Slice 2: Studio deploy panel (sign-in, project, plan preview, approve, follow jobs, status overlay), browser tests, real-browser check.

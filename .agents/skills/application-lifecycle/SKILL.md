---
name: application-lifecycle
description: Work on application management — drafts, published versions, derived deployment status, and Start/Restart/Stop operations — in the control plane, worker and Studio. Use for the Applications pages, lifecycle buttons, operations, or status derivation.
---

# Application lifecycle

## Model

- **Draft**: editable, versioned (`version`); saving names its base version. A stale save gets `409 draft_conflict` with `currentVersion`; the Studio shows a conflict dialog (load newest, or overwrite deliberately). Nothing is overwritten silently.
- **Published version**: immutable, numbered. **Deployments reference versions, never drafts.** A draft cannot be started or deployed.
- **Status** is derived on the server (`crates/domain/src/lifecycle.rs`, `derive_status`) from the desired replicas and what Incus reports: `not-deployed`, `starting`, `running`, `degraded`, `stopping`, `stopped`, `failed`, `unknown`. If Incus cannot be observed the answer is `unknown` and **stale**; it is never shown as healthy.
- **Actions** (`start`, `restart`, `stop`) are returned per caller on every application, each `{allowed, reason}`, combining permission and state. The UI renders exactly that.

## Button rules (as implemented)

| State | Start | Restart | Stop |
|---|---|---|---|
| Stopped | enabled | disabled | disabled |
| Running | disabled | disabled | enabled |
| Degraded | disabled | enabled | enabled |
| Failed | enabled | enabled | enabled |
| Starting / Stopping / operation in progress | disabled | disabled | disabled |
| Not deployed / Unknown / no permission | disabled | disabled | disabled |

Disabled buttons carry the reason as a title. Running exposes **Stop only** (the user asked for Start and Restart greyed while running); Restart is offered when something is wrong.

## Operations

- One active operation per application (unique index). A second request gets `operation_in_progress`.
- An operation is a coordinator job `application.operation` with phases in dependency order from the recorded connections (stop: dependents first; start/restart: providers first), and per-instance jobs `instance.start|stop`.
- **Success is declared only when Incus confirms each instance.** The UI polls the operation and keeps buttons pending until the backend says it finished; a failure shows the real error and the real state afterwards.
- Verbs (ADR 0015): Start, Restart, **Shut down** (`shutdown`, graceful: the old `stop`), **Stop** (`stop`, immediate power-off: the instance jobs carry `force`; phase action `poweroff`) and **Migrate** (`migrate` with `target`, one `instance.move` per instance, checked by `GET /applications/{name}/migration` first; status `migrating`). Old operation rows saying `stop` were graceful. Studio: `LifecycleActions` and `MigrateDialog`; Console: Shut down… and Stop… (it falls back to the old graceful `stop` against a control plane without `shutdown`).
- Shut down, Stop and Restart ask first; if clients would lose service the API answers `409 confirmation_required` and the dialog asks again with a stronger warning (`confirmDisruption: true`).
- Authorisation is per application (`application.operate`); lifecycle requests are audited, including denials.
- **Update** (`update`, issue #48, [design](../../../docs/design/rolling-updates.md)): created by a deployment whose plan has `replace` items (outdated stateless replicas), never through `POST …/operations` (400). Per replica: `create` (`instance.create` of `<replica>-u<n>`), `healthy` and `drain` (no instance job; looked at by `rollout.rs` on every pass, progress kept in the step's `payload`), `delete`. A failed `create`/`healthy` deletes the update, a failed `drain` undrains the old replica; the error starts "update stopped:". Status `updating`; another deployment is refused while it runs; the supervisor skips the application's slots and health restarts meanwhile.
- **Cancelling an operation** (`POST /api/v1/jobs/{coordinator}/cancel`, `application.operate`) is honoured between phases: the running phase's instance jobs finish, later phases are `skipped`, and the operation ends in state `cancelled` (migration 0023 added it) with `error` naming who cancelled and where. A coordinator is never cancelled "at once", even while it waits between polls, because it must close its operation record (and the one-active-operation index). Operations are **not retried** as jobs (`409 not_retryable`): start, stop or restart again, which plans anew. The Studio toasts a cancelled operation as a warning, not a failure.
- **Single instances** in the Console use the same job path: Start, Stop and Restart on the instance page queue `instance.start|stop|restart` (`POST /api/v1/instances/{project}/{name}/state`, `instance.manage`); there is no separate restart route.

## Studio structure

`features/applications/` (model, `apiBackend`, `localBackend`, `lifecycle` store, `draftSync`, `prefs`, `icons`), `components/applications/` (page, cards, rows, `ApplicationActions` = `DeployAction` + `LifecycleActions`, workspace tabs, dialogs; Import on the list and Export in the row menu through `features/applications/transfer.ts`), routes in `features/router.ts` (`/studio/applications[/:name[/:tab]]`; `/` stays the Assembly Library). The browser-only backend can design and publish but reports runtime `unknown` and offers no lifecycle action.

## Testing

`tests/e2e/studio-applications.spec.ts` covers both backends (local; stubbed control plane) including state→button rules, pending-until-confirmed, failure display, confirmation flow, stale status, parity of cards and list, pagination and axe. Rust: `services/control-plane/src/api/manage_tests.rs`, `services/deployment-worker` operation tests, `crates/domain/src/lifecycle.rs`. Label results honestly: these use mock Incus, not a real cluster.

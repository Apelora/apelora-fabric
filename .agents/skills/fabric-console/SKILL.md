---
name: fabric-console
description: Build and change the Apelora Fabric Console (apps/fabric-console) — the administrator UI for cluster status, instances, deployments, preflight, networks and storage, membership (machine enrolment, evacuate/restore/remove members, join tokens) and the audit log. Use for any console view, action, dialog or API wiring.
---

# Fabric Console

The console is a thin, honest client of the Rust API: every action goes through `@apelora/api-client` (generated from the OpenAPI document), never to Incus. It follows the Studio visual contract (`studio-design-system`, AGENTS.md rule 5).

## Structure

| Path | What |
|---|---|
| `src/App.tsx` | Shell: 48px header, side navigation (wraps into rows under `md`), hash routing (`#/instances`), health check, sign-in, `whoami` |
| `src/lib/api.ts` | `client`, token in `sessionStorage` only, `useLoad` (401 → sign-in), `followJob` (polls `GET /jobs/{id}` until final), `idempotencyKey` |
| `src/components/ui.tsx` | `Section`/`SectionHeader`/`Heading`, `SourceNote`, `ErrorPanel`, `TableFrame`, `ConfirmDialog`, `FormDialog` |
| `src/lib/identity.ts` | Identity helpers: `holds`/`holdsPlatform` (from `whoami.capabilities`/`platformPermissions`), `useDirectory` (names behind ids), `useRun` (one action, its error, 401 hand-off), `describeError`, scope/level vocabulary |
| `src/views/identity/*` | Identity & Access pages (`#/identity/<page>`, list in `pages.ts`): Users, Groups, Organizations (→ projects → environments), Project memberships, Roles & permissions, Service identities, Access policies (+ Approvals tab), Audit history; plus `AssignmentDialog` (server preview before saving), `EmergencyAccessDialog`, `PersonLookup` |
| `src/components/TerminalDialog.tsx` | Terminal into an instance (`target`: `{ application, instance }` or `null`), lazy-loads `TerminalSession.tsx` (access check with the server's reason, production approval request and its state, `@apelora/terminal` screen). Mount it from any view that shows an Apelora instance |
| `src/views/Sessions.tsx` | Sessions (`#/sessions`, under Identity & Access, offered for `instance.console`, `instance.desktop` or `application.operate`): open sessions with last input and time left, End session with confirmation, history from `identity_audit` (`action=session.`, needs `audit.read`) |
| `src/views/Health.tsx` | Health (`#/health`, needs `read-inventory`): component rows and server cards from `GET /health/components` and `/health/servers`, read every 20 s while visible (`usePolled`); a failed read keeps the last data with a notice of its age. States, reasons, next steps and warnings come from the server (`apelora_domain::capacity`) |
| `src/views/*.tsx` | One file per view: Applications (deployed applications with instances, port checks, service addresses and volumes; a kept volume is deleted here with its name typed), Instances (+ deploy dialog), Operations (job queue), Preflight, Infrastructure, Membership (Machines: add, approve, check and admit — `views/Machines.tsx`; members with evacuate/restore and remove (after a check, name typed); unused join tokens), Audit, Administration (API tokens, System, Image mirrors — `views/ImageMirrors.tsx`, read-only: mode, each mirror and whether it answers with the time checked, the image servers each deployable class needs and their coverage; Monitoring — `views/Monitoring.tsx`, read-only from `GET /api/v1/monitoring`) |

## Rules

1. **Show only what the API says.** Label the source of every inventory list (`SourceNote`: mock vs "From Incus · observed …"). Unknown values show as "—" or "unavailable", never guessed.
2. **Permissions:** hide views and actions the caller's `whoami.permissions` do not allow (Membership needs `manage-infrastructure`, Audit `read-audit`, actions `deploy`). The API enforces them anyway; the UI only avoids dead ends.
3. **Instance actions** only on `instance.managed` instances in `whoami.deployProjects`; others say "Not managed by Apelora". Each action sends an `Idempotency-Key`, then follows its job: the row shows "Stopping…", retries (`attempt n`) and the job's `lastError`; the list reloads when the job ends.
4. **Deploy** offers only catalog classes with a `runtime` and builds the body with `instanceRequestFor` (`@apelora/assembly-model`); the image choice follows the class's `imageBy` setting.
5. **Destructive actions** use `ConfirmDialog`: the consequence in the copy, Cancel focused by default, the action quieter (`btn-ghost text-error`); deleting an instance requires typing its name (focus goes to that field).
6. **Secrets shown once:** a join token or a new API token appears in a read-only field with "Copy token" and "Hide token"; it is never stored (not in state after hiding, not in storage, not in the URL).
7. **One page title:** views with a single section pass `level={1}` so their section header is the `h1`; multi-section views get the shell's `h1`. One `btn-primary` per view.
8. **Phones:** no sideways page scroll at 390px; secondary table columns use `max-sm:hidden` so row actions stay visible; on touch screens (`pointer: coarse`, `styles/index.css`) controls get a 44px hit area while desktop keeps compact `btn-sm`/`btn-xs`.

## Identity & Access

- A page is offered when the caller holds one of its permissions *somewhere* (`whoami.capabilities`); buttons use `capabilities` for scope-level permissions and `platformPermissions` for platform-only ones. The server still decides: show its message (403 as "Not allowed: …") and never rely on the UI check.
- Lists the caller may not read are "off", not empty (`useDirectory`); application scopes need a pasted application id because the API does not list ids yet.
- `AssignmentDialog` calls `preview_assignment` (debounced) and enables Save only when the preview for the current form says `allowed`; every effective permission shows its origin (role, scope, group, emergency).
- Secrets shown once (automation tokens) use `SecretOnceDialog`: the value lives only in the opener's state while the dialog is open.
- The client takes `query` for routes with query strings (`lookup_user`, `identity_audit`, `assignments`, `projects_list`).
- Tests: `tests/e2e/fabric-console-identity.spec.ts` with the stub in `tests/e2e/identity-fixtures.ts`.

## Verify

- `tests/e2e/fabric-console.spec.ts`: a `beforeEach` aborts every unstubbed `/api/**` call (tests must never reach a running control plane); `stub()` answers by method + path. Cover each view, each action's request body, confirmation behaviour, permission-based navigation, and the phone layout (`isMobile`, `hasTouch`).
- Real browser against the real stack (control plane + worker + PostgreSQL + the test cluster, `docs/development/TEST-CLUSTER.md`) before calling a console feature done; sign in with a short-lived token and revoke it afterwards.
- Design review (Better Design): `get-review-rules`, `check-comprehension` per view, `inspect-spacing` at 1440/1728/390. Keep the project's design system; do not restyle to another one.

## Patterns learned

- **Live status pages** (Health): poll only while `document.visibilityState === "visible"`, say how old the data is, keep the last data when a read fails (notice with its age, never an empty or invented state), do not put a ticking "… ago" text in an `aria-live` region, and show a busy button only for reads the person asked for. Bars show *used of total* with both numbers in text; the server decides thresholds and warnings.

- **High-impact actions** (evacuate a member, delete an instance): show what will happen first (a dry run from the API, never computed in the browser), then require the target's name typed; Cancel is the default focus and the destructive button stays secondary (`btn-ghost text-error`). `FormDialog wide` for dialogs holding a table.
- **API messages** are clauses; show them with `sentence()` (capital first letter) rather than changing the API text.
- **Lifecycle tables** (Machines): one row per object with its state in words (status dot + label), the API's `nextStep` and only the actions that state allows (at most two); the server derives state and next step, the browser never infers them. On phones (`useWide` below `sm`) the next step, warnings and actions move under the state, so the table never scrolls sideways; render actions in one place only (no hidden duplicates — they break test ids).
- **Sub-headings inside a section** (Image mirrors): wrap each sub-heading with its list in its own `flex flex-col gap-2` group and space the groups with one parent `gap-4`, rather than `mt-*` on the heading; `inspect-spacing` flags mixed sibling gaps.
- **Read-only status from the server** (Image mirrors): show the server's probe detail and the time it was checked; a section's load error stays in that section with its retry, so the rest of the page keeps working.
- **Several `ConfirmDialog`s on one view**: tests scope to `page.locator("dialog[open]")`.
- **Real-browser checks with a token**: never type it into the transcript; fulfil a routed URL from a git-ignored file and store it in `sessionStorage` after the first page load (a 401 on load clears it).

---
name: fabric-console
description: Build and change the Apelora Fabric Console (apps/fabric-console) — the administrator UI for cluster status, instances, deployments, preflight, networks and storage, membership (join tokens, evacuate/restore members) and the audit log. Use for any console view, action, dialog or API wiring.
---

# Fabric Console

The console is a thin, honest client of the Rust API: every action goes through `@apelora/api-client` (generated from the OpenAPI document), never to Incus. It follows the Studio visual contract (`studio-design-system`, AGENTS.md rule 5).

## Structure

| Path | What |
|---|---|
| `src/App.tsx` | Shell: 48px header, side navigation (wraps into rows under `md`), hash routing (`#/instances`), health check, sign-in, `whoami` |
| `src/lib/api.ts` | `client`, token in `sessionStorage` only, `useLoad` (401 → sign-in), `followJob` (polls `GET /jobs/{id}` until final), `idempotencyKey` |
| `src/components/ui.tsx` | `Section`/`SectionHeader`/`Heading`, `SourceNote`, `ErrorPanel`, `TableFrame`, `ConfirmDialog`, `FormDialog` |
| `src/views/*.tsx` | One file per view: Instances (+ deploy dialog), Operations (job queue), Preflight, Infrastructure, Membership (members with evacuate/restore, join tokens), Audit, Administration (API tokens, System) |

## Rules

1. **Show only what the API says.** Label the source of every inventory list (`SourceNote`: mock vs "From Incus · observed …"). Unknown values show as "—" or "unavailable", never guessed.
2. **Permissions:** hide views and actions the caller's `whoami.permissions` do not allow (Membership needs `manage-infrastructure`, Audit `read-audit`, actions `deploy`). The API enforces them anyway; the UI only avoids dead ends.
3. **Instance actions** only on `instance.managed` instances in `whoami.deployProjects`; others say "Not managed by Apelora". Each action sends an `Idempotency-Key`, then follows its job: the row shows "Stopping…", retries (`attempt n`) and the job's `lastError`; the list reloads when the job ends.
4. **Deploy** offers only catalog classes with a `runtime` and builds the body with `instanceRequestFor` (`@apelora/assembly-model`); the image choice follows the class's `imageBy` setting.
5. **Destructive actions** use `ConfirmDialog`: the consequence in the copy, Cancel focused by default, the action quieter (`btn-ghost text-error`); deleting an instance requires typing its name (focus goes to that field).
6. **Secrets shown once:** a join token or a new API token appears in a read-only field with "Copy token" and "Hide token"; it is never stored (not in state after hiding, not in storage, not in the URL).
7. **One page title:** views with a single section pass `level={1}` so their section header is the `h1`; multi-section views get the shell's `h1`. One `btn-primary` per view.
8. **Phones:** no sideways page scroll at 390px; secondary table columns use `max-sm:hidden` so row actions stay visible; on touch screens (`pointer: coarse`, `styles/index.css`) controls get a 44px hit area while desktop keeps compact `btn-sm`/`btn-xs`.

## Verify

- `tests/e2e/fabric-console.spec.ts`: a `beforeEach` aborts every unstubbed `/api/**` call (tests must never reach a running control plane); `stub()` answers by method + path. Cover each view, each action's request body, confirmation behaviour, permission-based navigation, and the phone layout (`isMobile`, `hasTouch`).
- Real browser against the real stack (control plane + worker + PostgreSQL + the test cluster, `docs/development/TEST-CLUSTER.md`) before calling a console feature done; sign in with a short-lived token and revoke it afterwards.
- Design review (Better Design): `get-review-rules`, `check-comprehension` per view, `inspect-spacing` at 1440/1728/390. Keep the project's design system; do not restyle to another one.

## Patterns learned

- **High-impact actions** (evacuate a member, delete an instance): show what will happen first (a dry run from the API, never computed in the browser), then require the target's name typed; Cancel is the default focus and the destructive button stays secondary (`btn-ghost text-error`). `FormDialog wide` for dialogs holding a table.
- **API messages** are clauses; show them with `sentence()` (capital first letter) rather than changing the API text.
- **Several `ConfirmDialog`s on one view**: tests scope to `page.locator("dialog[open]")`.
- **Real-browser checks with a token**: never type it into the transcript; fulfil a routed URL from a git-ignored file and store it in `sessionStorage` after the first page load (a 401 on load clears it).

# Lessons learned

What went wrong while building Apelora, why, and the rule we follow now. Every coding agent reads this before starting a task (AGENTS.md says so) and adds an entry when something fails for a reason that was not obvious.

## How to use this file

- **Before a task:** read the sections that touch your area. The rules here come from real failures.
- **Add an entry** when a check fails for a non-obvious reason, when a real system behaves differently from the mock or the documentation, when the owner corrects how we work, or when a fix needed more than one attempt.
- **Entry format:** one bullet, `**What happened** → cause → rule now`, with the date (YYYY-MM-DD) and, where useful, the file or test that now guards it. Keep it to three lines.
- **Keep it useful:** merge duplicates, and when a rule moves into code, a test or a skill, shorten the entry to a pointer. Delete entries that no longer apply.
- **Review it** at the end of each phase: look for repeated causes, which mean a process or tooling gap. Fix the gap, not just the instance.

## Patterns so far

Read these first. Most entries below come from one of four causes:

1. **The mock or stub was kinder than the real thing.** It answered instantly, had no limits and never failed. Model latency and limits in stubs, and test against a real local system before calling something done.
2. **A failure was silent.** Nothing in the logs or on screen said why. Every refusal and swallowed error gets a log line with identifiers, and the UI shows the server's reason.
3. **Check the result, not the step.** Wait for the observable result (an instance's location changed, the page shows data), not for "the job finished".
4. **Environment differences** between macOS and Linux, development and production builds, and mouse and touch input.

## Front end and browser

- **The provider's sign-in code was exchanged twice in development and the sign-in failed** (2026-10-21) → React StrictMode runs mount effects twice. The second run found the pending sign-in already used, failed, and triggered a request with no token. → Do one-shot side effects (code exchange, one-time tokens, POSTs on mount) once per page load through a shared promise (`finishOidcSignIn` in `apps/fabric-console/src/App.tsx`). Guard: the e2e test delays the token endpoint by 400 ms and expects exactly one exchange.
- **Stubbed tests passed while the real flow failed** (2026-10-21) → the stub answered at once, so the race never happened. → Give stubs of external services realistic latency, and finish UI work with one run against a real local service (`infra/dev`, profile `oidc`).
- **"Your session ended" when the real cause was a refused token** (2026-10-21) → the UI picked a generic reason. → Show the server's message when the cause is not the obvious one (`fromProvider` in `App.tsx`).
- **A Playwright test's second `goto` to another hash did not reload the page** → hash-only navigation keeps the old route stubs and state. → Use separate tests for separate stub setups, or `page.reload()`.
- **A stubbed identity provider failed in the browser** → no CORS headers. Real providers send them for registered origins. → Stubs of cross-origin services send `access-control-allow-origin`.
- **Row actions pushed Delete off screen at 390 px; 24 px buttons in measurements** → no wrapping, and the measurement ran without touch emulation, while the console makes controls 44 px on coarse pointers. → Use `flex-wrap justify-end` for action groups, and measure phones with `isMobile` and `hasTouch` on.
- **Screenshots caught dialogs mid-animation; Ctrl+click did nothing on macOS** → use `emulateMedia({ reducedMotion: "reduce" })` and `ControlOrMeta`.
- **The Studio showed the generic "saved sign-in was not accepted" after a refused provider token** (2026-10-21) → two places ran the same detection, and the one that did not do the exchange reported last. → Keep facts about the page load (which token the provider issued) at module level, not in one call's local variable (`providerToken` in `apps/studio/src/features/applications/backend.ts`).
- **A docs agent captured screenshots while the same page was being redesigned in the main checkout** (2026-10-21) → parallel work on one surface. → Do not run screenshot work for a page while it is being changed; regenerate shots after the design settles, in the same change as the design.
- **Tests broke when a fixture list grew** (permission catalogue counts) → expected when you add to a shared fixture: update the counts in the same change.

## Control plane and identity

- **The control plane refused a provider's token and logged nothing** (2026-10-21) → refusals returned 401 without a log line. → Each refused credential is logged at warn as `credential refused` with method, path, credential kind, issuer and reason, never the token. It is not audited, since one row per bad request would let anyone fill the audit log.
- **A real provider issues 24-hour tokens; Apelora accepts at most one hour** (2026-10-21) → provider defaults differ from our limits. → Document the limit next to the setting, configure the dev provider for one hour, and make the refusal reason visible.
- **A provider that is down at startup stayed refused until a restart** (2026-10-21) → discovery ran only at startup and on save. → Retry unreachable providers every minute (`auth_config::retry_unreachable`).
- **An LDAP bind with an empty password succeeds** (2026-10-21) → it is an anonymous ("unauthenticated") bind, not a failed one. → Refuse an empty password before connecting (`ldap::sign_in`; guarded by `an_empty_password_is_refused_before_anything_is_sent`).
- **`ldaps://LDAP.Example` kept its capitals after URL parsing** (2026-10-21) → the `url` crate lowercases hosts only for special schemes (http, https, …), not `ldap`/`ldaps`. → Lowercase the host ourselves (`directories::validate`): the directory URL is its people's issuer, so its spelling must be stable.
- **The development directory's `memberOf` was empty** (2026-10-21) → the `memberof` overlay does not run for offline `slapadd`. → The seed lists `memberOf` explicitly (`infra/dev/directory/seed.ldif`).
- **Any change to sign-in settings can lock everyone out** → the API enforces the guards, not just the UI (`lockout_guard`, `set_api_tokens`). Keep both guards when changing sign-in.

## Incus (REAL INCUS TESTED behaviour that the documentation did not make obvious)

- **Waiting on a copy operation was refused for the restricted identity** ("permission for project default") → the operation is created in another project. → Fallback: poll the project's running operations for the names, then check that the target exists (`copy_volume`, `project_operation_running`).
- **Copied volumes lost their size and `user.*` labels** → set them explicitly on the copy.
- **A `refresh` copy deletes the target volume's own snapshots** and is accepted while the instance runs. Plan restores around that.
- **Moving an instance was refused** → the project needs `restricted.cluster.target=allow` (now in `scripts/test-cluster/up.sh`).
- **Attached custom volumes do not follow a live-migrating VM** unless marked dependent, which ties their lifetime to the instance. Design moves with that in mind (`docs/design/mobility-and-ha.md`).
- **The mock copy ignored `fail_create`; the mock move did not carry `used_by`** → when the real adapter gains a behaviour, give the mock the same behaviour, failures included, in the same change.
- **A real-move test passed too early** → it waited for the job, not for the result. → Wait until the instance's location has changed.

## Lab, shell and tooling

- **`npm run lint` failed with "multiple candidate TSConfigRootDirs"** (2026-10-21) → agent worktrees live under `.claude/worktrees/` inside the repository, and ESLint scanned their copies. → `.claude/**` is in the ESLint ignores; any new repo-wide tool must ignore it too.
- **"`cargo run` gives development mode" was wrong inside the repository** (2026-10-21) → `.cargo/config.toml` sets `DATABASE_URL` for every cargo command. → Development mode is `DATABASE_URL= cargo run -p apelora-control-plane`; check tool-level config before documenting a default.

- **A worktree with a new migration must not use the shared development database** (2026-10-21) → once a worktree applies a migration the main checkout does not have, the main checkout's control plane refuses to start (sqlx: an applied migration is missing from its sources). → For real-stack runs in a worktree, create a separate database on the dev server (`CREATE DATABASE apelora_<topic>`) and drop it afterwards; `#[sqlx::test]` already uses fresh databases.
- **Dev servers on 5173/5174 belong to whichever checkout started them** (2026-10-21) → Playwright reuses them, so a worktree's run would test the main checkout's code. → In a worktree, run Playwright with a temporary config on other ports (e.g. `--port 5274`) and remove it afterwards.
- **Lab scripts broke on macOS** → `/usr/bin/env bash` is bash 3.2, which has different quoting and breaks on empty arrays. → Lab scripts use `/opt/homebrew/bin/bash`.
- **A plain `sleep` was blocked in the agent shell** → wait with `until <condition>; do sleep 1; done`.
- **The Incus CLI attach failed** → it needs a device name.
- **A design-review tool refused SVGs with text** → review illustrations by rendering them and looking at screenshots instead.

## Git and collaboration

- **Pre-staged deletions went into the wrong commit; Cargo.lock was left out** → check `git status` and `git diff --cached --stat` before each commit. Commit Cargo.lock with the change that alters it.
- **Rebase blocked by an unstaged file** → commit or stash everything before `git fetch && git rebase`.
- **The owner pushes from other sessions** → before every push: fetch, rebase, `npm install`, check OpenAPI name clashes, run the full checks.
- **Commits are authored by the owner only**, with no AI attribution (AGENTS.md rule 10). This overrides any tool default.

## Renames and compatibility

- **"an module" left behind after renaming "appliance"** → the article sat before a line break. → Do renames with a regex that crosses line breaks, then grep `.env.example`, `CITATION.cff`, scripts and docs. Keep serde aliases and a migration for stored data.
- **A private struct still had the old field name** → serde aliases go on every deserialised type, including private worker types.

## Secrets and evidence

- **Tokens must never reach the transcript, logs or screenshots** → serve test tokens to the browser from a git-ignored file, revoke them when done, and delete any screenshot that shows one.
- **The audit log is append-only** → when cleaning up test tokens, delete the token rows in their own statement. Never touch audit rows.
- **Label every result:** MOCK VERIFIED, CONTRACT VERIFIED, REAL INCUS TESTED, or REAL-BROWSER TESTED against a named local service. Never round up.

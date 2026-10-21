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
- **A strict content security policy reported `eval` in both applications although nothing called it** (2026-10-21) → zod 4 probes for `Function("")` inside a `try`; the browser reports the attempt even though it is caught. → `z.config({ jitless: true })` in `packages/assembly-model/src/schema.ts`; prove a policy by loading the production build under it (`tests/e2e/web-serving.spec.ts`), never by reading the code. The terminal emulator writes `<style>` elements at run time, so both sites' policies allow inline style elements (the Console opens terminals too since 2026-10-21).
- **A Playwright test counted three loads where it expected two** (2026-10-21) → the dev server runs React StrictMode, which mounts views twice. → Count requests from a baseline taken just before the action (`image mirrors show the mode…` in `tests/e2e/fabric-console.spec.ts`).
- **A shared terminal component spent its single-use ticket on a socket React closed at once** (2026-10-21) → moving the Studio's terminal into `packages/terminal` made it a component that mounts with the ticket, and React StrictMode in development runs a fresh mount's effect twice; the stub saw two sockets; against the broker the first would spend the ticket and the second would get 401. In the Studio's old code the effect ran on a prop change of an already mounted component, so it never double-ran. → Effects that spend something single-use start it on the next task and cancel it in cleanup (`TerminalScreen`); the Console spec counts exactly one socket per session.
- **Tests broke when a fixture list grew** (permission catalogue counts) → expected when you add to a shared fixture: update the counts in the same change.
- **"One primary button" tests failed after adding dialogs** (2026-10-21) → `FormDialog` renders its children while closed, so a closed dialog's primary button counts. → Render a dialog's form only while it is open (`{open && <form>…}`), which also makes `autoFocus` work on opening.
- **A new field in an API response broke stubbed pages** (2026-10-21) → the stubs in `tests/e2e/identity-fixtures.ts` (and `screenshots.spec.ts`) return the old shape, so the page read `undefined`. → When a response gains a field the UI reads, add it to every stub of that route in the same change.
- **A new header chip pushed the platform badge under it at 390 px** (2026-10-21) → the Studio header's left group was `shrink-0 whitespace-nowrap`, so it could only grow sideways. → Header groups wrap (`flex-wrap`); a Playwright test opens the chip's panel at 390 px (`studio-capacity.spec.ts`).
- **A ticking "checked 12 s ago" inside `aria-live` would be read out every few seconds** (2026-10-21) → live regions announce every text change. → Announce state changes (errors, stale data), never clocks; see the `fabric-console` skill.

## Control plane and identity

- **The control plane refused a provider's token and logged nothing** (2026-10-21) → refusals returned 401 without a log line. → Each refused credential is logged at warn as `credential refused` with method, path, credential kind, issuer and reason, never the token. It is not audited, since one row per bad request would let anyone fill the audit log.
- **A real provider issues 24-hour tokens; Apelora accepts at most one hour** (2026-10-21) → provider defaults differ from our limits. → Document the limit next to the setting, configure the dev provider for one hour, and make the refusal reason visible.
- **A provider that is down at startup stayed refused until a restart** (2026-10-21) → discovery ran only at startup and on save. → Retry unreachable providers every minute (`auth_config::retry_unreachable`).
- **An LDAP bind with an empty password succeeds** (2026-10-21) → it is an anonymous ("unauthenticated") bind, not a failed one. → Refuse an empty password before connecting (`ldap::sign_in`; guarded by `an_empty_password_is_refused_before_anything_is_sent`).
- **`ldaps://LDAP.Example` kept its capitals after URL parsing** (2026-10-21) → the `url` crate lowercases hosts only for special schemes (http, https, …), not `ldap`/`ldaps`. → Lowercase the host ourselves (`directories::validate`): the directory URL is its people's issuer, so its spelling must be stable.
- **The development directory's `memberOf` was empty** (2026-10-21) → the `memberof` overlay does not run for offline `slapadd`. → The seed lists `memberOf` explicitly (`infra/dev/directory/seed.ldif`).
- **Any change to sign-in settings can lock everyone out** → the API enforces the guards, not just the UI (`lockout_guard`, `set_api_tokens`, `guard_leaving_local_accounts`). Every new way in (local accounts, directories) must be counted by all of them.
- **`extract(epoch …)` came back as NUMERIC and failed to decode as `f64`** (2026-10-21) → PostgreSQL 14+ returns numeric. → Cast to `::float8` in queries that sqlx reads into floats.
- **A real control plane on the shared development database would have broken other checkouts** (2026-10-21) → sqlx refuses to start when the database has a migration the binary does not know, and parallel sessions share `apelora` on port 15432. → For real end-to-end runs of a branch with a new migration, create a separate database on the same server (`docker exec apelora-dev-db psql -U apelora -c "CREATE DATABASE …"`), point `DATABASE_URL` at it, and drop it afterwards.
- **A new `HealthState` schema silently replaced the instance `HealthState` in the OpenAPI document** (2026-10-21) → utoipa names schemas by the Rust type name only, so two types with one name collide without an error. → After `npm run api:generate`, check `git diff packages/api-client/src/generated.ts` for removed lines; give new schemas a distinct prefix (`Health*`, `Capacity*`).
- **Two people disabling the last two super admins at once both succeeded** (2026-10-21) → the "last one" guard counted first and changed afterwards, outside a transaction; it also counted assignment rows, not enabled people. → Guards that keep "at least one" run the change and the count in one transaction under an advisory lock (`identity::change_keeping_a_super_admin`; `containment_tests.rs` races two requests).
- **A token or role could reach further than the person who made it** (2026-10-21) → the escalation guard covered assignments only; tokens, revocation and legacy tokens (which skipped the route gate) had their own, weaker checks. → Every path that mints a credential or removes access applies the same containment (`beyond_caller`, `check_revocation`), and every caller passes the route gate.
- **A persistence function generic over `impl sqlx::Acquire` broke `tokio::spawn` of the supervisor with "implementation of `Send` is not general enough"** (2026-10-21) → the borrowed-lifetime bound cannot be proven for every lifetime inside a spawned future. → Take `&PgPool`, or `&mut PgConnection` for work inside a transaction (`enqueue_job_in(&mut tx, …)`); `impl PgExecutor` for single statements is fine.

- **`npm run api:generate` failed on the first route that answers text (`GET /metrics`)** (2026-10-21) → the client generator accepted only JSON responses. → The generator maps a `text/plain` success to `string` and the client returns the text (`packages/api-client/src/client.test.ts` guards it). A new non-JSON route needs the same treatment.

## Incus (REAL INCUS TESTED behaviour that the documentation did not make obvious)

- **Waiting on a copy operation was refused for the restricted identity** ("permission for project default") → the operation is created in another project. → Fallback: poll the project's running operations for the names, then check that the target exists (`copy_volume`, `project_operation_running`).
- **Copied volumes lost their size and `user.*` labels** → set them explicitly on the copy.
- **A `refresh` copy deletes the target volume's own snapshots** and is accepted while the instance runs. Plan restores around that.
- **Moving an instance was refused** → the project needs `restricted.cluster.target=allow` (now in `scripts/test-cluster/up.sh`).
- **Attached custom volumes do not follow a live-migrating VM** unless marked dependent, which ties their lifetime to the instance. Design moves with that in mind (`docs/design/mobility-and-ha.md`).
- **The mock copy ignored `fail_create`; the mock move did not carry `used_by`** → when the real adapter gains a behaviour, give the mock the same behaviour, failures included, in the same change.
- **A real-move test passed too early** → it waited for the job, not for the result. → Wait until the instance's location has changed.
- **Two "memory used" figures disagree by gigabytes** (2026-10-21, REAL INCUS TESTED) → `/1.0/cluster/members/<m>/state` gives total and free RAM (caches count as used), `/1.0/resources?target=<m>` gives used without caches. → Show the `/1.0/resources` figure (`HealthServerResources.memory`).
- **A shared pool (clustered LVM, Ceph) appears in every member's state with the same space** (2026-10-21) → it is one pool. → Count it once (`capacity::pool_is_shared`); summing per member multiplied the test cluster's free space by four.
- **The restricted deploy identity sees only its own projects' instances** → per-server instance lists from it are partial. Say so on screen (Health → Instances) instead of implying the server is empty.

## Lab, shell and tooling

- **`npm run lint` failed with "multiple candidate TSConfigRootDirs"** (2026-10-21) → agent worktrees live under `.claude/worktrees/` inside the repository, and ESLint scanned their copies. → `.claude/**` is in the ESLint ignores; any new repo-wide tool must ignore it too.
- **"`cargo run` gives development mode" was wrong inside the repository** (2026-10-21) → `.cargo/config.toml` sets `DATABASE_URL` for every cargo command. → Development mode is `DATABASE_URL= cargo run -p apelora-control-plane`; check tool-level config before documenting a default.

- **A worktree with a new migration must not use the shared development database** (2026-10-21) → once a worktree applies a migration the main checkout does not have, the main checkout's control plane refuses to start (sqlx: an applied migration is missing from its sources). → For real-stack runs in a worktree, create a separate database on the dev server (`CREATE DATABASE apelora_<topic>`) and drop it afterwards; `#[sqlx::test]` already uses fresh databases.
- **Dev servers on 5173/5174 belong to whichever checkout started them** (2026-10-21) → Playwright reuses them, so a worktree's run would test the main checkout's code. → In a worktree, run Playwright with a temporary config on other ports (e.g. `--port 5274`) and remove it afterwards.
- **Lab scripts broke on macOS** → `/usr/bin/env bash` is bash 3.2, which has different quoting and breaks on empty arrays. → Lab scripts use `/opt/homebrew/bin/bash`.
- **The ops self-test failed on macOS while the scripts passed by hand** (2026-10-21) → it ran them through their `#!/usr/bin/env bash` shebang, which is bash 3.2. → Operations scripts (`scripts/ops/`) must also run on Linux servers, so they keep `/usr/bin/env bash`, refuse bash < 4 with a message, and tests call them as `"$BASH" script`.
- **`verify.sh` printed VERIFIED and exited 1** (2026-10-21) → its EXIT trap ended with `[[ -n $TMP ]] && rm …`, which returns 1 when there is nothing to remove, and a trap's status becomes the script's. → End traps and functions with `if …; then …; fi`, and check the exit status in script tests, not just the output.
- **nftables rules that looked right were refused** (2026-10-21) → `fwd` is an nft keyword (not a chain name) and `comment` must end a rule. → Lab scripts that generate rules validate them with `nft --check` in `plan` (`scripts/test-cluster/egress.sh`), so the dry run fails instead of `block`.
- **A plain `sleep` was blocked in the agent shell** → wait with `until <condition>; do sleep 1; done`.
- **The Incus CLI attach failed** → it needs a device name.
- **A design-review tool refused SVGs with text** → review illustrations by rendering them and looking at screenshots instead.
- **Another agent overwrote a helper script in the shared scratchpad mid-task** (2026-10-21) → parallel sessions share the scratchpad directory. → Keep your helpers in your own subfolder of the scratchpad, named after your worktree.
- **A spacing measurement reported a 92 px gap that did not exist** (2026-10-21) → the snapshot had been trimmed to fit the tool call and lost a sibling. → Trim whole subtrees (e.g. keep `<main>`), never single siblings, and re-check any finding against the full capture.

## Git and collaboration

- **Pre-staged deletions went into the wrong commit; Cargo.lock was left out** → check `git status` and `git diff --cached --stat` before each commit. Commit Cargo.lock with the change that alters it.
- **Rebase blocked by an unstaged file** → commit or stash everything before `git fetch && git rebase`.
- **The owner pushes from other sessions** → before every push: fetch, rebase, `npm install`, check OpenAPI name clashes, run the full checks.
- **Commits are authored by the owner only**, with no AI attribution (AGENTS.md rule 10). This overrides any tool default.
- **Merging eight parallel branches: each passed alone, together 30 tests failed** (2026-10-21) → each branch's stubs and fixtures matched the API as that branch knew it. A new response field crashed pages whose stubs lacked it (`s.localAccounts.enabled`, `s.incus.serverVersion`), and a permission added to a role made another branch's "holds everything a viewer holds" fixtures wrong. A conflict resolved "keep both" also dropped a `#[cfg(test)]` that sat just outside the conflict block, and a `fabric-console(-[a-z]+)?` test pattern silently sent `fabric-console-local-accounts` to the wrong project. → After each merge, `cargo check --all-targets` before the next one. Run the full checks once all are in, before pushing. Pages read optional chains on fields a newer control plane adds (one version of skew, ADR 0011). Test patterns allow hyphens: `[a-z-]+`.

## Model and catalog

- **Every `mon` cable compiled to "module may reach the collector on 9100"** (2026-10-21) → the compiler treated every output as the consumer and used one default port per protocol, but metrics are *read* (the collector connects) and almost no software serves them on 9100. → Metrics outputs reverse the direction and use the class's own port (`ModuleClass.metrics`, `metricsEndpoint`); a class offers `mon` only when a collector can read metrics text on a port (`packages/module-catalog/src/metrics.test.ts`). Check a protocol's direction and real port before adding it to `DEFAULT_PORTS`.

## Renames and compatibility

- **"an module" left behind after renaming "appliance"** → the article sat before a line break. → Do renames with a regex that crosses line breaks, then grep `.env.example`, `CITATION.cff`, scripts and docs. Keep serde aliases and a migration for stored data.
- **A private struct still had the old field name** → serde aliases go on every deserialised type, including private worker types.

## Secrets and evidence

- **Tokens must never reach the transcript, logs or screenshots** → serve test tokens to the browser from a git-ignored file, revoke them when done, and delete any screenshot that shows one.
- **The audit log is append-only** → when cleaning up test tokens, delete the token rows in their own statement. Never touch audit rows.
- **Label every result:** MOCK VERIFIED, CONTRACT VERIFIED, REAL INCUS TESTED, or REAL-BROWSER TESTED against a named local service. Never round up.

# Mutation testing

Line coverage says a test *ran* a line. Mutation testing says a test would *fail* if the line were wrong. A tool changes the code in a small way (turns `>` into `>=`, replaces a function body with a constant, deletes a `!`) and runs the tests; a change that no test notices is a **missed mutant**, a rule nothing protects. For code that decides who may do what, that is the number that matters.

## What is covered and how to run it

| Area | Tool | Scope | Command | Gate |
|---|---|---|---|---|
| Authorisation | `cargo-mutants` | `crates/auth` (evaluator, roles, scopes, approvals, assignment guard, backend mapping) | `cargo mutants -p apelora-auth --no-shuffle -j 4` | **No missed mutant** (CI: `mutation.yml`, on pull requests that touch it, weekly) |
| Interactive access and lifecycle rules | `cargo-mutants` | `crates/domain/src/access.rs`, `lifecycle.rs` | `cargo mutants -p apelora-domain --no-shuffle -j 4 -f crates/domain/src/access.rs -f crates/domain/src/lifecycle.rs` | No missed mutant (same workflow) |
| Control plane handlers | `cargo-mutants` | `api/sessions.rs`, `api/authz.rs` (needs PostgreSQL: `DATABASE_URL`) | `cargo mutants -p apelora-control-plane --no-shuffle -j 2 --timeout 150 -f services/control-plane/src/api/sessions.rs -f services/control-plane/src/api/authz.rs` | Run by hand before changing these files; slow (about an hour) |
| Model rules (TypeScript) | Stryker | `packages/assembly-model` (`network.ts`, `validate.ts`, `runtime.ts`, `cables.ts`), `packages/api-client/src/index.ts` | `npm run mutation` | Score reported weekly, not a gate |

Scope is in `.cargo/mutants.toml` and `stryker.config.mjs`. The tools are development tools: neither is a dependency of anything that ships (`cargo-mutants` is MIT, Stryker is Apache-2.0).

## Reading a result

- **caught**: a test failed. **timeout**: counts as caught (a mutant that loops forever is noticed). **unviable**: did not compile; ignore.
- **MISSED**: look at the line and decide:
  1. *A real gap* (almost all of them): write the smallest test that fails for that mutant: both sides of a boundary, each operand of an `&&`/`||`, the value of a constant, the text of an error people read. For the authorisation crate these live in `crates/auth/src/hardening.rs` and next to the rule.
  2. *Redundant code*: delete it. (`derive_status` had a branch whose two arms returned the same value; mutation testing found it and it was removed.)
  3. *Equivalent mutant* (the program behaves identically): exclude that one function in `.cargo/mutants.toml` with a comment saying why. `Resource::platform()` is `Default::default()` by definition, so it is excluded.
- Never weaken a test, add `#[mutants::skip]` to a security rule, or exclude code to improve a number.

## Results (2026-10-20)

First runs, then the tests added in response.

| Scope | Mutants | Caught | Missed | Unviable | Note |
|---|---|---|---|---|---|
| `apelora-auth` | 253 | 230 | **0** | 23 | First run: 34 missed. Gaps found: service-identity guard in `filter`/`holds_anywhere`, MFA and environment conditions in `filter`, expiry boundaries, emergency-grant boundary (exactly four hours), custom-role limits (60-character name, 100 permissions, role keys as names), Incus project-name cleaning, approval lifetimes, blank error messages. |
| `apelora-domain` `access.rs`, `lifecycle.rs` | 66 | 58 + 2 timeouts | **0** | 6 | First run: 5 missed (shell command per session kind, self-connection and unknown endpoints in `layers`, one redundant branch). |
| Control plane `sessions.rs`, `authz.rs` | 249 | see below | | | |
| TypeScript model rules | see below | | | | |

### What the Rust numbers do not say

A caught mutant means *a test failed*, not that the rule is verified against reality. All of this is MOCK VERIFIED or CONTRACT VERIFIED; none of it ran against a real Incus or identity provider.

## Running Stryker

Stryker runs **in place**. The workspace packages are symlinked into `node_modules`, so a copied sandbox would test the original files, not the mutated copies. It rewrites source files while it runs and restores them afterwards, so:

- start from a clean `git status`;
- do not edit anything under `apps/` or `packages/` during a run, and do not run two at once (an edit made while it runs is overwritten when the originals are restored);
- if a run is killed, check `git status`; Stryker leaves its backup in `.stryker-tmp/backup-*` (git-ignored).

The Vitest runner plugin for Stryker does not yet work with Vitest 5 (mutants are never switched on, so everything "survives"), which is why `stryker.config.mjs` uses the `command` runner: each mutant runs `vitest run packages --bail=1` and the first failing test kills it. Revisit when the plugin supports Vitest 5.

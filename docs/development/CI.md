# Continuous integration

CI has to be fast enough that nobody waits on it and strict enough that main stays releasable. It does that with tiers: a quick gate on every change, the whole browser suite every night, a real Incus on the runner itself, and code scanning. No tier has secrets, and none touches infrastructure outside the runner. The real Incus tier installs Incus on the throwaway runner and generates its identities there; they never leave it. Tests that need several members, OVN, virtual machines or shared storage run only on the disposable lab (see [TEST-CLUSTER.md](TEST-CLUSTER.md)).

## Tiers

| Workflow | When | What | Typical time |
|---|---|---|---|
| `ci.yml` | every pull request and push to main, by hand | the areas the change touches: frontend (typecheck, lint, unit, build, generated files), Rust lint (fmt, clippy) and Rust test (with PostgreSQL and the database backup drill) as separate jobs, dependencies, secrets and workflow files (actionlint), example systemd units and the NGINX configuration (`scripts/ci/verify-install-examples.sh`), the example collector compose file (`scripts/ci/verify-observe-examples.sh`), the `@smoke` browser tests; `CI passed` sums them up | a few minutes; the smoke tests take about 35 s locally |
| `e2e-full.yml` | nightly on main, by hand, on a pull request labelled `full-e2e` | the whole Playwright suite, split by test over four runners | about a quarter of the single-runner time |
| `real-incus.yml` | pull requests that touch the Incus adapter, domain, persistence, worker, lab setup, or the workspace/toolchain files; nightly; by hand | Incus on the runner, from the channel in `upstream.lock.yaml` (`scripts/test-cluster/install-incus.sh`, then `up.sh` with `APELORA_TEST_LOCAL=1`), then the one-member REAL INCUS TESTED tests: container and OCI lifecycle, refused architecture, join tokens, volumes, in-place resize, logs, a cluster setting. The step fails unless those 9 tests actually ran. | about 3 minutes (9 tests in 49 s after a cold compile) |
| `codeql.yml` | pull requests, main, weekly | CodeQL for Rust, TypeScript and the workflows, shipping code only | |
| `dependency-review.yml` | pull requests | refuses new dependencies with high-severity advisories | |
| `dependabot-auto-merge.yml` | after CI finishes on a Dependabot pull request | merges passing patch and minor updates | |
| `mutation.yml`, `upstream-check.yml` | as described in each file | | |
| `release.yml` | a version tag (`v*`) | checks the tag, that it is on `main` and that CI passed; builds each service and screen for both architectures with checksums and a provenance attestation; publishes pre-releases, drafts final versions | [releasing](RELEASING.md) |

Which areas run is decided by `scripts/ci/changed-areas.sh`: a pull request compares with its base, a push with the commit before it. A change to `.github/workflows/`, `.github/actions/` or `scripts/ci/` runs everything, and so does a manual run or an unknown base. A documentation-only change runs only the dependency and secret job, which also checks that counts stated in the documentation (built-in alert rules, module classes, deployable classes, API operations) match the code.

## The smoke set

`@smoke` marks one or two tests per area that open the main page and do its main job: each Console area, the Studio canvas and its module card (including resizing), deploy, sign-in, sessions, volumes and the web serving policy. Tag a test with the details argument, not in its title:

```ts
test("lists directories with their status in words", { tag: "@smoke" }, async ({ page }) => {
```

Add a smoke test when a new area or page appears. Keep the set small (about 30 tests). Everything else runs in the nightly full suite, and before a push locally (`npm run test:e2e`). To run the full suite on a pull request, add the `full-e2e` label. To run only the smoke set locally: `npx playwright test --grep @smoke`.

## Speed rules

- Pin every third-party action to a full commit SHA with the version in a comment (`uses: owner/action@<sha> # v1.2.3`). Dependabot moves the SHA and the comment together.
- Pin the runner image (`ubuntu-24.04`), not `ubuntu-latest`, so an image change is a deliberate edit.
- Cargo: `Swatinem/rust-cache` keyed by job; only main writes the cache (`save-if`), so pull requests do not fill the 10 GB budget. `CARGO_INCREMENTAL=0` and no debug info in CI builds.
- Playwright: Chromium is cached per Playwright version (`.github/actions/playwright-browsers`); the system libraries are installed every run.
- Every job has a `timeout-minutes`. Pull request runs cancel the previous run of the same pull request; each commit on main keeps its own result.
- `npm ci --no-audit --no-fund`; the audit runs once, in the dependency job, against shipped packages only.

## Dependabot

`.github/dependabot.yml` checks GitHub Actions, npm and Cargo weekly, with a three-day cooldown on version updates (a fresh release settles before it is proposed; security updates are not delayed). Minor and patch updates are grouped per ecosystem. The hash and MAC crates (`sha1`, `sha2`, `hmac`, `digest`) form their own group because they only build together. `tokio-tungstenite` stays at the version axum uses and is moved by hand with axum.

`dependabot-auto-merge.yml` runs when CI completes. It merges a Dependabot pull request when:

- CI succeeded on the pull request's exact head commit,
- the pull request comes from Dependabot in this repository, and
- every dependency in it moves by a patch or minor version, read from Dependabot's commit metadata (`update-type: version-update:semver-…`).

It never checks out or runs the pull request's code. A major version (and anything without metadata) stays open for review. After merging it starts CI on main, because a merge made with the workflow's token starts no workflows by itself. For 0.x crates Dependabot calls `0.29 → 0.30` a minor update; CI is the gate for those, which is why it has to stay meaningful.

Repository settings (enabled through the API): Dependabot alerts and Dependabot security updates.

**Branch protection is not enabled**, because maintainers push straight to main. If that changes, require the single `CI passed` check: it passes when every job passed or was skipped because its area did not change.

## Code scanning

CodeQL runs from `codeql.yml` with `.github/codeql/codeql-config.yml` (advanced setup; the repository's default setup is off, because both cannot upload results). Test code is left out by file name: `tests.rs`, `*_tests.rs`, `tests/`, `*.test.ts(x)`. Tests use fixed passwords and print values in assertion messages on purpose, which the secret-handling queries report. An inline `mod tests { … }` is scanned with its module; when one of its fixtures is reported, move it to `<module>/tests.rs` with `#[cfg(test)] mod tests;`.

A finding in shipping code is fixed in the code. The exception is a finding that describes intended behaviour; dismiss it in the Security tab with the reason, for example the `token create` command that prints the new token once, to the person who created it.

## Changing CI

Read `.agents/skills/ci-pipeline/SKILL.md` first. Measure before and after: `gh run view <id> --json jobs` gives each job's start and end.

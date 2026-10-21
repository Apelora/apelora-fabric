---
name: ci-pipeline
description: Change Apelora's GitHub Actions CI — the fast gate (ci.yml), the nightly full browser suite (e2e-full.yml), CodeQL, Dependabot and its auto-merge — while keeping it fast, pinned and secret-free. Use when editing anything under .github/, scripts/ci/, the @smoke tags, or when CI is slow, flaky or failing.
---

# CI pipeline

**Read first:** `docs/development/CI.md` (tiers, smoke set, Dependabot, code scanning).

## Rules

| Area | Rule |
|---|---|
| Tiers | Every change: `ci.yml`, only the areas it touches (`scripts/ci/changed-areas.sh`). The whole Playwright suite runs in `e2e-full.yml` (nightly, by hand, `full-e2e` label), never on every push |
| Smoke | `{ tag: "@smoke" }` on one or two tests per area (main page, main job). New area → one smoke test. Keep it near 30 tests |
| Actions | Full commit SHA + `# vX.Y.Z` comment; resolve with `gh api repos/<owner>/<action>/commits/<tag> --jq .sha`. Runner `ubuntu-24.04` (or `-arm`), never `-latest` |
| Jobs | `timeout-minutes` on every job; least `permissions` (workflow default `contents: read`, widen per job); add new jobs to `ci-ok.needs` |
| Caches | `Swatinem/rust-cache` with `save-if: main`; Playwright browsers through `.github/actions/playwright-browsers`; `setup-node` `cache: npm` |
| Secrets | None in CI; no Incus credentials; no real infrastructure; no `pull_request_target`. A `workflow_run` job with write permissions never checks out pull request code |
| Dependabot | Groups: per ecosystem minor/patch; crates that only build together in their own group (`rustcrypto`); a pin that follows another crate goes under `ignore` with the reason. Auto-merge only patch/minor after CI passed on the exact head commit |
| CodeQL | Advanced setup only (`codeql.yml`); test code is excluded by file name (`codeql-config.yml`). Fix shipping-code findings in code; dismiss only intended behaviour, with the reason |

## Procedure

1. Note the current job times: `gh run list --workflow ci.yml --limit 5`, then `gh run view <id> --json jobs --jq '.jobs[]|"\(.name) \(.startedAt) \(.completedAt)"'`.
2. Make the change. A new area or path: extend `changed-areas.sh` and check it with `printf 'path\n' | bash scripts/ci/changed-areas.sh -`.
3. Locally: `npx playwright test --grep @smoke`; for sharding, `npx playwright test --list --fully-parallel --shard=1/4`.
4. Push, then watch: `gh run watch` or `gh run view --log-failed`. Compare the job times with step 1 and record the result in `docs/development/CI.md` when it changes the table.

## Acceptance

- Every `uses:` is SHA-pinned; no `ubuntu-latest`; every job has a timeout.
- A documentation-only change runs only the dependency job; a workflow change runs everything.
- `CI passed` is green, and the nightly full suite passes on main.

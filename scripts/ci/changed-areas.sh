#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Decides which CI areas a change touches, so a pull request runs only the jobs it needs
# (Phase 10). Prints `frontend=true|false` and `rust=true|false` (append to $GITHUB_OUTPUT).
# The base is a pull request's base, or for a push to main the commit before it. Everything runs
# for manual runs, when the base is unknown (a new branch, a force push), and when CI itself
# changes; a skipped job still satisfies the `CI passed` check.
#   bash scripts/ci/changed-areas.sh                 # all areas (no base given)
#   bash scripts/ci/changed-areas.sh <base-sha>      # areas changed since <base-sha>
#   git diff --name-only … | bash scripts/ci/changed-areas.sh -   # paths on standard input
set -euo pipefail

all() { printf 'frontend=true\nrust=true\n'; exit 0; }

base=${1:-}
[ -n "$base" ] || all
if [ "$base" = - ]; then
  files=$(cat)
else
  git cat-file -e "$base^{commit}" 2>/dev/null || all
  files=$(git diff --name-only "$base"...HEAD)
fi

# CI itself (workflows, this script) or the shared toolchain files: run everything.
if grep -qE '^(\.github/(workflows|actions)/|scripts/ci/)' <<<"$files"; then all; fi

rust=false frontend=false
# The Rust side also checks that the committed OpenAPI document is current, and runs the
# database backup and restore drill test (scripts/ops) against its PostgreSQL service.
grep -qE '^(crates/|services/|scripts/ops/|infra/systemd/|Cargo\.(toml|lock)$|rust-toolchain\.toml$|deny\.toml$|\.cargo/|packages/api-client/openapi\.json$)' <<<"$files" && rust=true
# Apps, packages, end-to-end tests, the web serving policy (tests/e2e/web-serving.spec.ts), npm
# and TypeScript tooling, and generated references.
grep -qE '^(apps/|packages/|tests/|infra/serve/|package(-lock)?\.json$|tsconfig[^/]*\.json$|eslint\.config\.[a-z]+$|playwright\.config\.[a-z]+$|docs/architecture/CATALOG\.md$)' <<<"$files" && frontend=true
printf 'frontend=%s\nrust=%s\n' "$frontend" "$rust"

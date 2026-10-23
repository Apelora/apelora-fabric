#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Self-test for scripts/ops/upgrade.sh. Uses throwaway directories outside the repository.
# Does not stop a service, touch PostgreSQL, or write under the checkout.
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)
root=$(cd "$here/../.." && pwd -P)
upgrade=("$BASH" "$here/upgrade.sh")
work=$(mktemp -d "${TMPDIR:-/tmp}/apelora-upgrade-test.XXXXXX")
trap 'rm -rf "$work"' EXIT
bundle=$work/bundle
prefix=$work/prefix
backup=$work/apelora-db.dump
log=$work/log
mkdir -p "$bundle/bin" "$bundle/web/studio" "$bundle/web/console" "$prefix/bin" "$prefix/web/studio" "$prefix/web/console"
printf 'old-api\n' >"$prefix/bin/apelora-control-plane"
printf 'old-worker\n' >"$prefix/bin/apelora-deployment-worker"
printf 'old-studio\n' >"$prefix/web/studio/index.html"
printf 'old-console\n' >"$prefix/web/console/index.html"
printf 'new-api\n' >"$bundle/bin/apelora-control-plane"
printf 'new-worker\n' >"$bundle/bin/apelora-deployment-worker"
printf 'new-studio\n' >"$bundle/web/studio/index.html"
printf 'new-console\n' >"$bundle/web/console/index.html"
printf 'version: 9.9.9-test\n' >"$bundle/BUILD-INFO"
printf 'dump\n' >"$backup"
fails=0
pass() { echo "PASS  $1"; }
flunk() { echo "FAIL  $1"; fails=$((fails + 1)); }
expect() {
  local want=$1 what=$2 got=0
  shift 2
  "$@" >"$log" 2>&1 || got=$?
  if [[ $got == "$want" ]]; then pass "$what"; else flunk "$what (exit $got, wanted $want)"; tail -n 8 "$log" | sed 's/^/      /'; fi
}

expect 1 "refuses to apply without a database dump" \
  env APPLY=0 BUNDLE="$bundle" PREFIX="$prefix" BACKUP="" "${upgrade[@]}"
grep -q 'systemctl stop apelora-deployment-worker' "$log" &&
  grep -q 'systemctl stop apelora-control-plane' "$log" &&
  awk '/stop apelora-deployment-worker/{w=NR} /stop apelora-control-plane/{c=NR} END{exit !(w && c && w<c)}' "$log" &&
  pass "plan stops the worker before the control plane" ||
  flunk "plan stops the worker before the control plane"

expect 1 "refuses a bundle inside the repository" \
  env APPLY=0 BUNDLE="$root" PREFIX="$prefix" BACKUP="$backup" "${upgrade[@]}"

expect 1 "refuses a prefix inside the repository" \
  env APPLY=1 BUNDLE="$bundle" PREFIX="$root" BACKUP="$backup" "${upgrade[@]}"

expect 0 "installs the bundle and keeps the previous binaries" \
  env APPLY=1 BUNDLE="$bundle" PREFIX="$prefix" BACKUP="$backup" "${upgrade[@]}"
grep -q 'version: 9.9.9-test' "$prefix/upgrade-record" && pass "record names the installed version" || flunk "record names the installed version"
[[ $(cat "$prefix/bin/apelora-control-plane") == new-api ]] && pass "control plane binary replaced" || flunk "control plane binary replaced"
[[ $(cat "$prefix/previous/bin/apelora-deployment-worker") == old-worker ]] && pass "previous worker kept" || flunk "previous worker kept"

expect 0 "rollback restores the previous binaries" \
  env APPLY=1 ROLLBACK=1 PREFIX="$prefix" "${upgrade[@]}"
[[ $(cat "$prefix/bin/apelora-control-plane") == old-api ]] && pass "control plane binary restored" || flunk "control plane binary restored"
[[ $(cat "$prefix/web/console/index.html") == old-console ]] && pass "console files restored" || flunk "console files restored"
grep -q '^rolled-back:' "$prefix/upgrade-record" && pass "record says rolled back" || flunk "record says rolled back"

echo
if [[ $fails -eq 0 ]]; then echo "upgrade self-test: all checks passed"; exit 0; fi
echo "upgrade self-test: $fails failed" >&2
exit 1

#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Checks the enrolment preseed for a sized loop pool without joining a cluster or reading a token.
#
#   bash scripts/enrol/test-enrol-member.sh
set -euo pipefail
if ((BASH_VERSINFO[0] < 4)); then
  echo "error: bash 4 or newer is required (on macOS: /opt/homebrew/bin/bash $0)" >&2
  exit 2
fi
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)
enrol=("$BASH" "$here/enrol-member.sh")
fails=0
pass() { echo "PASS  $1"; }
flunk() { echo "FAIL  $1"; fails=$((fails + 1)); }

out=$(mktemp)
trap 'rm -f "$out"' EXIT

if "${enrol[@]}" --name m4 --address 10.0.0.14 --pool-size 20GiB --print-member-config >"$out" 2>"$out.err"; then
  if grep -q 'key: size' "$out" && grep -q 'value: "20GiB"' "$out" \
    && grep -q 'key: source' "$out" && grep -q 'value: ""' "$out" \
    && grep -q 'name: local' "$out"; then
    pass "a loop pool answers size and an empty source"
  else
    flunk "a loop pool answers size and an empty source"
    sed 's/^/      /' "$out"
  fi
else
  flunk "a loop pool answers size and an empty source (exit $?)"
  sed 's/^/      /' "$out.err"
fi

if "${enrol[@]}" --name m4 --address 10.0.0.14 --pool-size 12GiB --print-member-config >"$out"; then
  if grep -q 'value: "12GiB"' "$out" && ! grep -q 'cluster_token' "$out"; then
    pass "printing the pool config does not include a token"
  else
    flunk "printing the pool config does not include a token"
  fi
else
  flunk "printing the pool config does not include a token"
fi

got=0
"${enrol[@]}" --name m4 --address 10.0.0.14 --pool-size 20GiB --member-config /dev/null --print-member-config >"$out" 2>"$out.err" || got=$?
if [[ $got != 0 ]] && grep -q 'not both' "$out.err"; then
  pass "pool size and a member-config file are refused together"
else
  flunk "pool size and a member-config file are refused together (exit $got)"
fi

got=0
"${enrol[@]}" --name m4 --address 10.0.0.14 --pool-size '20GiB;rm' --print-member-config >"$out" 2>"$out.err" || got=$?
if [[ $got != 0 ]] && grep -q '20GiB' "$out.err"; then
  pass "a pool size that is not a size is refused"
else
  flunk "a pool size that is not a size is refused (exit $got)"
fi

if ((fails > 0)); then
  echo "$fails check(s) failed"
  exit 1
fi
echo "all checks passed"

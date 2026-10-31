#!/usr/bin/env bash
# Reports whether upstream has a newer release in the tracked series (`series` in the lock file:
# "7" follows every 7.x release, newest first; "7.0" would follow one long-term series only).
# Never changes anything.
# Prints `update=<tag>` (or `update=`) and, in GitHub Actions, writes it to $GITHUB_OUTPUT.
source "$(dirname "$0")/lib.sh"
url="$(lock_get incus url)"; series="$(lock_get incus series)"; pinned="$(lock_get incus pinned_tag)"
latest="$(remote_tags "$url" "v${series}.*" | tail -1)"
newest_feature="$(remote_tags "$url" 'v*' | tail -1)"
update=""
if [ -n "$latest" ] && [ "$latest" != "$pinned" ] && [ "$(printf '%s\n%s\n' "$pinned" "$latest" | sort -V | tail -1)" = "$latest" ]; then
  update="$latest"
fi
echo "pinned=$pinned"
echo "update=$update"
echo "newest_feature=$newest_feature"
if [ -n "${GITHUB_OUTPUT:-}" ]; then
  { echo "pinned=$pinned"; echo "update=$update"; echo "newest_feature=$newest_feature"; } >> "$GITHUB_OUTPUT"
fi

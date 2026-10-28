#!/usr/bin/env bash
# Shows the pinned Incus release, what upstream has published, and the local reference checkout.
source "$(dirname "$0")/lib.sh"
url="$(lock_get incus url)"; series="$(lock_get incus series)"; pinned="$(lock_get incus pinned_tag)"
latest_lts="$(remote_tags "$url" "v${series}.*" | tail -1)"
latest_any="$(remote_tags "$url" 'v*' | tail -1)"
echo "Incus pinned:           $pinned ($(lock_get incus pinned_commit | cut -c1-12)), series $series"
echo "Upstream latest $series:   ${latest_lts:-unknown}"
echo "Upstream latest overall: ${latest_any:-unknown}"
echo "Fork / patch branch:    $(lock_get incus fork) @ $(lock_get incus fork_patch_branch)"
echo "Tested with release:    $(lock_get incus tested_release)"
echo "Tested with:            $(lock_get incus tested_with)"
if [ -d "$ROOT/upstream/incus/.git" ]; then
  echo "Local checkout:         upstream/incus @ $(git -C "$ROOT/upstream/incus" rev-parse --short=12 HEAD)"
else
  echo "Local checkout:         none (run scripts/upstream/bootstrap.sh)"
fi

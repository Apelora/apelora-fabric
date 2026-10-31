#!/usr/bin/env bash
# Verifies the lock file against upstream: the pinned tag must resolve to the pinned commit.
source "$(dirname "$0")/lib.sh"
url="$(lock_get incus url)"; tag="$(lock_get incus pinned_tag)"; want="$(lock_get incus pinned_commit)"
got="$(remote_commit "$url" "$tag")"
if [ "$got" != "$want" ]; then
  echo "MISMATCH: $tag resolves to ${got:-nothing} upstream, lock says $want" >&2
  exit 1
fi
# The lab VM definition installs Incus from one channel; it must be the pinned one.
channel="$(lock_get incus channel)"
if ! grep -q "pkgs.zabbly.com/incus/${channel}\$" "$ROOT/scripts/test-cluster/incus-vm.yaml"; then
  echo "MISMATCH: scripts/test-cluster/incus-vm.yaml does not install Incus from the $channel channel the lock names" >&2
  exit 1
fi
fork="$(lock_get incus fork)"; branch="$(lock_get incus fork_patch_branch)"
if git ls-remote --exit-code --heads "$fork" "$branch" >/dev/null 2>&1; then
  echo "OK: $tag -> ${want:0:12}; fork branch $branch present"
else
  echo "OK: $tag -> ${want:0:12}; WARNING fork branch $branch not found at $fork" >&2
fi

#!/usr/bin/env bash
# Optional: a read-only reference checkout of Incus at the pinned release, in upstream/incus
# (git-ignored). For reading and tests only; it is never built into Apelora.
source "$(dirname "$0")/lib.sh"
url="$(lock_get incus url)"; tag="$(lock_get incus pinned_tag)"; want="$(lock_get incus pinned_commit)"
dest="$ROOT/upstream/incus"
if [ ! -d "$dest/.git" ]; then
  git -c advice.detachedHead=false clone --quiet --depth 1 --branch "$tag" "$url" "$dest"
else
  git -C "$dest" fetch --quiet --depth 1 origin "refs/tags/$tag:refs/tags/$tag"
  git -C "$dest" -c advice.detachedHead=false checkout --quiet "$tag"
fi
git -C "$dest" remote get-url apelora >/dev/null 2>&1 || git -C "$dest" remote add apelora "$(lock_get incus fork)"
got="$(git -C "$dest" rev-parse HEAD)"
[ "$got" = "$want" ] || { echo "Checkout is at $got, expected $want" >&2; exit 1; }
echo "upstream/incus at $tag (${got:0:12}); remotes: origin=upstream, apelora=fork"

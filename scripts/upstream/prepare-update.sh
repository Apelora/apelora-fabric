#!/usr/bin/env bash
# prepare-update.sh <tag>: moves the pin to <tag> after resolving its commit upstream.
# Used by the weekly workflow and by humans; the result goes into a pull request for review.
source "$(dirname "$0")/lib.sh"
tag="${1:?usage: prepare-update.sh <tag>}"
url="$(lock_get incus url)"; old="$(lock_get incus pinned_tag)"
commit="$(remote_commit "$url" "$tag")"
[ -n "$commit" ] || { echo "Tag $tag not found upstream" >&2; exit 1; }
lock_set incus pinned_tag "$tag"
lock_set incus pinned_commit "$commit"
lock_set incus latest_feature_release "$(remote_tags "$url" 'v*' | tail -1)"
lock_set incus tested_with "NOT TESTED with $tag yet (update pending review)"
# The release last tested stays as it was until the new pin has been tested.
lock_set incus verified "$(date -u +%Y-%m-%d)"
echo "Pinned $old -> $tag (${commit:0:12})"
echo "Changes: ${url}/compare/${old}...${tag}"

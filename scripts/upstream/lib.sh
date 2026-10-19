#!/usr/bin/env bash
# Shared helpers for the upstream scripts. Reads upstream.lock.yaml without extra tools.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
LOCK="$ROOT/upstream.lock.yaml"

# lock_get <upstream> <key>: prints the value of `key` inside the `upstreams.<upstream>` block.
lock_get() {
  awk -v up="  $1:" -v key="$2" '
    $0 == up { inside = 1; next }
    inside && /^  [^ ]/ { inside = 0 }
    inside && $1 == key":" { sub(/^[^:]*:[ ]*/, ""); gsub(/^"|"$/, ""); print; exit }
  ' "$LOCK"
}

# lock_set <upstream> <key> <value>: replaces the value of `key` in that block (key must exist).
lock_set() {
  local tmp; tmp="$(mktemp)"
  awk -v up="  $1:" -v key="$2" -v val="$3" '
    $0 == up { inside = 1; print; next }
    inside && /^  [^ ]/ { inside = 0 }
    inside && $1 == key":" { sub(/:.*/, ": " val); done = 1 }
    { print }
    END { if (!done) exit 3 }
  ' "$LOCK" > "$tmp" && mv "$tmp" "$LOCK"
}

# remote_commit <url> <tag>: the commit a tag points to (dereferences annotated tags).
remote_commit() {
  git ls-remote "$1" "refs/tags/$2" "refs/tags/$2^{}" | awk '{ sha = $1 } /\^\{\}$/ { print $1; found = 1; exit } END { if (!found && sha) print sha }'
}

# remote_tags <url> <glob>: release tags matching a glob, oldest to newest.
remote_tags() {
  git ls-remote --tags --refs "$1" "$2" | sed 's#.*refs/tags/##' | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | sort -V
}

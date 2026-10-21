#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Fails when the repository's tracked files contain something that looks like a secret:
# private keys, Apelora API tokens, Incus join tokens, or client certificate keys. Run in CI
# and before pushing. Matches print the file and line, never the secret itself.
# Usage: bash scripts/security/scan-secrets.sh
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

patterns=(
  'BEGIN (RSA |EC |DSA |OPENSSH |ENCRYPTED )?PRIVATE KEY'   # any private key
  'ap[lstuw]_[0-9a-f]{64}'                                   # Apelora tokens: API (apl_), service (aps_), session (apu_), personal (apt_), setup link (apw_)
  'eyJzZXJ2ZXJfbmFtZSI6[A-Za-z0-9+/=]{60,}'                  # Incus join token (base64 JSON with a secret)
)
# Test fixtures and docs may show the *shape* of a join token with a short, fake value; the
# length above keeps those out. Nothing is excluded by path.
found=0
for p in "${patterns[@]}"; do
  if hits=$(git grep -nIE "$p" -- . ':!scripts/security/scan-secrets.sh' 2>/dev/null); then
    found=1
    # Show where, not what.
    while IFS= read -r where; do
      printf 'possible secret (%s): %s\n' "$p" "$where" >&2
    done < <(echo "$hits" | cut -d: -f1,2)
  fi
done
if [ "$found" -ne 0 ]; then
  echo "Secrets must never be committed (AGENTS.md rule 7). Remove them, rotate them, and rewrite history if they were pushed." >&2
  exit 1
fi
echo "no secrets found in tracked files"

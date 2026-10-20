#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Creates the pool volume backups go to (APELORA_BACKUP_POOL=backup) on every member of the
# disposable test cluster: a `dir` pool, separate from the instances' `local` pool. Safe to
# repeat. See docs/development/TEST-CLUSTER.md and docs/architecture/STORAGE.md.
set -euo pipefail
VM=${APELORA_TEST_VM:-apelora-incus}
POOL=${1:-backup}
[[ $POOL =~ ^[a-z][a-z0-9-]{0,62}$ ]] || { echo "invalid pool name" >&2; exit 1; }
incus() { limactl shell --workdir / "$VM" sudo incus "$@" </dev/null; }
if incus storage show "$POOL" >/dev/null 2>&1; then
  echo "pool $POOL exists"
  exit 0
fi
for m in $(incus cluster list -f csv -c n); do
  incus storage create "$POOL" dir --target "$m" >/dev/null
done
incus storage create "$POOL" dir
incus storage list -f csv

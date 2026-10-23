#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# PostgreSQL restore_command for a base backup of Apelora's own database. Copies one archived
# write-ahead log file back. A missing file exits 1 and prints nothing: that is how recovery
# learns it has reached the end of the archive. Runbook: docs/runbooks/apelora-database-backup.md.
#
#   restore_command = '/opt/apelora/ops/wal-restore.sh %f %p'
#   APELORA_WAL_DIR=/srv/apelora-wal
#
# %f is the file name recovery wants. %p is where it should be written.
set -euo pipefail
# shellcheck source=scripts/ops/db-common.sh
source "$(dirname "${BASH_SOURCE[0]}")/db-common.sh"

name=${1:-}
dest=${2:-}
[[ -n $name && -n $dest ]] || die "usage: APELORA_WAL_DIR=… wal-restore.sh %f %p"
[[ $name =~ $WAL_NAME_RE ]] || die "refusing to restore '$name' (not a write-ahead log name)"
dir=${APELORA_WAL_DIR:-}
dir=$(backup_dir_checked "$dir")
src="$dir/$name"
[[ -f $src ]] || exit 1
cp -- "$src" "$dest"

#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# PostgreSQL archive_command for Apelora's own database. Copies one completed write-ahead log
# file to storage outside the cluster, then fsyncs it. Runbook:
# docs/runbooks/apelora-database-backup.md.
#
#   archive_command = '/opt/apelora/ops/wal-archive.sh %p %f'
#   APELORA_WAL_DIR=/srv/apelora-wal
#
# %p is the path of the segment in the server's data directory. %f is its file name.
# Exit status: 0 the copy is in the archive (including a retry of an identical file);
# 1 the copy did not happen. A different file already stored under that name is a failure,
# because a write-ahead segment is immutable.
set -euo pipefail
# shellcheck source=scripts/ops/db-common.sh
source "$(dirname "${BASH_SOURCE[0]}")/db-common.sh"

src=${1:-}
name=${2:-}
[[ -n $src && -n $name ]] || die "usage: APELORA_WAL_DIR=… wal-archive.sh %p %f"
[[ $name =~ $WAL_NAME_RE ]] || die "refusing to archive '$name' (not a write-ahead log name)"
[[ -f $src ]] || die "the write-ahead log file does not exist: $src"
dir=${APELORA_WAL_DIR:-}
dir=$(backup_dir_checked "$dir")
[[ -w $dir ]] || die "the write-ahead log archive is not writable: $dir"

dest="$dir/$name"
if [[ -f $dest ]]; then
  [[ $(sha256_of "$src") == $(sha256_of "$dest") ]] || die "the archive already has a different file named $name"
  exit 0
fi

umask 077
tmp="$dir/.$name.partial.$$"
rm -f "$tmp"
cp -- "$src" "$tmp"
sync -- "$tmp"
mv -- "$tmp" "$dest"
sync -- "$dir"

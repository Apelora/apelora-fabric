#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Takes a physical base backup of Apelora's PostgreSQL with pg_basebackup --wal-method=none.
# pg_basebackup does not return until every write-ahead segment the backup needs has been
# archived. The script then checks that segment is in APELORA_WAL_DIR. The role
# needs replication (a superuser does). This does not replace the logical dump: a logical dump
# restores across major versions, and this backup is tied to one. Runbook:
# docs/runbooks/apelora-database-backup.md.
#
#   DATABASE_URL=postgres://… APELORA_BACKUP_DIR=/srv/apelora-backups APELORA_WAL_DIR=/srv/apelora-wal \
#     scripts/ops/db-basebackup.sh
#
# Writes base-<UTC time>/ under the backup directory (mode 0700) and base-<UTC time>.json beside
# the parent, recording the time and the write-ahead file that must be archived.
# Exit status: 0 the base backup is taken and that file is in the archive; 1 failed; 2 usage.
set -euo pipefail
# shellcheck source=scripts/ops/db-common.sh
source "$(dirname "${BASH_SOURCE[0]}")/db-common.sh"

usage() { awk 'NR > 2 && /^#/ { sub(/^# ?/, ""); print; next } NR > 2 { exit }' "${BASH_SOURCE[0]}"; }

dir=${APELORA_BACKUP_DIR:-}
wal_dir=${APELORA_WAL_DIR:-}
while (($#)); do
  case $1 in
  --dir) dir=${2:?--dir needs a value}; shift 2 ;;
  --wal-dir) wal_dir=${2:?--wal-dir needs a value}; shift 2 ;;
  -h | --help) usage; exit 0 ;;
  *) usage >&2; echo "error: unknown argument: $1" >&2; exit 2 ;;
  esac
done

pg_connect_setup "${DATABASE_URL:-}"
umask 077
dir=$(backup_dir_checked "$dir")
wal_dir=$(backup_dir_checked "$wal_dir")
[[ -w $dir && -w $wal_dir ]] || die "the backup directory and the write-ahead archive must be writable"

ts=$(date -u +%Y%m%dT%H%M%SZ)
slot="$dir/base-$ts"
[[ ! -e $slot ]] || die "$slot already exists"
mkdir -m 0700 "$slot"
note "base backup: $(pg_where) -> $slot (write-ahead logs in $wal_dir, not inside the backup)"

# pg_basebackup writes into the directory. A client container cannot see the host path, so the
# directory is mounted there and the tool runs as the server's usual user id when we know it.
run_basebackup() {
  if [[ -n ${APELORA_PG_CLIENT_IMAGE:-} ]]; then
    local -a envs=()
    local v
    for v in "${OPS_PG_VALUE_VARS[@]}"; do
      [[ -n ${!v:-} ]] && envs+=(-e "$v")
    done
    docker run --rm --network host --user "${APELORA_PG_UID:-999}:${APELORA_PG_GID:-999}" \
      --log-driver none -v "$slot":/backup "${envs[@]}" "$APELORA_PG_CLIENT_IMAGE" \
      pg_basebackup --no-password --wal-method=none --checkpoint=fast --pgdata=/backup
  else
    pg_basebackup --no-password --wal-method=none --checkpoint=fast --pgdata="$slot"
  fi
}
if ! run_basebackup; then
  rm -rf "$slot"
  die "pg_basebackup failed (see the message above); the partial directory was removed"
fi

# backup_label names the segment recovery starts from. pg_basebackup has already waited for it
# to be archived ("all required WAL segments have been archived").
segment=$(sed -n 's/.*file \([0-9A-F]\{24\}\).*/\1/p' "$slot/backup_label" | head -n 1)
[[ $segment =~ $WAL_NAME_RE ]] || die "the base backup has no write-ahead file name in backup_label"
[[ -f $wal_dir/$segment ]] || die "write-ahead file $segment (named by the base backup) is not in $wal_dir"
archive_failures=$(pg_psql -c "SELECT failed_count FROM pg_stat_archiver") || die "cannot read pg_stat_archiver"
note "base backup written: $slot; archived segment $segment (archiver failures so far: $archive_failures)"
printf '{"format":"apelora-base-backup/1","directory":%s,"created_at":%s,"wal_segment":%s,"archive_failed_count":%s}\n' \
  "$(json_str "base-$ts")" "$(json_str "$(utc_now)")" "$(json_str "$segment")" "$archive_failures" >"$dir/base-$ts.json"
echo "base backup written: $slot"

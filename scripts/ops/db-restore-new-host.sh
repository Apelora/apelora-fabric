#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Restores a logical backup of Apelora's database onto an empty database on another PostgreSQL.
# It refuses the server the live database is on (same host and port), whatever the database name.
# It does not drop the restored database: that database is the new plane's. Runbook:
# docs/runbooks/apelora-database-backup.md (§8).
#
#   DATABASE_URL=postgres://live… APELORA_RESTORE_DATABASE_URL=postgres://new-host…/apelora \
#     APELORA_BACKUP_DIR=/srv/apelora-backups scripts/ops/db-restore-new-host.sh
#
# The target database must already exist and contain no tables. The connecting role needs to be
# able to create objects in it. Decrypt with --age-identity or --openssl-key and --openssl-cert,
# as for the restore drill. The live database is never connected to for the restore.
# Exit status: 0 restored and verified; 1 failed (the target is left as it is); 2 usage.
set -euo pipefail
# shellcheck source=scripts/ops/db-common.sh
source "$(dirname "${BASH_SOURCE[0]}")/db-common.sh"

usage() { awk 'NR > 2 && /^#/ { sub(/^# ?/, ""); print; next } NR > 2 { exit }' "${BASH_SOURCE[0]}"; }

dir=${APELORA_BACKUP_DIR:-}
backup=""
age_identity=${APELORA_BACKUP_AGE_IDENTITY:-}
openssl_key=${APELORA_BACKUP_OPENSSL_KEY:-}
openssl_cert=${APELORA_BACKUP_OPENSSL_CERT:-}
restore_url=${APELORA_RESTORE_DATABASE_URL:-}
while (($#)); do
  case $1 in
  --dir) dir=${2:?--dir needs a value}; shift 2 ;;
  --backup) backup=${2:?--backup needs a file name}; shift 2 ;;
  --restore-url) restore_url=${2:?--restore-url needs a value}; shift 2 ;;
  --age-identity) age_identity=${2:?--age-identity needs a file}; shift 2 ;;
  --openssl-key) openssl_key=${2:?--openssl-key needs a file}; shift 2 ;;
  --openssl-cert) openssl_cert=${2:?--openssl-cert needs a file}; shift 2 ;;
  -h | --help) usage; exit 0 ;;
  *) usage >&2; echo "error: unknown argument: $1" >&2; exit 2 ;;
  esac
done
if [[ -n $backup ]]; then
  backup=${backup##*/}
  [[ $backup =~ $BACKUP_NAME_RE ]] || { echo "error: not a backup file name written by db-backup.sh: $backup" >&2; exit 2; }
fi
[[ -n ${DATABASE_URL:-} ]] || { echo "error: set DATABASE_URL to the live database so this script can refuse that server" >&2; exit 2; }
[[ -n $restore_url ]] || { echo "error: set APELORA_RESTORE_DATABASE_URL (or pass --restore-url) to an empty database on the new host" >&2; exit 2; }

started=$SECONDS
restored_at=$(utc_now)
verdict=fail reason="" checks_json=""

finish() {
  local status=$?
  [[ -n ${result_dir:-} ]] || exit "$status"
  if ((status != 0)) && [[ $verdict == pass ]]; then verdict=fail; fi
  [[ $verdict == pass || -n $reason ]] || reason="the restore stopped unexpectedly (exit status $status)"
  local tmp=$result_dir/.restore-new-host-last.json.partial
  if printf '{"format":"apelora-restore-new-host/1","restored_at":%s,"backup":%s,"duration_seconds":%s,"verdict":%s,"reason":%s,"restored_on":%s,"checks":[%s]}\n' \
    "$(json_str "$restored_at")" "$(json_str "${backup:-}")" "$((SECONDS - started))" \
    "$(json_str "$verdict")" "$(json_str "$reason")" "$(json_str "$(pg_endpoint)")" \
    "$checks_json" >"$tmp" && mv "$tmp" "$result_dir/restore-new-host-last.json"; then
    if [[ $verdict == pass ]]; then
      cp "$result_dir/restore-new-host-last.json" "$tmp" && mv "$tmp" "$result_dir/restore-new-host-last-verified.json"
    fi
  else
    note "warning: could not write the result to $result_dir"
  fi
  if [[ $verdict == pass ]]; then
    echo "restore onto the new host PASSED: $backup verified on $(pg_endpoint) in $((SECONDS - started))s (the database was kept)"
    exit 0
  fi
  echo "restore onto the new host FAILED: $reason" >&2
  echo "the target database was not dropped" >&2
  exit 1
}
fail() {
  reason=$1
  verdict=fail
  echo "error: $1" >&2
  exit 1
}

# The live URL is parsed only to learn its host and port. Nothing is restored there.
pg_env_from_url "$DATABASE_URL"
live_hostport=$(printf '%s:%s' "${PGHOST:-local socket}" "${PGPORT:-5432}")
for v in "${OPS_PG_VALUE_VARS[@]}"; do unset "$v"; done
pg_connect_setup "$restore_url"
new_hostport=$(printf '%s:%s' "${PGHOST:-local socket}" "${PGPORT:-5432}")
[[ $live_hostport != "$new_hostport" ]] || fail "refusing to restore onto $new_hostport, which is the live database's server; use a PostgreSQL on the new host"

umask 077
dir=$(backup_dir_checked "$dir")
[[ -w $dir ]] || fail "the backup directory is not writable (the result is written there): $dir"
result_dir=$dir
trap finish EXIT

if [[ -n $backup ]]; then
  [[ -f $dir/$backup && -f $dir/$backup.sha256 && -f $dir/$backup.json ]] \
    || fail "$backup, its .sha256 or its .json is missing in $dir"
else
  backup=$(list_backups "$dir" | tail -n 1)
  [[ -n $backup ]] || fail "no complete backup (file, .sha256 and .json) in $dir"
fi
file="$dir/$backup"
note "new host: $file -> $(pg_endpoint) (live server $live_hostport is not used)"

tables=$(pg_psql -c "SELECT count(*) FROM information_schema.tables WHERE table_schema = 'public' AND table_type = 'BASE TABLE'") \
  || fail "cannot query the target database $(pg_endpoint)"
[[ $tables == 0 ]] || fail "the target database $(pg_endpoint) is not empty ($tables tables); create a new empty database"

expected=$(cut -d' ' -f1 <"$file.sha256")
[[ $expected =~ ^[0-9a-f]{64}$ ]] || fail "$backup.sha256 does not hold a SHA-256"
manifest=$(head -c 1048576 "$file.json")
[[ $manifest =~ \"sha256\":\"([0-9a-f]{64})\" && ${BASH_REMATCH[1]} == "$expected" ]] \
  || fail "the manifest's checksum does not match $backup.sha256"
actual=$(sha256_of "$file")
[[ $actual == "$expected" ]] || fail "checksum mismatch for $backup: the file is damaged or was changed"
checks_json='{"check":"checksum","status":"ok"},{"check":"different server","status":"ok","detail":'"$(json_str "$live_hostport -> $new_hostport")"'}'

case $backup in
*.age)
  [[ -n $age_identity ]] || fail "$backup is encrypted with age; give --age-identity"
  command -v age >/dev/null || fail "age is not installed"
  decrypt() { age --decrypt -i "$age_identity"; }
  ;;
*.cms)
  [[ -n $openssl_key && -n $openssl_cert ]] || fail "$backup is encrypted with openssl cms; give --openssl-key and --openssl-cert"
  decrypt() { openssl cms -decrypt -binary -inform DER -inkey "$openssl_key" -recip "$openssl_cert"; }
  ;;
*) decrypt() { cat; } ;;
esac

if ! decrypt <"$file" | pg_tool pg_restore --no-password --dbname="$PGDATABASE" --no-owner --role="$PGUSER" \
  --no-privileges --exit-on-error --single-transaction; then
  checks_json+=',{"check":"restore","status":"fail"}'
  fail "pg_restore failed (see the message above)"
fi
checks_json+=',{"check":"restore","status":"ok"}'
note "  [ok] restore: pg_restore finished without errors"

run_restore_checks "$PGDATABASE" "$manifest" || fail "verification failed (see the lines above)"
verdict=pass

#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Restore drill for Apelora's own database (ADR 0011 decision 6): restores the newest (or a
# named) backup written by scripts/ops/db-backup.sh into a NEW scratch database, verifies it and
# drops it. It never restores into, changes or drops the live database. Runbook:
# docs/runbooks/apelora-database-backup.md.
#
#   DATABASE_URL=postgres://… APELORA_BACKUP_DIR=/srv/apelora-backups scripts/ops/db-restore-drill.sh
#   scripts/ops/db-restore-drill.sh --dir /srv/apelora-backups --backup apelora-db-20261021T020000Z.dump
#
# Checks: the file's SHA-256 matches; it restores without error; the migrations (count, latest
# version, fingerprint) match the backup's manifest; tables that had rows still have rows, and
# audit_events has at least as many as the manifest recorded; the audit append-only trigger
# exists and refuses an UPDATE and a DELETE of an audit row in the scratch copy.
# Result: restore-drill-last.json in the backup directory after every drill, and
# restore-drill-last-verified.json after a passing one (time, backup, duration, verdict, checks).
#
# Options (each also an environment variable):
#   --dir DIR               APELORA_BACKUP_DIR              where the backups are
#   --backup NAME           (none)                          a backup file in DIR; default the newest
#   --age-identity FILE     APELORA_BACKUP_AGE_IDENTITY     to decrypt .age backups
#   --openssl-key FILE      APELORA_BACKUP_OPENSSL_KEY      to decrypt .cms backups (with the cert)
#   --openssl-cert FILE     APELORA_BACKUP_OPENSSL_CERT
# Connection: APELORA_DRILL_DATABASE_URL (a PostgreSQL server for the scratch database, ideally
# another host) or else DATABASE_URL or the PG* variables. The role needs CREATEDB.
# Exit status: 0 verified; 1 failed (the result file says why); 2 usage.
set -euo pipefail
# shellcheck source=scripts/ops/db-common.sh
source "$(dirname "${BASH_SOURCE[0]}")/db-common.sh"

usage() { awk 'NR > 2 && /^#/ { sub(/^# ?/, ""); print; next } NR > 2 { exit }' "${BASH_SOURCE[0]}"; }

dir=${APELORA_BACKUP_DIR:-}
backup=""
age_identity=${APELORA_BACKUP_AGE_IDENTITY:-}
openssl_key=${APELORA_BACKUP_OPENSSL_KEY:-}
openssl_cert=${APELORA_BACKUP_OPENSSL_CERT:-}
while (($#)); do
  case $1 in
  --dir) dir=${2:?--dir needs a value}; shift 2 ;;
  --backup) backup=${2:?--backup needs a file name}; shift 2 ;;
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

started=$SECONDS
drilled_at=$(utc_now)
stamp=$(date -u +%Y%m%dt%H%M%Sz)
scratch="apelora_restore_drill_${stamp}_$$"
scratch_re='^apelora_restore_drill_[0-9]{8}t[0-9]{6}z_[0-9]+$'
created=0 verdict=fail reason="" backup_created_at="" checks_json="" warnings=""

# Every exit after the backup directory is known writes a result and drops the scratch
# database this run created (only that one: the name is checked against the pattern).
finish() {
  local status=$?
  if ((created == 1)) && [[ $scratch =~ $scratch_re && $scratch != "${PGDATABASE:-}" ]]; then
    if pg_psql -c "DROP DATABASE IF EXISTS \"$scratch\" WITH (FORCE)" >/dev/null; then
      note "drill: scratch database $scratch dropped"
    else
      warnings="the scratch database $scratch could not be dropped; drop it by hand"
      note "warning: $warnings"
    fi
  fi
  [[ -n ${result_dir:-} ]] || exit "$status"
  if ((status != 0)) && [[ $verdict == pass ]]; then verdict=fail; fi
  [[ $verdict == pass || -n $reason ]] || reason="the drill stopped unexpectedly (exit status $status)"
  local out tmp
  out=$(
    printf '{"format":"apelora-restore-drill/1","drilled_at":%s,"backup":%s,"backup_created_at":%s,' \
      "$(json_str "$drilled_at")" "$(json_str "${backup:-}")" "$(json_str "$backup_created_at")"
    printf '"duration_seconds":%s,"verdict":%s,"reason":%s,"warnings":%s,"restored_on":%s,"checks":[%s]}\n' \
      "$((SECONDS - started))" "$(json_str "$verdict")" "$(json_str "$reason")" "$(json_str "$warnings")" \
      "$(json_str "${PGHOST:-local socket}:${PGPORT:-5432}")" "$checks_json"
  )
  tmp="$result_dir/.restore-drill-last.json.partial"
  if printf '%s\n' "$out" >"$tmp" && mv "$tmp" "$result_dir/restore-drill-last.json"; then
    if [[ $verdict == pass ]]; then
      cp "$result_dir/restore-drill-last.json" "$tmp" && mv "$tmp" "$result_dir/restore-drill-last-verified.json"
    fi
    note "drill: result written to $result_dir/restore-drill-last.json"
  else
    note "warning: could not write the result to $result_dir"
  fi
  if [[ $verdict == pass ]]; then
    echo "restore drill PASSED: $backup restored into a scratch database and verified in $((SECONDS - started))s"
    exit 0
  fi
  echo "restore drill FAILED: $reason" >&2
  exit 1
}
fail() {
  reason=$1
  verdict=fail
  echo "error: $1" >&2
  exit 1
}

pg_connect_setup "${APELORA_DRILL_DATABASE_URL:-${DATABASE_URL:-}}"
umask 077
dir=$(backup_dir_checked "$dir")
[[ -w $dir ]] || die "the backup directory is not writable (the drill writes its result there): $dir"
result_dir=$dir
trap finish EXIT

# 1. Which backup.
if [[ -n $backup ]]; then
  [[ -f $dir/$backup && -f $dir/$backup.sha256 && -f $dir/$backup.json ]] \
    || fail "$backup, its .sha256 or its .json is missing in $dir"
else
  backup=$(list_backups "$dir" | tail -n 1)
  [[ -n $backup ]] || fail "no complete backup (file, .sha256 and .json) in $dir"
fi
file="$dir/$backup"
note "drill: $file -> scratch database $scratch on $(pg_where)"

# 2. Integrity: the checksum file, the manifest and the file itself agree.
expected=$(cut -d' ' -f1 <"$file.sha256")
[[ $expected =~ ^[0-9a-f]{64}$ ]] || fail "$backup.sha256 does not hold a SHA-256"
manifest=$(head -c 1048576 "$file.json")
[[ $manifest =~ \"sha256\":\"([0-9a-f]{64})\" && ${BASH_REMATCH[1]} == "$expected" ]] \
  || fail "the manifest's checksum does not match $backup.sha256"
[[ $manifest =~ \"created_at\":\"([0-9TZ:-]+)\" ]] && backup_created_at=${BASH_REMATCH[1]}
actual=$(sha256_of "$file")
[[ $actual == "$expected" ]] || fail "checksum mismatch for $backup: the file is damaged or was changed"
checks_json='{"check":"checksum","status":"ok"}'
note "  [ok] checksum: sha256 matches"

# 3. Decryption, streamed: no plaintext copy is written to disk.
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

# 4. Restore into a new, empty scratch database. Owners and grants are not restored: the drill
# proves the data and schema, under whatever role runs it.
[[ $scratch =~ $scratch_re && $scratch != "$PGDATABASE" ]] || fail "refusing scratch database name $scratch"
pg_psql -c "CREATE DATABASE \"$scratch\" TEMPLATE template0" >/dev/null \
  || fail "cannot create the scratch database (the role needs CREATEDB; or set APELORA_DRILL_DATABASE_URL)"
created=1
if ! decrypt <"$file" | pg_tool pg_restore --no-password --dbname="$scratch" --no-owner --no-privileges \
  --exit-on-error --single-transaction; then
  checks_json+=',{"check":"restore","status":"fail"}'
  fail "pg_restore failed (see the message above)"
fi
checks_json+=',{"check":"restore","status":"ok"}'
note "  [ok] restore: pg_restore finished without errors"

# 5. Verify the restored copy against the manifest. The probe transaction is rolled back.
run_restore_checks "$scratch" "$manifest" || fail "verification failed (see the lines above)"
verdict=pass

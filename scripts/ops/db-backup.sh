#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Backs up Apelora's own PostgreSQL database (ADR 0011 decision 6) to a directory OUTSIDE the
# cluster Apelora manages, then keeps the newest N backups. Runbook:
# docs/runbooks/apelora-database-backup.md. Prove a backup with scripts/ops/db-restore-drill.sh.
#
#   DATABASE_URL=postgres://… APELORA_BACKUP_DIR=/srv/apelora-backups scripts/ops/db-backup.sh
#   scripts/ops/db-backup.sh --dir /srv/apelora-backups --keep 30
#
# Writes, for each backup (UTC timestamp in the name):
#   apelora-db-<ts>.dump[.age|.cms]   pg_dump custom format, encrypted when a recipient is given
#   …dump[.age|.cms].sha256           SHA-256 of that file (sha256sum -c format)
#   …dump[.age|.cms].json             manifest: migrations, row counts, audit trigger, sizes
#
# Options (each also an environment variable):
#   --dir DIR              APELORA_BACKUP_DIR             existing directory, absolute, not in a checkout
#   --keep N               APELORA_BACKUP_KEEP            complete backups to keep (default 14)
#   --age-recipients FILE  APELORA_BACKUP_AGE_RECIPIENTS  encrypt with age to these public keys
#   --openssl-cert FILE    APELORA_BACKUP_OPENSSL_CERT    encrypt with openssl cms to this certificate
# Connection: DATABASE_URL or the standard PG* variables; never printed. Optional
# APELORA_PG_CLIENT_IMAGE runs the client tools in a container (see db-common.sh).
# Without a recipient the dump is written unencrypted (mode 0600): encryption at rest is then the
# backup storage's job. Only public material is needed here; private keys stay off this host.
# Exit status: 0 backup written; 1 failed (nothing kept from this run); 2 usage.
set -euo pipefail
# shellcheck source=scripts/ops/db-common.sh
source "$(dirname "${BASH_SOURCE[0]}")/db-common.sh"

usage() { awk 'NR > 2 && /^#/ { sub(/^# ?/, ""); print; next } NR > 2 { exit }' "${BASH_SOURCE[0]}"; }

dir=${APELORA_BACKUP_DIR:-}
keep=${APELORA_BACKUP_KEEP:-14}
age_recipients=${APELORA_BACKUP_AGE_RECIPIENTS:-}
openssl_cert=${APELORA_BACKUP_OPENSSL_CERT:-}
while (($#)); do
  case $1 in
  --dir) dir=${2:?--dir needs a value}; shift 2 ;;
  --keep) keep=${2:?--keep needs a value}; shift 2 ;;
  --age-recipients) age_recipients=${2:?--age-recipients needs a file}; shift 2 ;;
  --openssl-cert) openssl_cert=${2:?--openssl-cert needs a file}; shift 2 ;;
  -h | --help) usage; exit 0 ;;
  *) usage >&2; echo "error: unknown argument: $1" >&2; exit 2 ;;
  esac
done
[[ $keep =~ ^[1-9][0-9]{0,3}$ ]] || { echo "error: --keep must be a whole number from 1 to 9999" >&2; exit 2; }

# Encryption: optional, chosen by the operator, public material only.
encryption=none suffix=""
if [[ -n $age_recipients && -n $openssl_cert ]]; then
  echo "error: give either --age-recipients or --openssl-cert, not both" >&2
  exit 2
fi
refuse_private() { ! grep -qE 'PRIVATE KEY|AGE-SECRET-KEY' "$1" 2>/dev/null || die "$1 contains a private key; this host needs only the public recipient (keep private keys off the backup host)"; }
if [[ -n $age_recipients ]]; then
  command -v age >/dev/null || die "--age-recipients is set but age is not installed"
  [[ -r $age_recipients ]] || die "cannot read the age recipients file: $age_recipients"
  refuse_private "$age_recipients"
  encryption=age suffix=.age
elif [[ -n $openssl_cert ]]; then
  command -v openssl >/dev/null || die "--openssl-cert is set but openssl is not installed"
  [[ -r $openssl_cert ]] || die "cannot read the certificate: $openssl_cert"
  refuse_private "$openssl_cert"
  openssl x509 -noout -in "$openssl_cert" 2>/dev/null || die "not a PEM certificate: $openssl_cert"
  encryption=openssl-cms suffix=.cms
fi
encrypt() {
  case $encryption in
  none) cat ;;
  age) age --encrypt -R "$age_recipients" ;;
  openssl-cms) openssl cms -encrypt -binary -stream -outform DER -aes256 "$openssl_cert" ;;
  esac
}

pg_connect_setup "${DATABASE_URL:-}"
umask 077
dir=$(backup_dir_checked "$dir")
[[ -w $dir ]] || die "the backup directory is not writable: $dir"

lock="$dir/.apelora-db-backup.lock"
mkdir "$lock" 2>/dev/null || die "another backup is running in $dir (if none is, remove the stale lock $lock)"
ts=$(date -u +%Y%m%dT%H%M%SZ)
name="apelora-db-$ts.dump$suffix"
final="$dir/$name"
partial="$dir/.$name.partial"
done_ok=0
cleanup() {
  if ((done_ok == 0)); then rm -f "$partial" "$partial.sha256" "$partial.json"; fi
  rmdir "$lock" 2>/dev/null || true
}
trap cleanup EXIT
[[ ! -e $final ]] || die "a backup named $name already exists; wait a second and run again"

note "backup: $(pg_where) -> $final (encryption: $encryption)"
started=$SECONDS
created_at=$(utc_now)

# What the backup should contain, read in one statement: migrations (count, latest version and
# a fingerprint of version:checksum), the exact row count of every table, and whether the
# audit table's append-only trigger is present. Recorded in the manifest; the drill compares.
read -r -d '' contents_sql <<'SQL' || true
SELECT json_build_object(
  'database', current_database(),
  'server_version', current_setting('server_version'),
  'migrations', (SELECT json_build_object(
      'count', count(*),
      'latest', max(version),
      'fingerprint', md5(coalesce(string_agg(version::text || ':' || encode(checksum, 'hex'), ',' ORDER BY version), '')))
    FROM public._sqlx_migrations WHERE success),
  'row_counts', (SELECT coalesce(json_object_agg(c.table_name, c.n ORDER BY c.table_name), '{}'::json) FROM (
      SELECT table_name::text,
             (xpath('/row/n/text()', query_to_xml(format('SELECT count(*) AS n FROM %I.%I', table_schema, table_name), false, true, '')))[1]::text::bigint AS n
      FROM information_schema.tables
      WHERE table_schema = 'public' AND table_type = 'BASE TABLE') c),
  'audit_append_only_trigger', EXISTS (
      SELECT 1 FROM pg_trigger
      WHERE tgname = 'audit_events_no_change' AND tgrelid = 'public.audit_events'::regclass AND NOT tgisinternal)
)
SQL
fingerprint_sql="SELECT md5(coalesce(string_agg(version::text || ':' || encode(checksum, 'hex'), ',' ORDER BY version), '')) FROM public._sqlx_migrations WHERE success"

looks=$(pg_psql -c "SELECT to_regclass('public._sqlx_migrations') IS NOT NULL AND to_regclass('public.audit_events') IS NOT NULL") \
  || die "cannot query $(pg_where); check the connection settings and that the role may read the database"
[[ $looks == t ]] || die "$(pg_where) is not an Apelora database (no _sqlx_migrations or audit_events table); check DATABASE_URL"
contents=$(pg_psql -c "$contents_sql") || die "cannot read the database's migrations and row counts"
before=$(pg_psql -c "$fingerprint_sql") || die "cannot read the migrations table"
dump_version=$(pg_tool pg_dump --version) || die "cannot run pg_dump"

# The dump streams from pg_dump through the optional encryption straight into the file.
# --lock-wait-timeout: fail instead of queueing behind a schema change.
if ! pg_tool pg_dump --no-password --format=custom --lock-wait-timeout=60000 | encrypt >"$partial"; then
  die "pg_dump failed (see the message above); nothing was kept"
fi
[[ -s $partial ]] || die "pg_dump wrote nothing; nothing was kept"
after=$(pg_psql -c "$fingerprint_sql") || die "cannot read the migrations table after the dump"
[[ $before == "$after" ]] || die "the database's migrations changed during the backup (an upgrade ran); run the backup again"

# An unencrypted dump is checked here: its table of contents must be readable and must hold the
# audit table's data and its append-only trigger. An encrypted one is checked by the drill,
# which holds the private key.
if [[ $encryption == none ]]; then
  toc=$(pg_tool pg_restore --list <"$partial") || die "the dump's table of contents cannot be read; nothing was kept"
  grep -q 'TABLE DATA public audit_events ' <<<"$toc" || die "the dump has no data entry for audit_events; nothing was kept"
  grep -q 'TRIGGER public audit_events audit_events_no_change ' <<<"$toc" || die "the dump has no audit_events_no_change trigger; nothing was kept"
fi

sync "$partial" 2>/dev/null || sync
sum=$(sha256_of "$partial")
size=$(wc -c <"$partial" | tr -d ' ')
duration=$((SECONDS - started))
printf '%s  %s\n' "$sum" "$name" >"$partial.sha256"
{
  printf '{"format":"apelora-db-backup/1","file":%s,"created_at":%s,"sha256":%s,"size_bytes":%s,' \
    "$(json_str "$name")" "$(json_str "$created_at")" "$(json_str "$sum")" "$size"
  printf '"encryption":%s,"pg_dump_version":%s,"duration_seconds":%s,"contents":%s}\n' \
    "$(json_str "$encryption")" "$(json_str "$dump_version")" "$duration" "$contents"
} >"$partial.json"
mv "$partial.json" "$final.json"
mv "$partial.sha256" "$final.sha256"
mv "$partial" "$final"
done_ok=1

# Retention: only complete backups with names this script writes are counted or removed.
mapfile -t backups < <(list_backups "$dir")
removed=0
if ((${#backups[@]} > keep)); then
  for old in "${backups[@]:0:${#backups[@]}-keep}"; do
    rm -f "$dir/$old" "$dir/$old.sha256" "$dir/$old.json"
    removed=$((removed + 1))
  done
fi

echo "backup written: $final ($size bytes, sha256 $sum, ${duration}s); kept $((${#backups[@]} - removed)), removed $removed"

#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Self-test for db-backup.sh and db-restore-drill.sh against a real PostgreSQL holding a migrated
# Apelora database (the dev database from infra/dev/compose.yaml, or CI's service). It only
# READS that database: backups go to a temporary directory outside the repository, and drills
# restore into scratch databases that they drop. Run in CI (rust job) and by hand:
#
#   DATABASE_URL=postgres://apelora:apelora-dev-only@127.0.0.1:15432/apelora \
#   APELORA_PG_CLIENT_IMAGE=postgres:18-alpine scripts/ops/test-db-backup.sh
#
# Prints PASS/FAIL per check; exits non-zero when any check fails.
set -euo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)
# shellcheck source=scripts/ops/db-common.sh
source "$here/db-common.sh"
[[ -n ${DATABASE_URL:-} ]] || die "set DATABASE_URL to a migrated Apelora database (it is only read)"

work=$(mktemp -d "${TMPDIR:-/tmp}/apelora-db-backup-test.XXXXXX")
trap 'rm -rf "$work"' EXIT
dir=$work/backups other=$work/other log=$work/log
mkdir -p "$dir" "$other"
# The scripts run under the same bash as this test (on macOS /usr/bin/env bash is too old).
backup=("$BASH" "$here/db-backup.sh") drill=("$BASH" "$here/db-restore-drill.sh")
fails=0
pass() { echo "PASS  $1"; }
flunk() { echo "FAIL  $1"; fails=$((fails + 1)); }
# expect <0|1|2> <description> <command...>: runs it, output appended to the log.
expect() {
  local want=$1 what=$2 got=0
  shift 2
  "$@" >>"$log" 2>&1 || got=$?
  if [[ $got == "$want" ]]; then pass "$what"; else flunk "$what (exit $got, wanted $want; see below)"; tail -n 5 "$log" | sed 's/^/      /'; fi
}
# The scratch databases present now; a drill must leave none behind.
( pg_env_from_url "$DATABASE_URL"; pg_connect_setup ""
  pg_psql -c "SELECT count(*) FROM pg_database WHERE datname LIKE 'apelora_restore_drill_%'" ) >"$work/scratch-before"

# Refusals: no directory, a relative one, one inside this repository.
expect 1 "backup refuses a missing backup directory" env -u APELORA_BACKUP_DIR "${backup[@]}"
expect 1 "backup refuses a relative directory" "${backup[@]}" --dir relative/path
if [[ -n $OPS_REPO_ROOT ]]; then
  expect 1 "backup refuses a directory inside the repository" "${backup[@]}" --dir "$OPS_REPO_ROOT/scripts/ops"
  if tail -n 1 "$log" | grep -q 'inside the Apelora repository'; then
    pass "the refusal names the repository as the reason"
  else
    flunk "the refusal names the repository as the reason"
  fi
else
  echo "SKIP  refusal of a directory inside the repository (not run from a checkout)"
fi
expect 2 "backup refuses a bad --keep" "${backup[@]}" --dir "$dir" --keep 0

# A plain backup, then a drill of it.
expect 0 "backup writes a dump, checksum and manifest" "${backup[@]}" --dir "$dir" --keep 5
first=$(list_backups "$dir" | tail -n 1)
[[ -n $first ]] || { cat "$log"; die "no backup was written; the remaining checks need one"; }
if [[ -n $first ]] && (cd "$dir" && { sha256sum -c "$first.sha256" 2>/dev/null || shasum -a 256 -c "$first.sha256"; }) >>"$log" 2>&1; then
  pass "the checksum file verifies with the standard tool"
else
  flunk "the checksum file verifies with the standard tool"
fi
if [[ -n $(find "$dir/$first" -perm 0600) ]]; then pass "the dump is readable by its owner only"; else flunk "the dump is readable by its owner only"; fi
expect 0 "drill restores the newest backup into a scratch database and verifies it" "${drill[@]}" --dir "$dir"
if grep -q '"verdict":"pass"' "$dir/restore-drill-last.json" && [[ -f $dir/restore-drill-last-verified.json ]]; then
  pass "the drill result records a pass"
else
  flunk "the drill result records a pass"
fi

# Retention: two older complete sets plus the real one, --keep 2 after one more backup.
for ts in 20200101T000000Z 20200102T000000Z; do
  for ext in "" .sha256 .json; do cp "$dir/$first$ext" "$dir/apelora-db-$ts.dump$ext"; done
done
expect 0 "a second backup runs" "${backup[@]}" --dir "$dir" --keep 2
mapfile -t kept < <(list_backups "$dir")
if [[ ${#kept[@]} == 2 && ${kept[0]} == "$first" ]]; then
  pass "retention keeps the newest 2 and removes the oldest"
else
  flunk "retention keeps the newest 2 and removes the oldest (kept: ${kept[*]})"
fi

# A damaged file is caught by the checksum, recorded as a failure, and nothing is restored.
for ext in "" .sha256 .json; do cp "$dir/$first$ext" "$other/$first$ext"; done
printf 'X' | dd of="$other/$first" bs=1 seek=100 conv=notrunc 2>/dev/null
expect 1 "drill refuses a damaged backup" "${drill[@]}" --dir "$other"
if grep -q '"verdict":"fail"' "$other/restore-drill-last.json" && grep -q 'checksum mismatch' "$other/restore-drill-last.json"; then
  pass "the failed drill records the reason"
else
  flunk "the failed drill records the reason"
fi
if [[ ! -e $other/restore-drill-last-verified.json ]]; then
  pass "a failed drill does not count as verified"
else
  flunk "a failed drill does not count as verified"
fi
expect 2 "drill refuses a name that is not a backup" "${drill[@]}" --dir "$dir" --backup ../../etc/passwd

# Encryption with openssl cms: only the certificate on the backup side, the key for the drill.
if command -v openssl >/dev/null; then
  keys=$work/keys
  mkdir -p "$keys"
  openssl req -x509 -newkey rsa:2048 -nodes -subj /CN=apelora-backup-test -days 1 \
    -keyout "$keys/key.pem" -out "$keys/cert.pem" >>"$log" 2>&1
  enc=$work/encrypted
  mkdir -p "$enc"
  expect 1 "backup refuses a private key given as the recipient" "${backup[@]}" --dir "$enc" --openssl-cert "$keys/key.pem"
  expect 0 "backup encrypts with openssl cms to a certificate" "${backup[@]}" --dir "$enc" --openssl-cert "$keys/cert.pem"
  expect 1 "drill of an encrypted backup without the key fails clearly" "${drill[@]}" --dir "$enc"
  expect 0 "drill decrypts, restores and verifies an encrypted backup" \
    "${drill[@]}" --dir "$enc" --openssl-key "$keys/key.pem" --openssl-cert "$keys/cert.pem"
else
  echo "SKIP  openssl cms encryption (openssl not installed)"
fi

# Encryption with age: only public recipients on the backup side, the identity for the drill.
if command -v age >/dev/null && command -v age-keygen >/dev/null; then
  keys=$work/age
  mkdir -p "$keys"
  age-keygen -o "$keys/identity" >>"$log" 2>&1
  age-keygen -y "$keys/identity" >"$keys/recipients"
  aged=$work/aged
  mkdir -p "$aged"
  expect 1 "backup refuses an age identity given as the recipient" "${backup[@]}" --dir "$aged" --age-recipients "$keys/identity"
  expect 0 "backup encrypts with age to a public recipient" "${backup[@]}" --dir "$aged" --age-recipients "$keys/recipients"
  expect 1 "drill of an age backup without the identity fails clearly" "${drill[@]}" --dir "$aged"
  expect 0 "drill decrypts, restores and verifies an age backup" "${drill[@]}" --dir "$aged" --age-identity "$keys/identity"
else
  if [[ ${APELORA_REQUIRE_AGE:-0} == 1 ]]; then
    flunk "age encryption (age is not installed)"
  else
    echo "SKIP  age encryption (age not installed)"
  fi
fi

# No password in anything printed or written; no scratch database left behind.
password=$(pg_env_from_url "$DATABASE_URL"; printf '%s' "${PGPASSWORD:-}")
if [[ -n $password ]] && grep -rqF -- "$password" "$log" "$dir" "$other"; then
  flunk "the database password appears in output or files"
else
  pass "the database password appears in no output or file"
fi
( pg_env_from_url "$DATABASE_URL"; pg_connect_setup ""
  pg_psql -c "SELECT count(*) FROM pg_database WHERE datname LIKE 'apelora_restore_drill_%'" ) >"$work/scratch-after"
if cmp -s "$work/scratch-before" "$work/scratch-after"; then
  pass "no scratch database is left behind"
else
  flunk "no scratch database is left behind"
fi

if ((fails > 0)); then
  echo "--- log ---"
  cat "$log"
  echo "$fails check(s) failed"
  exit 1
fi
echo "all checks passed"

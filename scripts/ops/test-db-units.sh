#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Starts the example backup and restore-drill units under systemd, against DATABASE_URL, and
# checks that the timers are waiting on their calendars. Removes the units afterwards.
# On a machine without systemd the checks are skipped unless APELORA_REQUIRE_SYSTEMD=1.
#
#   sudo --preserve-env=DATABASE_URL,APELORA_REQUIRE_SYSTEMD bash scripts/ops/test-db-units.sh
#
# The backup directory, env file and units are the paths the examples name. Do not point
# DATABASE_URL at a database whose password must stay off this machine: it is written to
# /etc/apelora/db-backup.env for the service and removed at the end.
set -euo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)
root=$(cd "$here/../.." && pwd -P)
need=${APELORA_REQUIRE_SYSTEMD:-0}

if ! command -v systemctl >/dev/null || ! [[ -d /run/systemd/system ]]; then
  if [[ $need == 1 ]]; then
    echo "error: systemd is required" >&2
    exit 1
  fi
  echo "skip: systemd is not running on this machine (CI starts the units on ubuntu-24.04)"
  exit 0
fi
[[ $(id -u) == 0 ]] || { echo "error: run as root (sudo --preserve-env=DATABASE_URL)" >&2; exit 1; }
[[ -n ${DATABASE_URL:-} ]] || { echo "error: set DATABASE_URL (a migrated Apelora database; it is only read)" >&2; exit 1; }

# /usr/bin/pg_dump is a wrapper that wants a cluster. The service's PATH has /usr/local/bin
# first, so the real 18 binaries go there and the wrapper is never consulted.
pg18=/usr/lib/postgresql/18/bin
if [[ ! -x $pg18/pg_dump ]]; then
  export DEBIAN_FRONTEND=noninteractive
  apt-get update -qq
  apt-get install -yq postgresql-common ca-certificates
  /usr/share/postgresql-common/pgdg/apt.postgresql.org.sh -y
  apt-get install -yq postgresql-client-18
fi
[[ -x $pg18/pg_dump && -x $pg18/pg_restore && -x $pg18/psql ]] || { echo "error: PostgreSQL 18 client tools are not in $pg18" >&2; exit 1; }
for tool in pg_dump pg_restore psql; do
  ln -sfn "$pg18/$tool" "/usr/local/bin/$tool"
done
"$pg18/pg_dump" --version
command -v bash >/dev/null || { echo "error: bash is required" >&2; exit 1; }

if ! getent passwd apelora >/dev/null; then
  useradd --system --no-create-home --shell /usr/sbin/nologin apelora
fi
install -d -m 0755 /opt/apelora/ops /etc/apelora
install -d -m 0700 -o apelora -g apelora /srv/apelora-backups
install -m 0755 "$here/db-common.sh" "$here/db-backup.sh" "$here/db-restore-drill.sh" \
  "$here/db-verify.sql" /opt/apelora/ops/
# The env file is readable by the service user and by root. It is removed on the way out.
umask 077
cat >/etc/apelora/db-backup.env <<EOF
DATABASE_URL=${DATABASE_URL}
APELORA_BACKUP_DIR=/srv/apelora-backups
APELORA_BACKUP_KEEP=5
EOF
chown root:apelora /etc/apelora/db-backup.env
chmod 0640 /etc/apelora/db-backup.env

install -m 0644 "$root/infra/systemd/apelora-db-backup.service" \
  "$root/infra/systemd/apelora-db-backup.timer" \
  "$root/infra/systemd/apelora-db-restore-drill.service" \
  "$root/infra/systemd/apelora-db-restore-drill.timer" \
  /etc/systemd/system/
systemctl daemon-reload

cleanup() {
  local status=$?
  systemctl disable --now apelora-db-backup.timer apelora-db-restore-drill.timer >/dev/null 2>&1 || true
  rm -f /etc/systemd/system/apelora-db-backup.service /etc/systemd/system/apelora-db-backup.timer \
    /etc/systemd/system/apelora-db-restore-drill.service /etc/systemd/system/apelora-db-restore-drill.timer \
    /etc/apelora/db-backup.env
  local tool target
  for tool in pg_dump pg_restore psql; do
    target=$(readlink "/usr/local/bin/$tool" 2>/dev/null || true)
    if [[ $target == "$pg18/$tool" ]]; then rm -f "/usr/local/bin/$tool"; fi
  done
  systemctl daemon-reload >/dev/null 2>&1 || true
  if ((status != 0)); then
    echo "--- backup unit ---" >&2
    journalctl -u apelora-db-backup.service -n 40 --no-pager >&2 || true
    echo "--- drill unit ---" >&2
    journalctl -u apelora-db-restore-drill.service -n 40 --no-pager >&2 || true
  fi
  exit "$status"
}
trap cleanup EXIT

systemctl start apelora-db-backup.service
systemctl start apelora-db-restore-drill.service
compgen -G '/srv/apelora-backups/apelora-db-*.dump' >/dev/null || { echo "error: the backup unit wrote no dump" >&2; exit 1; }
grep -q '"verdict":"pass"' /srv/apelora-backups/restore-drill-last-verified.json || { echo "error: the drill unit did not record a pass" >&2; exit 1; }

systemctl enable --now apelora-db-backup.timer apelora-db-restore-drill.timer
[[ $(systemctl is-active apelora-db-backup.timer) == active ]] || { echo "error: the backup timer is not active" >&2; exit 1; }
[[ $(systemctl is-active apelora-db-restore-drill.timer) == active ]] || { echo "error: the drill timer is not active" >&2; exit 1; }
systemctl show apelora-db-backup.timer -p TimersCalendar --value | grep -q '02:15:00' || { echo "error: the backup timer is not set for 02:15 UTC" >&2; exit 1; }
systemctl show apelora-db-restore-drill.timer -p TimersCalendar --value | grep -q 'Sun' || { echo "error: the drill timer is not set for Sunday" >&2; exit 1; }

echo "systemd units: backup written, drill passed, both timers active"

# shellcheck shell=bash
# SPDX-License-Identifier: AGPL-3.0-only
# Shared helpers for scripts/ops/db-backup.sh and scripts/ops/db-restore-drill.sh. Sourced, not
# run. Runbook: docs/runbooks/apelora-database-backup.md.
#
# Connection: DATABASE_URL (the same URL the control plane uses) or the standard PostgreSQL
# variables (PGHOST, PGPORT, PGUSER, PGDATABASE, PGPASSFILE, ...). A URL is split into PG*
# variables here so that no password ever appears on a command line, in `ps` or in a log.
#
# Client tools: pg_dump, pg_restore and psql from PATH. Where they are not installed (a
# workstation, CI), set APELORA_PG_CLIENT_IMAGE to a PostgreSQL container image of the same or a
# newer major version than the server (e.g. postgres:18-alpine): each tool then runs in a
# throwaway container on the host network, with the connection passed as environment variables.

if ((BASH_VERSINFO[0] < 4)); then
  echo "error: bash 4 or newer is required (on macOS: /opt/homebrew/bin/bash $0)" >&2
  exit 2
fi

die() {
  echo "error: $*" >&2
  exit 1
}
note() { echo "$*" >&2; }

# The repository these scripts belong to, when they run from a checkout (empty when they are
# installed elsewhere, e.g. /opt/apelora/ops); backups and results are never written inside it.
OPS_REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)
[[ -f $OPS_REPO_ROOT/AGENTS.md && -d $OPS_REPO_ROOT/scripts/ops ]] || OPS_REPO_ROOT=""

# Variables forwarded to a client container. File-based ones (PGPASSFILE, PGSSLROOTCERT, ...)
# would name paths that do not exist inside it, so they are refused in container mode.
OPS_PG_VALUE_VARS=(PGHOST PGHOSTADDR PGPORT PGUSER PGPASSWORD PGDATABASE PGSSLMODE PGAPPNAME
  PGCONNECT_TIMEOUT PGOPTIONS PGTARGETSESSIONATTRS PGREQUIRESSL PGGSSENCMODE PGTZ)
OPS_PG_FILE_VARS=(PGPASSFILE PGSERVICE PGSERVICEFILE PGSSLROOTCERT PGSSLCERT PGSSLKEY PGSSLCRL)

urldecode() {
  local s=${1//\\/\\\\}
  printf '%b' "${s//%/\\x}"
}

# Splits a postgres:// URL into PG* variables (exported). Never prints the URL.
pg_env_from_url() {
  local url=$1 re='^postgres(ql)?://(([^@/?]*)@)?([^/?]*)(/([^?]*))?(\?(.*))?$'
  [[ $url =~ $re ]] || die "the database URL is not a postgres:// URL (it is not shown here)"
  local userinfo=${BASH_REMATCH[3]} hostport=${BASH_REMATCH[4]} db=${BASH_REMATCH[6]} query=${BASH_REMATCH[8]}
  if [[ -n $userinfo ]]; then
    if [[ $userinfo == *:* ]]; then
      export PGUSER PGPASSWORD
      PGUSER=$(urldecode "${userinfo%%:*}")
      PGPASSWORD=$(urldecode "${userinfo#*:}")
    else
      export PGUSER
      PGUSER=$(urldecode "$userinfo")
    fi
  fi
  [[ $hostport == *,* ]] && die "database URLs with several hosts are not supported here; set PGHOST and PGPORT instead"
  if [[ $hostport =~ ^\[([0-9A-Fa-f:.]+)\](:([0-9]+))?$ ]]; then
    export PGHOST=${BASH_REMATCH[1]}
    [[ -n ${BASH_REMATCH[3]} ]] && export PGPORT=${BASH_REMATCH[3]}
  elif [[ $hostport =~ ^([^:]*)(:([0-9]+))?$ ]]; then
    [[ -n ${BASH_REMATCH[1]} ]] && export PGHOST
    [[ -n ${BASH_REMATCH[1]} ]] && PGHOST=$(urldecode "${BASH_REMATCH[1]}")
    [[ -n ${BASH_REMATCH[3]} ]] && export PGPORT=${BASH_REMATCH[3]}
  else
    die "the host part of the database URL is not valid"
  fi
  if [[ -n $db ]]; then
    export PGDATABASE
    PGDATABASE=$(urldecode "$db")
  fi
  local pair key value
  if [[ -n $query ]]; then
    local -a pairs
    IFS='&' read -r -a pairs <<<"$query"
    for pair in "${pairs[@]}"; do
      key=${pair%%=*} value=$(urldecode "${pair#*=}")
      case $key in
      sslmode) export PGSSLMODE=$value ;;
      sslrootcert) export PGSSLROOTCERT=$value ;;
      sslcert) export PGSSLCERT=$value ;;
      sslkey) export PGSSLKEY=$value ;;
      application_name) export PGAPPNAME=$value ;;
      connect_timeout) export PGCONNECT_TIMEOUT=$value ;;
      *) die "database URL parameter '$key' is not supported here; set the matching PG* variable instead" ;;
      esac
    done
  fi
}

# Sets up the connection from $1 (a URL; may be empty) or from the PG* variables already set.
pg_connect_setup() {
  local url=${1:-}
  if [[ -n $url ]]; then pg_env_from_url "$url"; fi
  export PGCONNECT_TIMEOUT=${PGCONNECT_TIMEOUT:-10}
  export PGAPPNAME=${PGAPPNAME:-apelora-db-ops}
  [[ -n ${PGDATABASE:-} ]] || die "no database named: set DATABASE_URL (as for the control plane) or PGDATABASE"
  if [[ -n ${APELORA_PG_CLIENT_IMAGE:-} ]]; then
    command -v docker >/dev/null || die "APELORA_PG_CLIENT_IMAGE is set but docker is not installed"
    local v
    for v in "${OPS_PG_FILE_VARS[@]}"; do
      [[ -z ${!v:-} ]] || die "$v names a file, which a client container cannot see; install the PostgreSQL client tools instead of using APELORA_PG_CLIENT_IMAGE"
    done
  else
    local t
    for t in pg_dump pg_restore psql; do
      command -v "$t" >/dev/null || die "$t is not installed; install the PostgreSQL client tools (same or newer major version as the server) or set APELORA_PG_CLIENT_IMAGE=postgres:<major>-alpine"
    done
  fi
}

# Where the connection points, for messages: host, port and database, never user or password.
pg_where() { printf '%s:%s/%s' "${PGHOST:-local socket}" "${PGPORT:-5432}" "${PGDATABASE:-?}"; }

# Runs a PostgreSQL client tool (pg_dump, pg_restore, psql) natively or in a client container.
# Standard input and output pass through, so dumps stream without a shared directory.
pg_tool() {
  if [[ -n ${APELORA_PG_CLIENT_IMAGE:-} ]]; then
    local -a envs=()
    local v
    for v in "${OPS_PG_VALUE_VARS[@]}"; do
      [[ -n ${!v:-} ]] && envs+=(-e "$v")
    done
    docker run --rm -i --network host --log-driver none "${envs[@]}" "$APELORA_PG_CLIENT_IMAGE" "$@"
  else
    "$@"
  fi
}

# psql that never prompts, stops at the first error and prints unaligned, tuple-only output.
pg_psql() { pg_tool psql --no-password --no-psqlrc -v ON_ERROR_STOP=1 -X -q -A -t "$@"; }

sha256_of() {
  if command -v sha256sum >/dev/null; then
    sha256sum "$1" | cut -d' ' -f1
  elif command -v shasum >/dev/null; then
    shasum -a 256 "$1" | cut -d' ' -f1
  else
    die "neither sha256sum nor shasum is installed"
  fi
}

# The physical path of an existing directory, refused when it lies inside a source checkout
# (this repository or any directory with a .git above it).
backup_dir_checked() {
  local dir=$1
  [[ -n $dir ]] || die "no backup directory: pass --dir or set APELORA_BACKUP_DIR (storage outside the cluster Apelora manages)"
  [[ $dir == /* ]] || die "the backup directory must be an absolute path: $dir"
  [[ -d $dir ]] || die "the backup directory does not exist: $dir (mount the off-cluster storage and create it first)"
  local phys
  phys=$(cd "$dir" && pwd -P) || die "cannot enter the backup directory: $dir"
  [[ -z $OPS_REPO_ROOT ]] || case "$phys/" in
  "$OPS_REPO_ROOT"/*) die "refusing to use a directory inside the Apelora repository ($phys); backups belong on storage outside the managed cluster" ;;
  esac
  local p=$phys
  while [[ -n $p && $p != / ]]; do
    [[ -e $p/.git ]] && die "refusing to use a directory inside a source checkout ($p); backups belong on storage outside the managed cluster"
    p=${p%/*}
  done
  printf '%s' "$phys"
}

utc_now() { date -u +%Y-%m-%dT%H:%M:%SZ; }

# JSON string literal for our own messages (escapes backslash, quote and control characters).
json_str() {
  local s=$1
  s=${s//\\/\\\\}
  s=${s//\"/\\\"}
  s=${s//$'\n'/\\n}
  s=${s//$'\r'/\\r}
  s=${s//$'\t'/\\t}
  s=$(printf '%s' "$s" | LC_ALL=C tr -d '\000-\037')
  printf '"%s"' "$s"
}

# Backup files: apelora-db-<UTC timestamp>.dump, optionally .age or .cms, beside .sha256 and .json.
BACKUP_NAME_RE='^apelora-db-[0-9]{8}T[0-9]{6}Z\.dump(\.age|\.cms)?$'

# Prints complete backups in a directory (data file, checksum and manifest all present), oldest first.
list_backups() {
  local dir=$1 f base
  for f in "$dir"/apelora-db-*.dump "$dir"/apelora-db-*.dump.age "$dir"/apelora-db-*.dump.cms; do
    [[ -f $f ]] || continue
    base=${f##*/}
    [[ $base =~ $BACKUP_NAME_RE ]] || continue
    [[ -f $f.sha256 && -f $f.json ]] || continue
    printf '%s\n' "$base"
  done | sort
}

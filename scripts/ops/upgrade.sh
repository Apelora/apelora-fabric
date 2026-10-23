#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Stages an upgrade of Apelora's own binaries and web files, and can put the previous
# copies back. Dry-run unless APPLY=1. It only writes under PREFIX. It never stops or
# starts systemd: the plan prints those commands for the operator (docs/runbooks/offline-update.md).
#
#   BUNDLE=/path/to/apelora-<version> PREFIX=/usr/local/share/apelora \
#     BACKUP=/srv/apelora-backups/apelora-db-<ts>.dump scripts/ops/upgrade.sh
#   APPLY=1 … scripts/ops/upgrade.sh
#   APPLY=1 ROLLBACK=1 PREFIX=/usr/local/share/apelora scripts/ops/upgrade.sh
#
# BUNDLE is an unpacked release directory (scripts/release/verify.sh first). PREFIX is where
# this installation keeps bin/ and web/ (not the systemd unit paths; copy onwards from here).
# BACKUP is the database dump taken before APPLY. Migrations cannot be undone, so the dump
# is the database half of the rollback; this script does not restore PostgreSQL.
# Exit status: 0 plan or apply done; 1 refused; 2 usage.
set -euo pipefail

if [[ ${BASH_VERSINFO[0]} -lt 4 && -x /opt/homebrew/bin/bash && -z ${APELORA_REEXEC:-} ]]; then
  APELORA_REEXEC=1 exec /opt/homebrew/bin/bash "$0" "$@"
fi

die() { echo "upgrade: $*" >&2; exit 1; }
usage() { echo "usage: BUNDLE=… PREFIX=… BACKUP=… [APPLY=1] [ROLLBACK=1] $0" >&2; exit 2; }

ROOT=$(cd "$(dirname "$0")/../.." && pwd -P)
APPLY=${APPLY:-0}
ROLLBACK=${ROLLBACK:-0}
BUNDLE=${BUNDLE:-}
PREFIX=${PREFIX:-}
BACKUP=${BACKUP:-}
[[ $APPLY == 0 || $APPLY == 1 ]] || usage
[[ $ROLLBACK == 0 || $ROLLBACK == 1 ]] || usage

abs_dir() {
  local p=$1
  [[ $p == /* ]] || die "$2 must be an absolute path"
  [[ -d $p ]] || die "$2 does not exist: $p"
  (cd "$p" && pwd -P)
}

need_bundle() {
  [[ -n $BUNDLE ]] || die "set BUNDLE to the unpacked release directory"
  BUNDLE=$(abs_dir "$BUNDLE" BUNDLE)
  case $BUNDLE/ in "$ROOT"/*) die "BUNDLE is inside the repository; unpack the release outside it" ;; esac
  local f
  for f in bin/apelora-control-plane bin/apelora-deployment-worker web/studio web/console BUILD-INFO; do
    [[ -e $BUNDLE/$f ]] || die "bundle is missing $f"
  done
  [[ -f $BUNDLE/bin/apelora-control-plane && -f $BUNDLE/bin/apelora-deployment-worker ]] || die "bundle binaries are not files"
  version=$(sed -n 's/^version: //p' "$BUNDLE/BUILD-INFO" | head -1)
  [[ -n $version ]] || die "BUILD-INFO has no version line"
}

copy_tree() {
  local from=$1 to=$2
  mkdir -p "$to"
  rm -rf "$to/bin" "$to/web"
  mkdir -p "$to/bin" "$to/web"
  cp -p "$from/bin/apelora-control-plane" "$from/bin/apelora-deployment-worker" "$to/bin/"
  cp -R "$from/web/studio" "$from/web/console" "$to/web/"
}

plan() {
  cat <<EOF
staged upgrade of Apelora (dry run; nothing is copied or stopped)
1. Back up the database first. Migrations cannot be undone; the dump is the rollback.
   Keep the file you will pass as BACKUP.
2. Check the bundle: scripts/release/verify.sh <archive> <allowed_signers>
   version in this bundle: $version
3. Stop the worker, then the control plane:
   systemctl stop apelora-deployment-worker
   systemctl stop apelora-control-plane
4. APPLY=1 with this BUNDLE, PREFIX and BACKUP copies bin/ and web/ into PREFIX
   and keeps the previous copies in PREFIX/previous.
5. Start the control plane, then the worker:
   systemctl start apelora-control-plane
   systemctl start apelora-deployment-worker
6. Check GET /api/v1/health for version $version before treating the upgrade as done.
Rollback of the files: APPLY=1 ROLLBACK=1 PREFIX=$PREFIX
   Then restore the database from the dump taken in step 1, and start the old binaries.
   Running instances are not stopped by any of these steps.
EOF
}

if [[ $ROLLBACK == 1 ]]; then
  [[ $APPLY == 1 ]] || die "ROLLBACK=1 needs APPLY=1"
  [[ -n $PREFIX ]] || die "set PREFIX"
  PREFIX=$(abs_dir "$PREFIX" PREFIX)
  case $PREFIX/ in "$ROOT"/*) die "PREFIX is inside the repository" ;; esac
  [[ -f $PREFIX/previous/bin/apelora-control-plane && -f $PREFIX/previous/bin/apelora-deployment-worker ]] ||
    die "PREFIX/previous has no binaries to put back"
  copy_tree "$PREFIX/previous" "$PREFIX"
  printf '%s\n' "rolled-back: $(date -u +%Y-%m-%dT%H:%M:%SZ)" >"$PREFIX/upgrade-record"
  echo "upgrade: restored bin/ and web/ from $PREFIX/previous"
  exit 0
fi

need_bundle
[[ -n $PREFIX ]] || die "set PREFIX to the installation directory (absolute, outside this repository)"
if [[ $APPLY == 0 ]]; then
  plan
  if [[ -z $BACKUP || ! -s $BACKUP ]]; then
    echo "upgrade: refused to apply until BACKUP is a non-empty database dump" >&2
    exit 1
  fi
  exit 0
fi

PREFIX=$(abs_dir "$PREFIX" PREFIX)
case $PREFIX/ in "$ROOT"/*) die "PREFIX is inside the repository" ;; esac
[[ -n $BACKUP && -s $BACKUP ]] || die "BACKUP must be a non-empty database dump taken before this upgrade"
[[ $BACKUP == /* ]] || die "BACKUP must be an absolute path"

if [[ -f $PREFIX/bin/apelora-control-plane ]]; then
  copy_tree "$PREFIX" "$PREFIX/previous"
fi
copy_tree "$BUNDLE" "$PREFIX"
{
  echo "version: $version"
  echo "applied: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "backup: $(basename "$BACKUP")"
} >"$PREFIX/upgrade-record"
echo "upgrade: installed $version under $PREFIX (previous copies in $PREFIX/previous when they existed)"

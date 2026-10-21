#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Checks an Apelora release bundle offline (ADR 0010): the signature on SHA256SUMS against an
# allowed-signers file, then every file's checksum, and that the bundle holds no file the
# checksums do not list. Changes nothing outside a temporary directory.
#
#   scripts/release/verify.sh apelora-<version>.tar.gz allowed_signers
#   scripts/release/verify.sh dist/apelora-<version>/ allowed_signers
#
# allowed_signers holds one line per trusted release key, in OpenSSH's format:
#   release@your-organisation namespaces="apelora-release" ssh-ed25519 AAAA...
# Exit status 0 only when everything checks.
set -euo pipefail

if [[ ${BASH_VERSINFO[0]} -lt 4 && -x /opt/homebrew/bin/bash && -z ${APELORA_REEXEC:-} ]]; then
  APELORA_REEXEC=1 exec /opt/homebrew/bin/bash "$0" "$@"
fi

NAMESPACE=apelora-release
fail() { echo "NOT VERIFIED: $*" >&2; exit 1; }
[[ $# -eq 2 ]] || { echo "usage: $0 <bundle.tar.gz | bundle directory> <allowed_signers>" >&2; exit 2; }
BUNDLE=$1
SIGNERS=$2
[[ -r $SIGNERS ]] || fail "cannot read $SIGNERS"
command -v ssh-keygen >/dev/null || fail "ssh-keygen is needed"
if command -v sha256sum >/dev/null; then sum() { sha256sum "$@"; }
elif command -v shasum >/dev/null; then sum() { shasum -a 256 "$@"; }
else fail "sha256sum or shasum is needed"; fi

TMP=
# (An `&&` here would make the script exit 1 when there is no temporary directory.)
cleanup() { if [[ -n $TMP ]]; then rm -rf "$TMP"; fi; }
trap cleanup EXIT

if [[ -f $BUNDLE ]]; then
  # An archive from outside is untrusted: only plain files and directories under one top
  # directory, no absolute paths and no "..", before anything is extracted.
  listing=$(tar -tvzf "$BUNDLE") || fail "$BUNDLE is not a readable .tar.gz"
  if grep -qvE '^[-d]' <<< "$listing"; then fail "the archive holds links or special files"; fi
  names=$(tar -tzf "$BUNDLE")
  if grep -qE '(^/|(^|/)\.\.(/|$))' <<< "$names"; then fail "the archive holds unsafe paths"; fi
  top=$(sed 's|/.*||' <<< "$names" | sort -u)
  [[ $(wc -l <<< "$top") -eq 1 && $top =~ ^apelora-[A-Za-z0-9._+-]+$ ]] || fail "the archive must hold one apelora-<version>/ directory"
  TMP=$(mktemp -d)
  tar -xzf "$BUNDLE" -C "$TMP" --no-same-owner
  DIR=$TMP/$top
elif [[ -d $BUNDLE ]]; then
  DIR=$(cd "$BUNDLE" && pwd -P)
else
  fail "$BUNDLE does not exist"
fi
cd "$DIR"
[[ -f SHA256SUMS ]] || fail "SHA256SUMS is missing"
[[ -f SHA256SUMS.sig ]] || fail "SHA256SUMS.sig is missing (an unsigned bundle is never accepted)"

# 1. Who signed it, and is that key allowed to sign Apelora releases?
principal=$(ssh-keygen -Y find-principals -s SHA256SUMS.sig -f "$SIGNERS" 2>/dev/null | head -1) ||
  fail "SHA256SUMS is signed by a key that is not in $SIGNERS"
[[ -n $principal ]] || fail "SHA256SUMS is signed by a key that is not in $SIGNERS"
ssh-keygen -Y verify -f "$SIGNERS" -I "$principal" -n "$NAMESPACE" -s SHA256SUMS.sig < SHA256SUMS >/dev/null 2>&1 ||
  fail "the signature on SHA256SUMS does not match (changed after signing, or not signed for $NAMESPACE)"

# 2. Every listed file has its checksum; the list names only safe relative paths.
n=0
listed=$(mktemp)
while IFS= read -r line; do
  [[ $line =~ ^([0-9a-f]{64})\ [\ *](.+)$ ]] || fail "SHA256SUMS has a malformed line: $line"
  want=${BASH_REMATCH[1]}
  file=${BASH_REMATCH[2]}
  [[ $file != /* && ! $file =~ (^|/)\.\.(/|$) ]] || fail "SHA256SUMS names an unsafe path: $file"
  [[ -f $file && ! -L $file ]] || fail "$file is missing"
  got=$(sum "$file" | cut -d' ' -f1)
  [[ $got == "$want" ]] || fail "$file does not match its checksum"
  echo "$file" >> "$listed"
  n=$((n + 1))
done < SHA256SUMS

# 3. Nothing else is in the bundle.
extra=$(find . \( -type f -o -type l \) ! -name SHA256SUMS ! -name SHA256SUMS.sig | sed 's|^\./||' | LC_ALL=C sort | comm -23 - <(LC_ALL=C sort "$listed"))
rm -f "$listed"
[[ -z $extra ]] || fail "files not listed in SHA256SUMS: $(tr '\n' ' ' <<< "$extra")"

echo "VERIFIED: $n files match SHA256SUMS, signed by $principal ($NAMESPACE)"

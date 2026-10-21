#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Builds an offline release bundle (ADR 0010): the two services, the two web apps, database
# migrations, documentation, licence notices, a software inventory, SHA256SUMS and a signature
# of SHA256SUMS. The result is carried into an installation without internet access, checked
# with scripts/release/verify.sh and installed (docs/runbooks/offline-update.md).
#
#   APELORA_RELEASE_SIGNING_KEY=~/.keys/apelora-release scripts/release/bundle.sh
#   scripts/release/bundle.sh --unsigned      # a test bundle: verify.sh refuses it
#
# Environment:
#   APELORA_RELEASE_SIGNING_KEY  private SSH key that signs SHA256SUMS (ssh-keygen -Y sign).
#                                Kept outside the repository; never generated here.
#   APELORA_RELEASE_VERSION      bundle version (default: git describe of the checkout)
#   APELORA_RELEASE_OUT          output directory (default: dist/, which git ignores)
#
# Builds for the machine it runs on: run it on (or for) the architecture you install on.
# Creates no tag and publishes nothing.
set -euo pipefail

# macOS ships bash 3.2; prefer a newer bash when one is installed.
if [[ ${BASH_VERSINFO[0]} -lt 4 && -x /opt/homebrew/bin/bash && -z ${APELORA_REEXEC:-} ]]; then
  APELORA_REEXEC=1 exec /opt/homebrew/bin/bash "$0" "$@"
fi

die() { echo "bundle: $*" >&2; exit 1; }
NAMESPACE=apelora-release
UNSIGNED=0
for arg in "$@"; do
  case $arg in
    --unsigned) UNSIGNED=1 ;;
    -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
    *) die "unknown argument $arg (see --help)" ;;
  esac
done

ROOT=$(cd "$(dirname "$0")/../.." && pwd -P)
cd "$ROOT"
for tool in cargo npm node tar ssh-keygen; do
  command -v "$tool" >/dev/null || die "$tool is needed"
done
if command -v sha256sum >/dev/null; then sum() { sha256sum "$@"; }
elif command -v shasum >/dev/null; then sum() { shasum -a 256 "$@"; }
else die "sha256sum or shasum is needed"; fi

KEY=${APELORA_RELEASE_SIGNING_KEY:-}
if [[ $UNSIGNED -eq 0 ]]; then
  [[ -n $KEY ]] || die "set APELORA_RELEASE_SIGNING_KEY to your signing key's path (or pass --unsigned for a test bundle)"
  [[ -r $KEY ]] || die "cannot read the signing key $KEY"
  key_dir=$(cd "$(dirname "$KEY")" && pwd -P)
  case $key_dir/ in "$ROOT"/*) die "the signing key is inside the repository; keep it outside" ;; esac
fi

VERSION=${APELORA_RELEASE_VERSION:-$(git describe --tags --always --dirty 2>/dev/null || echo unversioned)}
[[ $VERSION =~ ^[A-Za-z0-9._+-]+$ ]] || die "version $VERSION: letters, digits, . _ + - only"
OUT=${APELORA_RELEASE_OUT:-$ROOT/dist}
NAME=apelora-$VERSION
DIR=$OUT/$NAME
[[ -e $DIR || -e $OUT/$NAME.tar.gz ]] && die "$DIR or its archive exists already; remove it or choose another version"
mkdir -p "$DIR"/{bin,web,migrations,notices,inventory}

echo "== Building $NAME"
cargo build --release --locked -p apelora-control-plane -p apelora-deployment-worker
cp target/release/apelora-control-plane target/release/apelora-deployment-worker "$DIR/bin/"
npm run build
cp -R apps/studio/dist "$DIR/web/studio"
cp -R apps/fabric-console/dist "$DIR/web/console"

echo "== Collecting migrations, documentation and notices"
# The services embed and run these on start; the copies are for review.
cp crates/persistence/migrations/*.sql "$DIR/migrations/"
cp -R docs "$DIR/docs"
cp LICENSE COPYRIGHT.md TRADEMARKS.md SECURITY.md README.md upstream.lock.yaml "$DIR/"
cp packages/module-catalog/assets/logos/ATTRIBUTION.md "$DIR/notices/logos-ATTRIBUTION.md"
node scripts/release/inventory.mjs "$DIR/inventory"
{
  echo "version: $VERSION"
  echo "commit: $(git rev-parse HEAD 2>/dev/null || echo unknown)"
  echo "built: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "host: $(rustc -vV | sed -n 's/^host: //p')"
  echo "rustc: $(rustc --version)"
  echo "node: $(node --version)"
} > "$DIR/BUILD-INFO"

echo "== Checksums"
(
  cd "$DIR"
  find . -type f | sed 's|^\./||' | LC_ALL=C sort |
    while IFS= read -r f; do sum "$f"; done > "$OUT/.$NAME.sums"
)
mv "$OUT/.$NAME.sums" "$DIR/SHA256SUMS"
echo "  $(wc -l < "$DIR/SHA256SUMS" | tr -d ' ') files"

if [[ $UNSIGNED -eq 0 ]]; then
  echo "== Signing SHA256SUMS ($NAMESPACE)"
  ssh-keygen -Y sign -f "$KEY" -n "$NAMESPACE" "$DIR/SHA256SUMS" >/dev/null
else
  echo "== Not signed (--unsigned): verify.sh will refuse this bundle"
fi

tar -C "$OUT" -czf "$OUT/$NAME.tar.gz" "$NAME"
echo "== Done: $OUT/$NAME.tar.gz"
sum "$OUT/$NAME.tar.gz"
echo "Check it with: scripts/release/verify.sh $OUT/$NAME.tar.gz <allowed_signers>"

#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Rotates one of Apelora's Incus identities on the disposable test cluster without a gap:
# a new key and certificate are trusted (with the same restrictions), checked against the
# Incus API, put in place under the same file names, and only then is the old one removed
# from the trust store. Restart the control plane and the worker afterwards (they read the
# certificate at start). See docs/runbooks/certificates.md.
#   bash scripts/test-cluster/rotate-identity.sh client   # the restricted deploy identity
#   bash scripts/test-cluster/rotate-identity.sh admin    # the unrestricted admin identity
set -euo pipefail

stem=${1:?usage: rotate-identity.sh client|admin}
[[ $stem == client || $stem == admin ]] || { echo "client or admin" >&2; exit 1; }
VM=${APELORA_TEST_VM:-apelora-incus}
DIR=${APELORA_TEST_DIR:-$HOME/.config/apelora/test-cluster}
crt="$DIR/$stem.crt" key="$DIR/$stem.key"
[ -s "$crt" ] && [ -s "$key" ] || { echo "no $stem identity in $DIR; run up.sh" >&2; exit 1; }
# shellcheck disable=SC1091
source "$DIR/env"
umask 077

incus() { limactl shell --workdir / "$VM" sudo incus "$@" </dev/null; }
fingerprint_of() { openssl x509 -in "$1" -noout -fingerprint -sha256 | cut -d= -f2 | tr -d ':' | tr 'A-F' 'a-f'; }

old_fp=$(fingerprint_of "$crt")
entry=$(incus query "/1.0/certificates/$old_fp")
name=$(jq -r .name <<<"$entry")
restricted=$(jq -r .restricted <<<"$entry")
projects=$(jq -c '.projects // []' <<<"$entry")

openssl ecparam -name secp384r1 -genkey -noout | openssl pkcs8 -topk8 -nocrypt -out "$key.new"
openssl req -new -x509 -key "$key.new" -subj "/CN=$name" -days 90 -out "$crt.new" \
  -config <(printf '[req]\ndistinguished_name=dn\nx509_extensions=v3\n[dn]\n[v3]\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=clientAuth\n')
new_fp=$(fingerprint_of "$crt.new")

# Trust the new certificate with the same restrictions as the old one.
limactl copy "$crt.new" "$VM:/tmp/apelora-$stem-new.crt"
args=(--name "$name")
if [ "$restricted" = true ]; then
  args+=(--restricted --projects "$(jq -r 'join(",")' <<<"$projects")")
fi
incus config trust add-certificate "/tmp/apelora-$stem-new.crt" "${args[@]}"
limactl shell --workdir / "$VM" rm -f "/tmp/apelora-$stem-new.crt"

# The new identity must work before the old one goes away.
host=${APELORA_INCUS_URL#https://}
check() {
  curl -s -m 20 --cert "$crt.new" --key "$key.new" --cacert "$APELORA_INCUS_SERVER_CERT" \
    --resolve "${APELORA_INCUS_TLS_SERVER_NAME:-${host%:*}}:${host##*:}:${host%:*}" \
    "https://${APELORA_INCUS_TLS_SERVER_NAME:-${host%:*}}:${host##*:}$1"
}
[ "$(check /1.0 | jq -r .metadata.auth)" = trusted ] || { echo "the new certificate is not trusted; the old one stays" >&2; exit 1; }
if [ "$restricted" = true ]; then
  for p in $(jq -r '.[]' <<<"$projects"); do
    [ "$(check "/1.0/projects/$p" | jq -r .status_code)" = 200 ] || { echo "the new certificate cannot use project $p; the old one stays" >&2; exit 1; }
  done
fi

mv "$crt" "$crt.old"; mv "$key" "$key.old"
mv "$crt.new" "$crt"; mv "$key.new" "$key"
incus config trust remove "$old_fp"
rm -f "$crt.old" "$key.old"
echo "rotated $stem identity $name: ${old_fp:0:12} → ${new_fp:0:12}; restart the control plane and the worker"

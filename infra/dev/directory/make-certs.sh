#!/opt/homebrew/bin/bash
# SPDX-License-Identifier: AGPL-3.0-only
# Makes a throwaway certificate authority and a server certificate for the development directory
# in infra/dev/directory/certs/ (git-ignored; never commit these keys).
#
# The server certificate names localhost and apelora-dev-directory, deliberately not 127.0.0.1:
# connect to ldaps://localhost:1636, and use ldaps://127.0.0.1:1636 to see a host-name mismatch.
#
#   bash infra/dev/directory/make-certs.sh          # once; again with --force to replace them
set -euo pipefail

dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/certs"
if [[ -e "$dir/ca.pem" && "${1:-}" != "--force" ]]; then
  echo "certificates exist in $dir (use --force to replace them)"
  exit 0
fi
mkdir -p "$dir"
chmod 0700 "$dir"
cd "$dir"
rm -f ca.pem ca.key server.pem server.key server.csr ca.srl ext.cnf

openssl ecparam -name prime256v1 -genkey -noout -out ca.key
openssl req -x509 -new -key ca.key -sha256 -days 825 -subj "/CN=Apelora development directory CA" \
  -addext "basicConstraints=critical,CA:true" -addext "keyUsage=critical,keyCertSign,cRLSign" -out ca.pem

openssl ecparam -name prime256v1 -genkey -noout -out server.key
openssl req -new -key server.key -subj "/CN=localhost" -out server.csr
cat > ext.cnf <<'EOF'
basicConstraints=critical,CA:false
keyUsage=critical,digitalSignature,keyEncipherment
extendedKeyUsage=serverAuth
subjectAltName=DNS:localhost,DNS:apelora-dev-directory
EOF
openssl x509 -req -in server.csr -CA ca.pem -CAkey ca.key -CAcreateserial -days 825 -sha256 \
  -extfile ext.cnf -out server.pem
rm -f server.csr ca.srl ext.cnf
# Readable by the container's slapd user (it copies them and tightens the key's permissions).
chmod 0644 ca.pem server.pem server.key
chmod 0600 ca.key
echo "made $dir/ca.pem (trust this one in Apelora) and the server certificate"

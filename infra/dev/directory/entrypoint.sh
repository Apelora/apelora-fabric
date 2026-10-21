#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-only
# Starts the development directory: copies the TLS files, loads the seed into an empty database,
# then runs slapd in the foreground. Development only.
set -eu

conf=/etc/apelora-directory
data=/var/lib/openldap/apelora

for f in ca.pem server.pem server.key; do
  if [ ! -s "/certs/$f" ]; then
    echo "missing /certs/$f: run infra/dev/directory/make-certs.sh first" >&2
    exit 1
  fi
done
mkdir -p "$conf/tls" /run/openldap "$data"
cp /certs/ca.pem /certs/server.pem /certs/server.key "$conf/tls/"
chown -R ldap:ldap "$conf/tls" /run/openldap "$data"
chmod 0600 "$conf/tls/server.key"

if [ ! -e "$data/data.mdb" ]; then
  # The development password, hashed (salted SHA-1, what this OpenLDAP build offers by default).
  hash=$(slappasswd -s apelora-dev-only)
  sed "s|@PASSWORD@|$hash|" "$conf/seed.ldif" > /tmp/seed.ldif
  slapadd -f "$conf/slapd.conf" -l /tmp/seed.ldif
  rm -f /tmp/seed.ldif
  chown -R ldap:ldap "$data"
  echo "development directory seeded"
fi

exec slapd -d "${SLAPD_DEBUG:-256}" -u ldap -g ldap -f "$conf/slapd.conf" -h "ldap:/// ldaps:///"

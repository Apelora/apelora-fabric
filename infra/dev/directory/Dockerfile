# SPDX-License-Identifier: AGPL-3.0-only
# Local LDAP directory for trying directory sign-in. Development only; see seed.ldif for the
# people and the development password.
#
# OpenLDAP (OpenLDAP Public License 2.8) from Alpine Linux's maintained packages, on the official
# Alpine image (MIT). It runs as a separate program; nothing of it is linked into Apelora.
FROM alpine:3.24
RUN apk add --no-cache openldap openldap-back-mdb openldap-clients openldap-overlay-memberof
COPY slapd.conf seed.ldif entrypoint.sh /etc/apelora-directory/
RUN chmod 0755 /etc/apelora-directory/entrypoint.sh
EXPOSE 389 636
ENTRYPOINT ["/etc/apelora-directory/entrypoint.sh"]

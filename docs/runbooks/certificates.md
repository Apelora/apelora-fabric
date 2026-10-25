# Rotate Apelora's Incus certificates

Use this page before a certificate expires, and at once if a key may have leaked.

Apelora talks to Incus with two client certificates (ADR 0003):

- the **deploy identity**, restricted to the deploy projects, used by the control plane and the worker;
- the **admin identity**, unrestricted, used by the control plane only for cluster administration.

Certificates made as in [install Incus](../guides/install-incus.md) last 90 days. Rotate them well
before they expire. Fabric Console → Administration → System shows when each expires
(`GET /api/v1/system`, `certificates`) and warns 30 days ahead.

Rotation never leaves Apelora without access:

1. Create a new key and a certificate with the same name (`CN`), key usage `clientAuth`,
   X.509 v3. Keep the key outside the repository with owner-only permissions.
2. Trust it with the **same restrictions** as the current one: for the deploy identity,
   `incus config trust add-certificate <file> --name <name> --restricted --projects <projects>`;
   for the admin identity, without `--restricted`.
3. Check the new certificate before removing anything: `GET /1.0` reports `auth: trusted`;
   for the deploy identity, `GET /1.0/projects/<project>` answers 200 for each deploy project.
4. Put the new key and certificate where `APELORA_INCUS_CLIENT_CERT/KEY` (or the admin
   variables) point, then restart the control plane and the worker.
5. Remove the old certificate from the trust store: `incus config trust remove <fingerprint>`.
6. Check: Preflight passes; the Fabric Console lists instances; the audit log shows normal
   activity.

A certificate that may have leaked: rotate as above, at once. The old one stops working the
moment it leaves the trust store (step 5).

# Rotate Apelora's Incus certificates

**Evidence:** REAL INCUS TESTED, 2026-10-20 — both identities rotated on the three-member test
cluster with `rotate-identity.sh`; the old certificates were gone from the trust store, the
deploy identity kept its project restriction, and the real Incus tests passed with the new ones.

Apelora talks to Incus with two client certificates (ADR 0003): the **deploy identity**,
restricted to the deploy projects (control plane and worker), and the **admin identity**,
unrestricted (control plane only, cluster administration). The test cluster issues them for 90
days; rotate them well before they expire, and at once if a key may have leaked.

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

On the test cluster, `scripts/test-cluster/rotate-identity.sh client|admin` does steps 1–5
(stopping before step 5 if the check in step 3 fails).

A certificate that may have leaked: rotate as above, at once; the old one stops working the
moment it leaves the trust store (step 5).

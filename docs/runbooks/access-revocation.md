# Revoke access

| What | How | Evidence |
|---|---|---|
| A person's or a script's API token | Fabric Console → Administration → *Revoke*, or `DELETE /api/v1/tokens/<id>`. Takes effect on the next request; audited (`token.revoke`). | REAL INCUS TESTED (control plane against the test cluster), 2026-10-20 |
| An unused cluster join token | Membership → *Revoke*, or `DELETE /api/v1/cluster/join-tokens/<member>`. | REAL INCUS TESTED, 2026-10-20 |
| Apelora's Incus identities (deploy or admin certificate) | Rotate them: [certificates.md](certificates.md). | REAL INCUS TESTED, 2026-10-20 |
| A server that left the cluster | See member-failure.md (*A member that will not return*). | NOT YET VERIFIED |

After any revocation, check the audit log for actions by the revoked actor since the time of
concern.

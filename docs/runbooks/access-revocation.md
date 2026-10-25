# Revoke access

Use this page to cut off a person, a script, a join token, Apelora's own Incus identities or a server.

| What | How |
|---|---|
| A person's or a script's API token | Fabric Console → Administration → *Revoke*, or `DELETE /api/v1/tokens/<id>`. Takes effect on the next request; audited (`token.revoke`). |
| An unused cluster join token | Servers → Machines and join tokens → *Revoke*, or `DELETE /api/v1/cluster/join-tokens/<member>`. |
| Apelora's Incus identities (deploy or admin certificate) | Rotate them: [certificates.md](certificates.md). |
| A server that left the cluster | See [member-failure.md](member-failure.md) (*A member that will not return*). |

After any revocation, check the audit log for actions by the revoked actor since the time of
concern.

For a stolen credential or a misused account, follow [a security incident](security-incident.md).

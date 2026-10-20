# Runbooks

Step-by-step procedures for operating an Apelora fabric. Every procedure states how far it has
been verified:

- **REAL INCUS TESTED** — carried out on the disposable test cluster (docs/development/TEST-CLUSTER.md),
  with the date and cluster size.
- **NOT YET VERIFIED** — written from the Incus documentation and our design; follow it with care
  and record the result here when it has been done.

Apelora never writes Incus's database and never moves instances itself: membership, placement,
evacuation and migration are Incus operations that Apelora requests, previews and audits.

| Situation | Runbook |
|---|---|
| Add a server to the cluster | [member-join.md](member-join.md) |
| Maintenance on a server | [member-maintenance.md](member-maintenance.md) |
| A server fails | [member-failure.md](member-failure.md) |
| The cluster loses quorum | [quorum-loss.md](quorum-loss.md) |
| A replica fails its health check | [replica-unhealthy.md](replica-unhealthy.md) |
| The deploy identity cannot see its project | [deploy-identity.md](deploy-identity.md) |
| Revoke access (people, automation, servers) | [access-revocation.md](access-revocation.md) |
| The control plane or worker is down | [api-outage.md](api-outage.md) |
| Rotate Apelora's Incus certificates | [certificates.md](certificates.md) |

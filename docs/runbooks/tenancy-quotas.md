# Tenancy: environments' Incus projects and quotas

Use this page to raise a quota, and to fix an environment whose Incus project is not prepared. Design: [tenancy isolation](../design/tenancy-isolation.md), [ADR 0012](../adr/0012-tenancy-on-incus-projects.md).

Each environment deploys into its own restricted Incus project (`<organisation>-<project>-<environment>`). Apelora creates it when the environment is created, with the administrator identity (`APELORA_INCUS_ADMIN_CLIENT_CERT`), and adds it to the deploy identity's project list. Quotas cap organisations, projects and environments; a deployment over a quota is refused before anything is saved.

## Raise a quota

Someone's deployment was refused with `409 quota_exceeded`: "nothing was deployed; project Shop: 14 CPUs in use, this adds 4, quota 16".

1. **Find which quota.** The message names the scope (organisation, project or environment), the dimension, what is in use and the limit. In the Console: Identity & Access → Tenancy & quotas shows each scope's use against its quota.
2. **Decide who raises it.**
   - An organisation's quota: a platform administrator with `quota.manage` (Fabric Administrator, Platform Super Admin).
   - A project's quota: the organisation owner, within the organisation's quota; or `quota.manage`.
   - An environment's quota: the project administrator or organisation owner, within the quotas above; or `quota.manage`.
   - If the quota above is the limit, raise that one first: a tenant's change above it is refused with `400 quota_above_parent`.
3. **Check the servers have room.** A quota is a promise, not capacity: compare with Health → servers (CPU, memory, storage) before raising an organisation's quota beyond what the cluster has.
4. **Change it.** Tenancy & quotas → *Set quota…* on the scope; or the API:

   ```sh
   curl -X PUT -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
     https://apelora.example/api/v1/identity/quotas/project/<project-id> \
     -d '{"instances": 20, "cpu": 24, "memoryGib": 64, "diskGib": 500}'
   ```

   An empty field (or `null`) means no limit in that dimension; `{}` removes the quota.
5. **Read the result.** The response (and the Console's notice) says, per environment beneath the scope, whether Incus has the new limits (`applied`, `unchanged`), was not asked (`skipped`: the project is not prepared yet, or there is no admin identity) or refused (`failed`, with Incus's message). The change is audited as `quota.set`; each mirror as `environment.incus_limits.apply`.
6. **Deploy again.** The plan preview (Studio → Applications → Deploy…) shows quota use after the deployment.

Lowering a quota below what is in use is allowed in Apelora (it stops new deployments) but Incus refuses a project limit below use ("limits.cpu is too low"): the environment shows `failed` until enough is removed; then *Set quota…* again (or any later quota change) mirrors it.

## An environment's Incus project is "Not prepared"

The Tenancy page says why. Common reasons:

| Reason | Do this |
|---|---|
| No administrator identity is configured | Configure `APELORA_INCUS_ADMIN_CLIENT_CERT`/`KEY` on the control plane (never the worker), restart it, then *Prepare Incus project* |
| `Incus: …` (unreachable, refused) | Fix Incus reachability ([api-outage](api-outage.md)), then *Prepare Incus project*; the first deployment also retries |
| The project exists but is not restricted | Apelora adopts only restricted projects. On a member: `incus project set <name> restricted=true` (and review its `restricted.*` keys), then *Prepare Incus project* |
| The deploy identity cannot use it | See [deploy-identity](deploy-identity.md); *Prepare Incus project* re-adds the grant |
| A platform deploy project (`APELORA_DEPLOY_PROJECTS`) | Set up by the operator; *Prepare Incus project* checks it and adopts it as it is |

*Prepare Incus project* is `POST /api/v1/identity/environments/{id}/incus-project` (`environment.manage`); it is safe to repeat.

## Check what Incus has

```sh
incus project show <organisation>-<project>-<environment>     # restricted.*, features.*, limits.*
incus query /1.0/projects/<name>/state                        # use and limits per resource
incus config trust show <deploy-fingerprint>                  # projects: includes the environment's
```

Never make the deploy identity unrestricted to work around a missing grant (ADR 0003).

## Before you rely on it

- The Incus side of this page (the restricted project, its profile, the grant, the limits Incus
  enforces) has run on a lab cluster. The Console and API steps have been tested with simulated
  data only. Raise a quota on a test environment first.

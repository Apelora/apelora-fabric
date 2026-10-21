# ADR 0012 — Environments own restricted Incus projects that Apelora prepares; quotas are Apelora's, mirrored as Incus limits

Status: **accepted; built (first slice)** · Date: 2026-10-21 · Design: [tenancy isolation and quotas](../design/tenancy-isolation.md) · Related: [ADR 0003](0003-incus-identities-and-cluster-join.md), [ADR 0005](0005-access-model.md)

## Context

ADR 0005 decided that every environment maps to one restricted Incus project. Nothing created those projects: an administrator made them by hand and added each to the deploy identity's project list. There were no quotas. Incus can restrict a project (`restricted.*`), cap it (`limits.*`) and limit a client certificate to a list of projects, but a restricted certificate may not create or edit projects, or change any certificate's project list: those need the unrestricted admin identity, which only the control plane holds (ADR 0003).

Quotas must work across environments (a project or an organisation has several), while an Incus limit applies to one Incus project.

## Decision

1. **Apelora prepares the environment's Incus project.** The control plane, with the admin identity, creates it (or adopts an existing restricted one) when the environment is created, on request, or at its first deployment; gives it its own `default` profile; and adds it to the deploy identity's project list. The project name stays derived from the organisation, project and environment identifiers. The admin identity's use grows from cluster membership to these three tenancy calls; it still never reaches the worker, and nothing that carries it is queued.
2. **A fixed baseline** of restrictions for projects Apelora creates (design §2.1). Adopted projects keep their own restrictions; an unrestricted project is never adopted.
3. **Quotas live in Apelora** on organisations, projects and environments (instances, CPUs, memory, disk) and are checked when a deployment is planned, using Incus's own per-project usage. A deployment that would exceed one is refused before anything is saved or queued.
4. **Incus enforces the per-environment ceiling**: each environment's Incus project gets, per dimension, the tightest quota of the environment, its project and its organisation. Sums across environments are Apelora's to enforce.
5. **Tenants divide, platform administrators grant.** `quota.manage` (infrastructure, platform) sets any quota; organisation owners and project administrators set quotas below their scope, never above a quota that bounds them.

## Consequences

- A new environment is usable at once, without an administrator on the Incus command line, and its isolation no longer depends on someone remembering the right keys.
- Losing the admin identity's configuration does not stop deployments into existing environments; new ones stay *not prepared* until it is back.
- The deploy identity's project list grows with every environment. Incus removes a deleted project from that list itself.
- Deleting an environment does not delete its Incus project yet (P2).
- Changing the derived naming or the 1:1 mapping later would mean moving instances between Incus projects: this is the hard-to-reverse part.

## Alternatives

| Option | Why not |
|---|---|
| One Incus project per Apelora project (environments share it) | Limits and restrictions could not differ between production and development; production isolation is the point of environments |
| One Incus project per organisation | Same, coarser; one team's test load could starve its production |
| Mirror only, no Apelora check | Incus cannot sum across projects, so organisation and project quotas would not be enforced; and a refusal would surface as a failed job after the deployment was saved |
| Check only, no mirror | A bug or a direct API caller could exceed a quota; Incus as a second line costs nothing |
| Give the worker the admin identity and do this as a job | Breaks ADR 0003: the most-used component would hold full cluster control |

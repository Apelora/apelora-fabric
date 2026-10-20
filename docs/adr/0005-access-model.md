# ADR 0005 — Access model: roles bound to scopes, applications owned by projects

Status: **proposed** · Date: 2026-10-20 · Related: [ADR 0001](0001-incus-first.md), [ADR 0003](0003-incus-identities-and-cluster-join.md), [access and roles](../architecture/ACCESS-AND-ROLES.md), [applications view](../design/applications-view.md)

## Context

Four roles and six permissions exist, applied platform-wide, with deployments limited by an allow-list of Incus projects. Applications are created implicitly at first deploy and have no owner or project of their own. The Studio is about to gain an Applications view with server-side drafts, which makes "who may see and change which application" a product question, not only an infrastructure one.

## Decision

1. **Roles stay few and meaningful:** `fabric-admin`, `operator`, `studio-designer`, `auditor`, plus a read-only `viewer`. The role-to-permission table stays in one place (`crates/domain`).
2. **Roles are granted through bindings** `(identity, role, scope)`, with scope = platform, project or application. A binding applies to everything beneath its scope. Bindings live in PostgreSQL and are managed and audited by administrators.
3. **Every application belongs to exactly one Apelora project, which maps to one restricted Incus project.** Apelora's authorisation and Incus's isolation line up.
4. **Authorisation is enforced on the server for every route;** UIs only reflect it. An identity that may not see an object gets 404 for it, not 403.
5. **Apelora and Incus authorities stay separate.** Humans never receive Incus credentials. The worker uses the restricted deploy identity; the unrestricted admin identity is used only for cluster administration.
6. **Humans sign in with OIDC** when it is built (group → role binding); API tokens remain for automation, scoped, short-lived and owned.
7. **Direct Incus access is break-glass** for a few administrators, announced and reconciled.

## Consequences

- Teams can design and run their own applications without seeing others'; separation of duties is possible (designers design, operators deploy, optional approval later).
- The permission set grows (`read-applications`, `operate-instances`, `manage-storage`, `manage-networks`, `approve-deployment`); tests must prove that each role gets 403 on every route it must not use.
- Existing deployments need a migration that assigns current applications to a project and current tokens to bindings.
- Until OIDC exists, sign-in remains by API token held in the tab's `sessionStorage`.

## Alternatives considered

| Option | Why not |
|---|---|
| Keep platform-wide roles only | Cannot isolate teams; every designer sees every application |
| Delegate authorisation to Incus (certificates per user) | Gives people Incus credentials, cannot express application-level rules, and splits audit |
| Many fine-grained custom roles from day one | Hard to review and test; start with few roles and add permissions as needs appear |

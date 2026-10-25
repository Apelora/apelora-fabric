# ADR 0016 — A group mapped from outside Apelora names its source

Status: **accepted; built (API, Console, persistence)** · Date: 2026-10-25 · Amends [ADR 0005](0005-access-model.md) (groups) and [ADR 0007](0007-sign-in-methods.md) (directory groups) · Design: [identity and authorization](../architecture/IDENTITY-AND-AUTHORIZATION.md)

## Context

An Apelora group can take its members from outside: everyone whose sign-in carries a given group name is a member, and holds the group's roles. Until now the mapping was only that name. It was unique across all sources, and any credential that carried the name matched it, whichever identity provider or directory issued that credential. Directory groups were named by the first value of their DN (`cn=dev-admins,ou=groups,…` became `dev-admins`).

So whoever can create groups in the least protected source could create a group with the name of a privileged mapping and become a member of it: a second identity provider added for contractors, or a branch of the directory where people manage their own groups (`cn=platform-admins,ou=self-service,…` and `cn=platform-admins,ou=groups,…` were the same group to Apelora).

## Decision

1. **A mapping records its source** next to the external name: an OpenID Connect issuer (without a trailing slash, as tokens are matched), or `directory:<id>` for a saved directory. The name is unique per source, not across sources.
2. **A credential's groups match only mappings of its own source.** A provider's token matches mappings of its issuer; a directory sign-in records `directory:<id>` on the session and matches only that directory's mappings. Personal tokens keep matching Apelora groups only.
3. **Directory groups are named by their full DN**, in one spelling: RDNs split at unescaped commas, attribute types and values trimmed and lowercased, values unescaped and escaped again (RFC 4514), multi-valued RDNs sorted. Groups found by the group search are named by the DN of their entry, not by their name attribute.
4. **Creating a mapping needs the source**, which must be one this control plane knows (the environment's provider, a saved provider, or a saved directory); a directory mapping must be a DN. **Setting or changing a group's mapping** (`PUT /api/v1/identity/groups/{group}/external`) needs platform `group.manage` and passes the escalation guard for every role the group holds, as adding a member does: the source's members receive those roles.
5. **Existing mappings** (database migration 0033). They have no source and match nobody. At its first start after the migration, the control plane binds them to the only source when exactly one is configured and it is an identity provider; it is the only time it does so, and it is audited (`group.sources.backfill`). Otherwise they stay without a source: when a directory is the only source, their names are first RDN values and must be entered again as full DNs. The API marks such a group `needsSource`, and the Console shows *Needs a source* with a *Set source* action. Directory sessions issued before the migration have no source either; their groups count again after the next sign-in.

## Consequences

- A mapping can no longer be reached from another source, and a self-service branch of a directory cannot imitate a group in another branch.
- Installations with several sources must set the source of every existing mapping before its members get its roles again. Until then those people lose the access the mapping gave; nobody gains any.
- Directory mappings are longer to type: the full DN. The directory check in the Console shows each group's DN as Apelora will read it, and which saved mappings match.
- Alternatives considered: inferring the source from the person's `users.issuer` alone (a directory's URL is not unique to one saved directory, and a provider's issuer changes when the provider moves); keeping first RDN names and adding the base DN as a filter (still lets any branch below the base imitate a group); binding existing mappings to every configured source (would keep the hole open).

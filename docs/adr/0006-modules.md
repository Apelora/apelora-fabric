# ADR 0006 — The parts of an application are called modules

Status: **accepted; implemented** · Date: 2026-10-21 · Related: [component model](../architecture/COMPONENT-MODEL.md), [module ports](../architecture/MODULE-PORTS.md)

## Context

The reusable parts that people place on the Studio canvas (a gateway, NGINX, PostgreSQL) were called *appliances*. The owner asked for a different word. The new word must read naturally in the designer ("drag modules onto the canvas"), in the API and in operations, and must not clash with words the product already uses: *component* is a group of parts packaged as one, *service* names service addresses and service identities, *block* names block volumes.

## Decision

1. **The word is *module*** in the user interfaces, the documentation, the catalog, the API, the saved-design format and the code (`ModuleClass`, `DesiredModule`, `spec.modules`, `moduleId`, `@apelora/module-catalog`).
2. **Nothing already saved stops working:**
   - Saved designs, links, local drafts, saved versions and custom classes written with `appliances`, `applianceId` or `appliance` are read through `migrateLegacyKeys` (`@apelora/assembly-model`) and saved with the new keys from then on.
   - API requests and stored JSON (desired state, published ports, job payloads, operation steps) accept the old field names (`#[serde(alias = …)]`); responses use the new ones.
   - Incus objects get `user.apelora.module`; objects labelled earlier with `user.apelora.appliance` are still recognised (`apelora_domain::module_label`, and the placement scriptlet reads both).
   - Database columns are renamed by migration 0013; earlier migrations stay as they were applied.
3. **Records of the past keep their words.** Audit events written before the rename are not rewritten.

## Consequences

- One word everywhere: documentation, API and UI agree, and a search finds every use.
- Clients that read API responses must use the new field names; clients that only send requests keep working.
- The old label key is read for as long as instances created before the rename may exist. It can be dropped once every such instance has been recreated.

## Alternatives considered

- **Rename only what people see.** Faster, but the API, the saved format and the code would keep a second word, and the two would drift apart.
- **Rename without compatibility.** Simpler code, but exported designs and running instances created earlier would no longer be recognised.

---
name: appliance-schema
description: Define or change Apelora appliance manifests — typed ports, resource envelopes, architectures, and configuration properties. Use when adding or editing an appliance definition, the manifest JSON Schema, or the shared TypeScript/Rust types generated from it.
---

# Appliance schema

The manifest is the contract shared by Studio (TypeScript), Controller and Node (Rust), and the Catalog. Change it deliberately.

## Manifest shape

- `id` (stable, kebab-case), `version` (semver), `name`, `category`, `logo` (local SVG path)
- `architectures`: subset of `x86_64`, `aarch64`
- `image`: reference plus digest; never a mutable tag alone
- `resources`: `{ vcpus, memory_mib, disk_gib }` with `min` and `recommended`
- `ports[]`: `{ id, direction: in|out, protocol, type, cardinality, required }`
- `properties[]`: typed config (`string|int|bool|enum|secret`), default, validation, `secret` values never inlined
- `requires` / `conflicts`: other appliance ids or capabilities

## Rules

1. One JSON Schema file is the source of truth; generate TS types and Rust structs from it. Do not hand-edit generated code.
2. Port `type` is a namespaced identifier (e.g. `net.tcp/http`, `storage.volume/block`). Compatibility is defined in `topology-validation`, not here.
3. Additive changes bump minor; removals or meaning changes bump major and need a migration note.
4. Unknown fields are rejected, not ignored.
5. Add a valid and an invalid fixture for every new field.

## Checklist

- Schema updated, generated types regenerated
- Fixtures and schema tests pass
- Existing catalog entries still validate
- `docs/` updated if the shape changed

## Configuration properties

Classes may declare `properties` (typed fields: string, int, bool, enum, secret; default, required, min/max, options, group, description). Instances store only overrides in `config`. `secret` fields hold a reference name, never the secret. Property ids are stable identifiers; never derive them again from a renamed label. Validation lives in `packages/assembly-model/src/config.ts`.

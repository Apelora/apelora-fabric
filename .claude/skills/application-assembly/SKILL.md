---
name: application-assembly
description: Define the assembly document — JSON import/export, format versioning, migrations, and nested assemblies. Use when changing how applications are saved, loaded, shared, or composed from sub-assemblies.
---

# Application assembly

An assembly is the portable JSON description of an application: appliance instances, connections, layout, and parameters.

## Document

- `format_version` (integer, bumped on any breaking change) and `id`, `name`, `revision`
- `instances[]`: `{ id, appliance: {id, version}, config, position, size }`
- `connections[]`: `{ id, from: {instance, port}, to: {instance, port} }`
- `assemblies[]` / `subassembly` instances: nested assemblies expose chosen inner ports as their own ports (explicit `exports`)
- Layout (position/size) lives in a separate `layout` section so semantic diffs stay clean

## Rules

1. Import is untrusted input: validate against schema, cap size and nesting depth, reject duplicate ids and dangling references.
2. Every `format_version` change ships a pure migration function `vN -> vN+1` with fixture round-trips. Import migrates forward; export always writes current.
3. Serialization is deterministic (sorted keys, stable ids) so documents diff and hash cleanly.
4. Nested assemblies are expanded for validation; cycles in nesting are errors.
5. Secrets are never exported in clear text.

## Checklist

- Round-trip test: export -> import -> export is byte-identical
- Migration fixtures for each old version
- Malformed/oversized/hostile import tests

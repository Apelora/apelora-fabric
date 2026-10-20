---
name: topology-validation
description: Implement and test validation of application graphs — port compatibility, cardinality, dependency order, cycles, and host resource constraints. Use when changing connection rules or any check that decides whether an assembly is deployable.
---

# Topology validation

Validation must be one pure, deterministic implementation with identical results in Studio (TS, for instant feedback) and Controller (Rust, authoritative). Share test vectors between them; the Rust result wins.

## Checks (in order)

1. **Schema**: document and every referenced manifest valid.
2. **References**: module ids/versions exist in the catalog.
3. **Ports**: edge joins `out` to `in`; types compatible (exact match or declared adapter); protocol matches.
4. **Cardinality**: required ports connected; max connections respected.
5. **Dependencies**: `requires` satisfied, `conflicts` absent; dependency cycles reported with the cycle path.
6. **Resources**: sum of vCPU/memory/disk fits the target pool; architecture available for every module.
7. **Config**: required properties set; secrets referenced, not inlined.

## Output

A list of diagnostics `{ severity: error|warning, code, path, message }`. Stable `code`s, JSON-pointer `path`s, never throw for invalid user input.

## Checklist

- Each new rule has a passing and failing shared fixture
- Messages say what to change, not just what is wrong
- Property tests for graph operations (cycle detection, reachability)

# Using Xen with Apelora's license (proposal)

> **Superseded by [ADR 0001](adr/0001-incus-first.md).** Apelora no longer integrates with Xen; it uses Incus (Apache-2.0) over its REST API. Kept for history; see `license-compliance` for current guidance.

> Engineering analysis, **not legal advice**. Have counsel confirm before the first release or any commercial-license sale. License facts below should be re-verified against the Xen version you target.

## Goal

Keep Apelora's own code (Studio, Controller, Node, Catalog) under `AGPL-3.0-only` **and** offerable under a separate commercial license, while running on the Xen Project hypervisor.

## Relevant Xen licensing (verify per version)

| Component | License (typical) | Implication |
|---|---|---|
| Xen hypervisor | GPL-2.0-only (public headers under `include/public` carry a permissive exception) | Never copy hypervisor code into Apelora |
| `xl` CLI, some tools | GPL-2.0 | Run as a separate program only |
| `libxl`, `libxenctrl`, `libxenstore` etc. | LGPL-2.1 | Dynamic linking is possible, with LGPL obligations |
| Guest OS images | Their own licenses | Catalog must track per image |

Key problem: GPL-2.0-**only** code cannot be combined into a single work with AGPL-3.0 code. Dual-licensing also requires that Apelora's code contain nothing it cannot relicense.

## Recommended approach: layered boundaries

1. **Controller and Studio never touch Xen.** They stay 100% Apelora-owned and freely dual-licensable.
2. **Node agent talks to Xen through a `Hypervisor` trait** (see the `xen-integration` skill). Only one backend module knows about Xen.
3. **Tier 1 (default): process boundary.** The backend executes the `xl` binary with argv arrays and generated domain configs, and reads structured output. This is "mere aggregation / arm's-length communication", the lowest-risk path for GPL-2.0 tools. Xen is installed by the host operator, not bundled in Apelora releases.
4. **Tier 2 (optional, after counsel review): dynamic link to LGPL libraries** (`libxl`) via a thin FFI crate in a *separate, clearly labeled* module. Ship it so users can replace the library (dynamic linking, no static link), and keep LGPL notices. Do not link GPL-only libraries.
5. **Tier 3 (optional): out-of-process helper.** If a GPL-licensed helper is ever needed, ship it as its own GPL-licensed program, published with source, communicating over a socket or stdio with a documented protocol. Apelora code never imports it.
6. **Do not vendor Xen source** in this repository. Depend on the host's Xen packages. If you must reference public headers, use only files with the permissive exception and keep their notices.

## Distribution rules

- Do not bundle Xen binaries in Apelora packages unless you also provide corresponding source and notices; prefer documenting install of distro Xen packages.
- If shipping an appliance/ISO that includes Xen, treat that image as a separate GPL-compliant distribution with its own source offer.
- Commercial licensees get Apelora code under the commercial terms; Xen and LGPL parts remain under their own licenses, and the commercial agreement must say so explicitly (update `COMMERCIAL-AGREEMENT-DRAFT.md`).
- Keep an SBOM and per-release license report (CI check; see `license-compliance`).

## Trademarks

"Xen" and the Xen logo belong to The Linux Foundation / Xen Project. Use factual phrasing ("runs on the Xen Project hypervisor", "Xen-compatible"), never imply endorsement, and do not use the Xen logo without permission. Same for any vendor logos in the Catalog.

## Contributor rights

Dual licensing needs ownership or a broad license from every contributor: require a CLA or copyright assignment before merging outside code (see `CONTRIBUTING.md`). Reject patches copied from Xen/Linux sources.

## Action items

1. Pin the target Xen version and record each tool/library license used.
2. Implement the Node `Hypervisor` trait with a fake and an `xl`-subprocess backend (Tier 1) first.
3. Add a CI license check forbidding GPL/AGPL-incompatible crates and any `libxl` static linking.
4. Get counsel's written view on Tier 2 linking and on the commercial agreement wording.
5. Update README/`docs/ARCHITECTURE.md` to reference this strategy.

---
name: license-compliance
description: Check licensing questions for Apelora Fabric — AGPL-3.0-only community license, commercial licensing boundaries, Incus (Apache-2.0) and other upstream license compatibility, third-party dependencies and logos, and trademarks. Use when adding dependencies, vendoring code, accepting contributions, or touching license files. Not legal advice.
---

# License compliance

Read `LICENSE`, `COPYRIGHT.md`, `COMMERCIAL-LICENSE.md`, `TRADEMARKS.md`, and `CONTRIBUTING.md` first. This skill flags issues for human/legal review; it does not replace counsel.

## Model

- Community: `AGPL-3.0-only`. Network use triggers source-offer obligations.
- Commercial: separate negotiated license. Offering it **requires Apelora to hold sufficient rights in all code**, so no third-party code may enter unless it is compatible with relicensing or kept as a clearly separate component.
- Marks: names/logos are not licensed by the code license.

## Checks

1. **New dependency**: record license. Fine: MIT, BSD, Apache-2.0, ISC, MPL-2.0 (file-level). Needs review: GPL/LGPL/AGPL, SSPL, BUSL, "non-commercial", unknown. Apache-2.0 is compatible with GPLv3/AGPLv3 but not GPLv2-only.
2. **Incus**: Incus is Apache-2.0, which may be combined with AGPLv3 code (one-way: the combination is AGPLv3). Apelora talks to Incus over its REST API and does not vendor or fork it; keep upstream notices for anything we redistribute, never relicense upstream files, and record the pinned version in `upstream.lock.yaml`.
3. **Contributions**: outside code needs a CLA or copyright assignment before merge to preserve dual licensing.
4. **Headers**: new source files carry `SPDX-License-Identifier: AGPL-3.0-only`.
5. **Logos/trademarks**: third-party logos in the catalog are the owners' marks; use only with a basis (nominative use, permission) and record it. Do not use Incus or other marks to imply endorsement.
6. **Release gate**: `LICENSE` must be the complete, unmodified AGPLv3 text (`scripts/install-agpl-license.sh`).

## Automated checks (CI job "Dependencies")

- Rust: `cargo deny check` against `deny.toml` — licence allow-list (permissive only), RustSec advisories, no wildcard versions, crates.io only. Install locally with `brew install cargo-deny` or `cargo install --locked cargo-deny`.
- npm: `node scripts/licenses/check-npm.mjs` — the same allow-list for the production dependencies the apps ship (dev tools are not shipped); `npm audit --omit=dev --audit-level=high`.
- A failing licence is never fixed by extending the allow-list on the spot: record the licence, the reason and the maintainers' decision first.

## Output

State the license, the conflict or risk (if any), and the recommended action; escalate unresolved items to the maintainers.

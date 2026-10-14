---
name: license-compliance
description: Check licensing questions for Apelora Fabric — AGPL-3.0-only community license, commercial licensing boundaries, Xen/toolstack license compatibility, third-party dependencies and logos, and trademarks. Use when adding dependencies, vendoring code, accepting contributions, or touching license files. Not legal advice.
---

# License compliance

Read `LICENSE`, `COPYRIGHT.md`, `COMMERCIAL-LICENSE.md`, `TRADEMARKS.md`, and `CONTRIBUTING.md` first. This skill flags issues for human/legal review; it does not replace counsel.

## Model

- Community: `AGPL-3.0-only`. Network use triggers source-offer obligations.
- Commercial: separate negotiated license. Offering it **requires Apelora to hold sufficient rights in all code**, so no third-party code may enter unless it is compatible with relicensing or kept as a clearly separate component.
- Marks: names/logos are not licensed by the code license.

## Checks

1. **New dependency**: record license. Fine: MIT, BSD, Apache-2.0, ISC, MPL-2.0 (file-level). Needs review: GPL/LGPL/AGPL, SSPL, BUSL, "non-commercial", unknown. Apache-2.0 is compatible with GPLv3/AGPLv3 but not GPLv2-only.
2. **Xen**: Xen hypervisor is GPLv2 (with a special exception for guest-interface headers); libxl/toolstack are LGPL/GPL. GPLv2-only code is not combinable with AGPLv3 in one work, so keep Xen as a separate process (invoke `xl`, talk over a defined interface) unless counsel approves linking. Do not copy Xen code into the repo.
3. **Contributions**: outside code needs a CLA or copyright assignment before merge to preserve dual licensing.
4. **Headers**: new source files carry `SPDX-License-Identifier: AGPL-3.0-only`.
5. **Logos/trademarks**: third-party logos in the catalog are the owners' marks; use only with a basis (nominative use, permission) and record it. Do not use Xen or other marks to imply endorsement.
6. **Release gate**: `LICENSE` must be the complete, unmodified AGPLv3 text (`scripts/install-agpl-license.sh`).

## Output

State the license, the conflict or risk (if any), and the recommended action; escalate unresolved items to the maintainers.

<p align="center">
  <img src="assets/brand/logo.svg" alt="Apelora Fabric logo" width="96" height="96">
</p>

<h1 align="center">Apelora Fabric</h1>

**Distributed Application OS** — an open-source, Xen-based platform for composing, connecting, deploying, and operating distributed applications through reusable virtual appliances.

> Status: early-stage research and development. The orchestration system is not production-ready.

## Planned modules

- **Apelora Studio:** Vite, React, TypeScript, React Flow appliance editor.
- **Apelora Controller:** Rust application orchestration and scheduling.
- **Apelora Node:** Rust Xen host agent (integration licensing requires separate review).
- **Apelora Catalog:** Appliance definitions, ports, properties and resources.
- **Apelora Fabric:** virtual networking and application interconnections.

## Licensing

**Community:** GNU Affero General Public License version 3 only (`AGPL-3.0-only`). See [LICENSE](LICENSE) and [official license](https://www.gnu.org/licenses/agpl-3.0.html).

**Commercial:** A separate negotiated license may be available from the copyright owners. See [COMMERCIAL-LICENSE.md](COMMERCIAL-LICENSE.md). Commercial use is also permitted under AGPLv3 when its conditions are met; a commercial license is *not* mandatory merely because the user is a business.

**Marks:** Code licensing does not grant a right to use Apelora names or logos to imply official endorsement. See [TRADEMARKS.md](TRADEMARKS.md).

**Important:** Before releasing source code, replace the LICENSE placeholder with the *complete, unmodified* official AGPLv3 text (instructions below). Review third-party Xen component license compatibility and any contributor rights needed to support dual licensing.

## Preparing the license before publication

Run `bash scripts/install-agpl-license.sh`, inspect `LICENSE`, and commit the resulting full text. Do not publish a release until this step is complete.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). The initial contribution policy limits outside code contributions while commercial licensing rights are being designed.

# Runtime providers

Appliances run on **Incus** ([ADR 0001](../adr/0001-incus-first.md)). Other runtimes, **Xen** and **VMware**, are possible later, so the designer stays provider-neutral: an assembly describes *what* runs and how it is wired, never *how a particular runtime runs it*. ADR 0001 already puts the runtime behind an adapter; this is the design-time half of that boundary.

## Rules

1. **The assembly model stays neutral.** Appliances, ports, cables, link tags, zones and component structure carry no provider concepts. Do not add runtime-specific fields to them.
2. **One optional field chooses the design target:** `spec.target.platform` (`incus` | `xen` | `vmware`). Absent means the default (Incus), so existing files do not change. It is stored in exports and share links.
3. **Provider facts live in one table:** `PLATFORM_INFO` in `packages/assembly-model/src/platforms.ts` (name, status, guest agent, disk formats, NIC models, packaging, design guardrails). The UI reads that table and never branches on a provider name elsewhere.
4. **Status is honest.** `supported` means a first-class target; `planned` means the design is portable but there is no adapter, and validation says so (`platform-planned`).
5. **Guardrails are design hints,** not host limits. Real limits come from hosts once the backend reports them.

## Adding a provider

1. Add its id to `PLATFORMS` in `constants.ts`.
2. Add an entry to `PLATFORM_INFO`.
3. Run `npm run schema` and add a test next to the existing platform tests.
4. Later, implement an adapter behind the control plane's provider traits (`crates/domain`). The Studio does not change.

Trademarks belong to their owners; this project is not affiliated with them.

import { PLATFORMS } from "./constants";
import type { ApplicationAssembly } from "./schema";

export type PlatformId = (typeof PLATFORMS)[number];

/**
 * What the designer knows about a runtime provider. The assembly model itself stays provider-neutral
 * (appliances, ports, cables); everything provider-specific lives here so a new provider is one entry
 * plus, later, a backend adapter (ADR 0001: the runtime sits behind an adapter). Nothing here talks to a host.
 */
export interface PlatformInfo {
  id: PlatformId;
  name: string;
  /** "supported" = the first-class target; "planned" = the model is portable but no adapter exists yet. */
  status: "supported" | "planned";
  summary: string;
  /** In-guest helper that reports status and enables clean shutdown on this platform. */
  guestAgent: string;
  diskFormats: readonly string[];
  nicModels: readonly string[];
  /** How a finished design would be packaged for this platform. */
  packaging: string;
  /** Design-time guardrails only; real limits come from the hosts, and are checked when a backend reports them. */
  guardrails: { cpu: number; memoryMiB: number };
}

export const DEFAULT_PLATFORM: PlatformId = "incus";

export const PLATFORM_INFO: Readonly<Record<PlatformId, PlatformInfo>> = {
  incus: {
    id: "incus",
    name: "Incus",
    status: "supported",
    summary: "Primary target: appliances run as Incus instances (LXC system containers by default, OCI containers or QEMU virtual machines where needed) on a cluster of Linux servers.",
    guestAgent: "None for containers; the Incus agent for virtual machines",
    diskFormats: ["Storage-pool volumes", "qcow2 (virtual machines)"],
    nicModels: ["veth (containers)", "virtio (virtual machines)"],
    packaging: "Incus or OCI image plus an assembly manifest",
    guardrails: { cpu: 32, memoryMiB: 131072 },
  },
  xen: {
    id: "xen",
    name: "Xen",
    status: "planned",
    summary: "Planned alternative: the same assemblies mapped onto Xen guests. Design only; no adapter exists.",
    guestAgent: "Xen guest agent",
    diskFormats: ["VHD", "raw", "qcow2"],
    nicModels: ["netfront (paravirtual)", "Realtek RTL8139", "Intel e1000"],
    packaging: "Appliance image plus an assembly manifest",
    guardrails: { cpu: 32, memoryMiB: 131072 },
  },
  vmware: {
    id: "vmware",
    name: "VMware",
    status: "planned",
    summary: "Planned alternative: the same assemblies mapped onto VMware virtual machines. Design only; VMware is a trademark of its owner.",
    guestAgent: "VMware guest tools",
    diskFormats: ["VMDK"],
    nicModels: ["VMXNET3", "Intel e1000e"],
    packaging: "OVF/OVA package plus an assembly manifest",
    guardrails: { cpu: 32, memoryMiB: 131072 },
  },
};

export const platformOf = (a: ApplicationAssembly): PlatformId => a.spec.target?.platform ?? DEFAULT_PLATFORM;

/** Sets the design-time target. The default platform is stored as "absent" so older files stay byte-identical. */
export function setPlatform(a: ApplicationAssembly, id: PlatformId): ApplicationAssembly {
  const { target: _drop, ...spec } = a.spec;
  void _drop;
  return { ...a, spec: id === DEFAULT_PLATFORM ? spec : { ...spec, target: { platform: id } } };
}

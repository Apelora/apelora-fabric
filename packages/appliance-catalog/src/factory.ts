import type { ApplianceClass, Port, Property } from "@apelora/assembly-model";

export const p = (id: string, name: string, direction: Port["direction"], type: Port["type"], description: string, opts: Partial<Pick<Port, "optional" | "multiplicity">> = {}): Port => ({
  id, name, direction, type, description, optional: opts.optional ?? false, multiplicity: opts.multiplicity ?? "many",
});

export const prop = (id: string, label: string, type: Property["type"], o: Partial<Omit<Property, "id" | "label" | "type">> = {}): Property => ({ id, label, type, required: false, ...o });

/** Every appliance has a network interface: plug it into a network or a network appliance; unplugged it joins the default network. */
export const NIC: Port = { id: "net", name: "net", direction: "input", type: "network", optional: true, multiplicity: "one", description: "Network interface (NIC). Plug into a network or a network appliance; unplugged, it joins the default network." };

export const cls = (
  id: string, displayName: string, category: ApplianceClass["category"], summary: string, icon: string,
  ports: Port[], cpu: number, memoryMiB: number, properties: Property[] = [],
): ApplianceClass => ({
  id, version: "1.0.0", displayName, category, summary, iconAssetPath: icon,
  runtimeImage: { ref: null }, architectures: ["x86_64", "aarch64"], ports: ports.some((q) => q.id === "net") ? ports : [...ports, { ...NIC }], resources: { cpu, memoryMiB }, ...(properties.length ? { properties } : {}),
});


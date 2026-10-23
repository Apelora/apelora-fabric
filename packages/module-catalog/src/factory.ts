import type { ModuleClass, Port, Property } from "@apelora/assembly-model";

export const p = (id: string, name: string, direction: Port["direction"], type: Port["type"], description: string, opts: Partial<Pick<Port, "optional" | "multiplicity">> = {}): Port => ({
  id, name, direction, type, description, optional: opts.optional ?? false, multiplicity: opts.multiplicity ?? "many",
});

export const prop = (id: string, label: string, type: Property["type"], o: Partial<Omit<Property, "id" | "label" | "type">> = {}): Property => ({ id, label, type, required: false, ...(type === "secret" ? { supply: "deploy" as const } : {}), ...o });

/** Every module has a network interface: plug it into a network or a network module; unplugged it joins the default network. */
export const NIC: Port = { id: "net", name: "net", direction: "input", type: "network", optional: true, multiplicity: "one", description: "Network interface (NIC). Plug into a network or a network module; unplugged, it joins the default network." };

/** What a `mon` output says: a collector reads it, and a drawn connection opens only the class's metrics port. */
export const MON_DESCRIPTION = "Metrics a collector reads through its scrape input; the connection opens only this module's metrics port";

/** The metrics output (`mon`): last on the right, optional, any number of collectors. Only for classes whose metrics a collector can read as text on a port (`readableMetrics`). */
export const MON = (id = "mon"): Port => p(id, "mon", "output", "metrics", MON_DESCRIPTION, { optional: true, multiplicity: "many" });

export const cls = (
  id: string, displayName: string, category: ModuleClass["category"], summary: string, icon: string,
  ports: Port[], cpu: number, memoryMiB: number, properties: Property[] = [],
): ModuleClass => ({
  id, version: "1.0.0", displayName, category, summary, iconAssetPath: icon,
  runtimeImage: { ref: null }, architectures: ["x86_64", "aarch64"], ports: ports.some((q) => q.id === "net") ? ports : [...ports, { ...NIC }], resources: { cpu, memoryMiB }, ...(properties.length ? { properties } : {}),
});


import { portTypesCompatible } from "./compat";
import type { ApplianceClass, ApplicationAssembly, ApplianceInstance, Endpoint, Port } from "./schema";

export type Severity = "error" | "warning";

export interface Diagnostic {
  severity: Severity;
  code: string;
  message: string;
  hint?: string;
  applianceId?: string;
  connectionId?: string;
  portId?: string;
}

export type ClassLookup = (name: string, version: string) => ApplianceClass | undefined;

function findPort(a: ApplianceInstance | undefined, portId: string): Port | undefined {
  return a?.ports.find((p) => p.id === portId);
}

/**
 * Explains why `source -> target` is not a legal new connection, or returns null when it is valid.
 * Used both for live connection feedback and for whole-assembly validation.
 */
export function explainConnection(
  assembly: ApplicationAssembly,
  source: Endpoint,
  target: Endpoint,
  ignoreConnectionId?: string,
): { code: string; message: string; hint?: string } | null {
  const byId = new Map(assembly.spec.appliances.map((a) => [a.id, a]));
  const sa = byId.get(source.applianceId);
  const ta = byId.get(target.applianceId);
  if (!sa || !ta) return { code: "endpoint-missing", message: "One end of the connection no longer exists.", hint: "Remove the connection." };
  const sp = findPort(sa, source.portId);
  const tp = findPort(ta, target.portId);
  if (!sp || !tp) return { code: "port-missing", message: "A port of this connection no longer exists.", hint: "Remove the connection or recreate the port." };
  if (sp.direction !== "output" || tp.direction !== "input")
    return { code: "direction", message: "Connections must run from an output port (right) to an input port (left).", hint: "Drag from a right-hand port to a left-hand port." };
  if (source.applianceId === target.applianceId)
    return { code: "self-connection", message: "An appliance cannot be connected to itself.", hint: "Connect it to another appliance." };
  if (!portTypesCompatible(sp.type, tp.type))
    return { code: "type-mismatch", message: `${sp.name} (${sp.type}) cannot feed ${tp.name} (${tp.type}).`, hint: `Choose an input of type ${sp.type}.` };
  const others = assembly.spec.connections.filter((c) => c.id !== ignoreConnectionId);
  if (others.some((c) => c.source.applianceId === source.applianceId && c.source.portId === source.portId && c.target.applianceId === target.applianceId && c.target.portId === target.portId))
    return { code: "duplicate", message: "These ports are already connected.", hint: "Select the existing connection instead." };
  if (sp.multiplicity === "one" && others.some((c) => c.source.applianceId === source.applianceId && c.source.portId === source.portId))
    return { code: "source-full", message: `${sp.name} on ${sa.name} accepts only one connection.`, hint: "Disconnect it first or add another output port." };
  if (tp.multiplicity === "one" && others.some((c) => c.target.applianceId === target.applianceId && c.target.portId === target.portId))
    return { code: "target-full", message: `${tp.name} on ${ta.name} accepts only one connection.`, hint: "Disconnect it first or add another input port." };
  return null;
}

export function validateAssembly(assembly: ApplicationAssembly, lookup: ClassLookup): Diagnostic[] {
  const out: Diagnostic[] = [];
  const seen = new Set<string>();
  for (const a of assembly.spec.appliances) {
    if (seen.has(a.id)) out.push({ severity: "error", code: "duplicate-appliance", message: `Duplicate appliance id "${a.id}".`, hint: "Appliance ids must be unique.", applianceId: a.id });
    seen.add(a.id);

    const cls = lookup(a.classRef.name, a.classRef.version);
    if (!cls) out.push({ severity: "error", code: "class-missing", message: `${a.name}: appliance class ${a.classRef.name}@${a.classRef.version} is not in the catalog.`, hint: "Replace it with a catalog appliance.", applianceId: a.id });

    const portIds = new Set<string>();
    for (const p of a.ports) {
      if (portIds.has(p.id)) out.push({ severity: "error", code: "duplicate-port", message: `${a.name} has two ports with id "${p.id}".`, hint: "Port ids must be unique per appliance.", applianceId: a.id, portId: p.id });
      portIds.add(p.id);
    }
    if (a.resources.cpu > 32 || a.resources.memoryMiB > 131072)
      out.push({ severity: "warning", code: "resources-high", message: `${a.name} requests ${a.resources.cpu} vCPU / ${a.resources.memoryMiB} MiB.`, hint: "Check the request against your target hosts.", applianceId: a.id });
  }

  const connIds = new Set<string>();
  for (const c of assembly.spec.connections) {
    if (connIds.has(c.id)) out.push({ severity: "error", code: "duplicate-connection-id", message: `Duplicate connection id "${c.id}".`, connectionId: c.id });
    connIds.add(c.id);
    const problem = explainConnection(assembly, c.source, c.target, c.id);
    if (problem) out.push({ severity: "error", ...problem, connectionId: c.id, applianceId: c.target.applianceId });
  }

  for (const a of assembly.spec.appliances) {
    for (const p of a.ports) {
      if (p.direction === "input" && !p.optional && !assembly.spec.connections.some((c) => c.target.applianceId === a.id && c.target.portId === p.id))
        out.push({ severity: "error", code: "required-port-unconnected", message: `${a.name}: required input "${p.name}" is not connected.`, hint: `Connect a ${p.type} output to it, or mark the port optional.`, applianceId: a.id, portId: p.id });
    }
  }
  if (assembly.spec.appliances.length === 0)
    out.push({ severity: "warning", code: "empty", message: "The assembly has no appliances.", hint: "Drag appliances from the catalog onto the canvas." });
  return out;
}

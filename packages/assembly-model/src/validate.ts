import { portTypesCompatible } from "./compat";
import { effectiveValue, validateProperty } from "./config";
import { platformOf, PLATFORM_INFO } from "./platforms";
import { explainTag, linkCount, tagNames, tagNet, portOf } from "./tags";
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
  if (sp.multiplicity === "one" && linkCount(assembly, source, { connectionId: ignoreConnectionId }) >= 1)
    return { code: "source-full", message: `${sp.name} on ${sa.name} accepts only one connection.`, hint: "Disconnect it first or add another output port." };
  if (tp.multiplicity === "one" && linkCount(assembly, target, { connectionId: ignoreConnectionId }) >= 1)
    return { code: "target-full", message: `${tp.name} on ${ta.name} accepts only one connection.`, hint: "Disconnect it first or add another input port." };
  return null;
}

export function validateAssembly(assembly: ApplicationAssembly, lookup: ClassLookup): Diagnostic[] {
  const out: Diagnostic[] = [];
  const platform = PLATFORM_INFO[platformOf(assembly)];
  const guard = platform.guardrails;
  if (platform.status === "planned")
    out.push({ severity: "warning", code: "platform-planned", message: `${platform.name} is a planned target: this design is portable but cannot be deployed yet.`, hint: "Switch the target back to Incus in the assembly properties, or keep designing." });
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
    for (const prop of cls?.properties ?? []) {
      const problem = validateProperty(prop, effectiveValue(prop, a.config));
      if (problem) out.push({ severity: "error", code: "config-invalid", message: `${a.name}: ${problem.message}`, hint: problem.hint, applianceId: a.id });
    }
    if (a.resources.cpu > guard.cpu || a.resources.memoryMiB > guard.memoryMiB)
      out.push({ severity: "warning", code: "resources-high", message: `${a.name} requests ${a.resources.cpu} vCPU / ${a.resources.memoryMiB} MiB.`, hint: `Check the request against your ${platform.name} hosts.`, applianceId: a.id });
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
      const tagged = (assembly.spec.links ?? []).find((l) => l.endpoint.applianceId === a.id && l.endpoint.portId === p.id);
      const fedByTag = !!tagged && tagNet(assembly, tagged.tag).some((l) => portOf(assembly, l.endpoint)?.direction === "output");
      if (p.direction === "input" && !p.optional && !fedByTag && !assembly.spec.connections.some((c) => c.target.applianceId === a.id && c.target.portId === p.id))
        out.push({ severity: "error", code: "required-port-unconnected", message: `${a.name}: required input "${p.name}" is not connected.`, hint: `Connect a ${p.type} output to it, or mark the port optional.`, applianceId: a.id, portId: p.id });
    }
  }
  for (const l of assembly.spec.links ?? []) {
    const problem = explainTag(assembly, l.endpoint, l.tag, l.id);
    if (problem) out.push({ severity: "error", code: "tag-invalid", message: problem.message, hint: problem.hint, applianceId: l.endpoint.applianceId, portId: l.endpoint.portId });
  }
  for (const name of tagNames(assembly)) {
    const net = tagNet(assembly, name);
    const outs = net.filter((l) => portOf(assembly, l.endpoint)?.direction === "output").length;
    const ins = net.length - outs;
    if (outs === 0) out.push({ severity: "warning", code: "tag-no-source", message: `Tag “${name}” has ${ins} input${ins === 1 ? "" : "s"} but nothing feeds it.`, hint: "Give an output terminal the same tag.", applianceId: net[0]?.endpoint.applianceId });
    else if (ins === 0) out.push({ severity: "warning", code: "tag-no-target", message: `Tag “${name}” has no listening input.`, hint: "Give an input terminal the same tag.", applianceId: net[0]?.endpoint.applianceId });
  }
  if (assembly.spec.appliances.length === 0)
    out.push({ severity: "warning", code: "empty", message: "The assembly has no appliances.", hint: "Drag appliances from the catalog onto the canvas." });
  return out;
}

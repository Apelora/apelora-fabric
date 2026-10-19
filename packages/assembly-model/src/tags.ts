import { portTypesCompatible } from "./compat";
import type { ApplicationAssembly, Endpoint, LinkTag, Port } from "./schema";

export const TAG_NAME = /^[a-z0-9][a-z0-9._-]{0,31}$/;

const same = (a: Endpoint, b: Endpoint) => a.applianceId === b.applianceId && a.portId === b.portId;
const links = (a: ApplicationAssembly): LinkTag[] => a.spec.links ?? [];

export function portOf(a: ApplicationAssembly, e: Endpoint): Port | undefined {
  return a.spec.appliances.find((x) => x.id === e.applianceId)?.ports.find((p) => p.id === e.portId);
}

export function tagOf(a: ApplicationAssembly, e: Endpoint): LinkTag | undefined {
  return links(a).find((l) => same(l.endpoint, e));
}

/** Every terminal sharing a name. */
export function tagNet(a: ApplicationAssembly, name: string): LinkTag[] {
  return links(a).filter((l) => l.tag === name);
}

export function tagNames(a: ApplicationAssembly): string[] {
  return [...new Set(links(a).map((l) => l.tag))].sort();
}

/** Number of links a terminal already has: its cables plus the opposite-direction terminals in its tag net. */
export function linkCount(a: ApplicationAssembly, e: Endpoint, ignore?: { connectionId?: string; linkId?: string }): number {
  const port = portOf(a, e);
  let n = a.spec.connections.filter((c) => c.id !== ignore?.connectionId && (same(c.source, e) || same(c.target, e))).length;
  const mine = links(a).find((l) => l.id !== ignore?.linkId && same(l.endpoint, e));
  if (mine && port) n += tagNet(a, mine.tag).filter((l) => l.id !== ignore?.linkId && !same(l.endpoint, e) && portOf(a, l.endpoint)?.direction !== port.direction).length;
  return n;
}

export interface Problem { code: string; message: string; hint?: string }

/** Why `endpoint` cannot join net `name` (ignoring one link being replaced), or null. */
export function explainTag(a: ApplicationAssembly, endpoint: Endpoint, name: string, ignoreLinkId?: string): Problem | null {
  if (!TAG_NAME.test(name)) return { code: "tag-name", message: `“${name}” is not a valid tag name.`, hint: "Use lowercase letters, digits, . _ or - (up to 32 characters)." };
  const port = portOf(a, endpoint);
  if (!port) return { code: "port-missing", message: "That port no longer exists." };
  const members = tagNet(a, name).filter((l) => l.id !== ignoreLinkId && !same(l.endpoint, endpoint));
  for (const m of members) {
    const mp = portOf(a, m.endpoint);
    if (!mp || mp.direction === port.direction) continue;
    const [out, outE, inn, inE] = port.direction === "output" ? [port, endpoint, mp, m.endpoint] : [mp, m.endpoint, port, endpoint];
    if (outE.applianceId === inE.applianceId) return { code: "tag-self", message: `Tag “${name}” would connect an appliance to itself.`, hint: "Use a different tag name." };
    if (!portTypesCompatible(out.type, inn.type)) return { code: "tag-type", message: `Tag “${name}”: ${out.name} (${out.type}) cannot feed ${inn.name} (${inn.type}).`, hint: `Use a tag whose members share the protocol ${out.type}.` };
  }
  if (port.multiplicity === "one") {
    const peers = members.filter((m) => portOf(a, m.endpoint)?.direction !== port.direction).length;
    const cables = a.spec.connections.filter((c) => same(c.source, endpoint) || same(c.target, endpoint)).length;
    if (cables + peers > 1 || (cables + peers === 1 && cables > 0)) return { code: "tag-full", message: `${port.name} accepts only one link.`, hint: "Unplug its cable first, or allow several cables on that port." };
  }
  for (const m of members) {
    const mp = portOf(a, m.endpoint);
    if (mp && mp.direction !== port.direction && mp.multiplicity === "one" && linkCount(a, m.endpoint, { linkId: ignoreLinkId }) >= 1) return { code: "tag-peer-full", message: `${mp.name} on another appliance already has its single link.`, hint: "Choose a different tag name." };
  }
  return null;
}

function nextId(prefix: string, taken: Set<string>): string {
  let n = taken.size + 1;
  while (taken.has(`${prefix}-${String(n).padStart(2, "0")}`)) n++;
  return `${prefix}-${String(n).padStart(2, "0")}`;
}

/** Joins (or moves) a terminal into net `name`. */
export function tagPort(a: ApplicationAssembly, endpoint: Endpoint, name: string): { assembly: ApplicationAssembly } | { error: Problem } {
  const existing = tagOf(a, endpoint);
  if (existing?.tag === name) return { assembly: a };
  const error = explainTag(a, endpoint, name, existing?.id);
  if (error) return { error };
  const rest = links(a).filter((l) => l.id !== existing?.id);
  const id = nextId("tag", new Set(links(a).map((l) => l.id)));
  return { assembly: { ...a, spec: { ...a.spec, links: [...rest, { id, tag: name, endpoint }] } } };
}

export function untagPort(a: ApplicationAssembly, endpoint: Endpoint): ApplicationAssembly {
  const next = links(a).filter((l) => !same(l.endpoint, endpoint));
  return { ...a, spec: { ...a.spec, links: next.length ? next : undefined } };
}

export function removeTag(a: ApplicationAssembly, name: string): ApplicationAssembly {
  const next = links(a).filter((l) => l.tag !== name);
  return { ...a, spec: { ...a.spec, links: next.length ? next : undefined } };
}

/** Renames (or merges into) a net, checking every member against the target net. */
export function renameTag(a: ApplicationAssembly, from: string, to: string): { assembly: ApplicationAssembly } | { error: Problem } {
  if (from === to) return { assembly: a };
  let cur = a;
  for (const m of tagNet(a, from)) {
    const r = tagPort(cur, m.endpoint, to);
    if ("error" in r) return r;
    cur = r.assembly;
  }
  return { assembly: cur };
}

/** Replaces a cable with two same-named tags, so the diagram needs no line between the appliances. */
export function cableToTag(a: ApplicationAssembly, connectionId: string, name?: string): { assembly: ApplicationAssembly; tag: string } | { error: Problem } {
  const c = a.spec.connections.find((x) => x.id === connectionId);
  if (!c) return { error: { code: "connection-missing", message: "That cable no longer exists." } };
  const sp = portOf(a, c.source);
  let tag = name;
  if (!tag) {
    const used = new Set(tagNames(a));
    let n = 1;
    const base = sp?.type ?? "link";
    while (used.has(`${base}-${n}`)) n++;
    tag = `${base}-${n}`;
  }
  const without: ApplicationAssembly = { ...a, spec: { ...a.spec, connections: a.spec.connections.filter((x) => x.id !== connectionId) } };
  const first = tagPort(without, c.source, tag);
  if ("error" in first) return first;
  const second = tagPort(first.assembly, c.target, tag);
  if ("error" in second) return second;
  return { assembly: second.assembly, tag };
}

/** Turns a one-output/one-input tag net back into a drawn cable. */
export function tagToCable(a: ApplicationAssembly, name: string): { assembly: ApplicationAssembly } | { error: Problem } {
  const net = tagNet(a, name);
  const outs = net.filter((l) => portOf(a, l.endpoint)?.direction === "output");
  const ins = net.filter((l) => portOf(a, l.endpoint)?.direction === "input");
  if (outs.length !== 1 || ins.length !== 1) return { error: { code: "tag-not-pair", message: `Tag “${name}” links ${net.length} terminals.`, hint: "Only a tag with exactly one output and one input can become a cable." } };
  const without = removeTag(a, name);
  const taken = new Set(without.spec.connections.map((c) => c.id));
  const id = nextId("conn", taken);
  return { assembly: { ...without, spec: { ...without.spec, connections: [...without.spec.connections, { id, source: outs[0]!.endpoint, target: ins[0]!.endpoint }] } } };
}

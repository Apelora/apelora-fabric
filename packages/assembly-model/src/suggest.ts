import { effectiveSize, DENSITY_SIZE } from "./geometry";
import { portTypesCompatible } from "./compat";
import { addModule, connect, findFreeSpot } from "./ops";
import { portOf } from "./tags";
import type { ModuleClass, ApplicationAssembly, Endpoint, Port } from "./schema";

export interface Suggestion {
  cls: ModuleClass;
  /** The terminal of the new module that would be plugged in. */
  port: Port;
  /** True when the protocols are identical (not just compatible); such matches are listed first. */
  exact: boolean;
}

/**
 * Catalog modules that could be plugged into `end` by a single new cable: for an output terminal, classes with a
 * compatible input; for an input terminal, classes with a compatible output. Network (NIC) terminals are skipped
 * because every module already has one; use a Network module for that.
 */
export function suggestAttachments(a: ApplicationAssembly, end: Endpoint, classes: readonly ModuleClass[]): Suggestion[] {
  const port = portOf(a, end);
  if (!port || port.type === "network") return [];
  const want = port.direction === "output" ? "input" : "output";
  const out: Suggestion[] = [];
  for (const cls of classes) {
    const match = cls.ports.filter((p) => p.direction === want && p.type !== "network" && (port.direction === "output" ? portTypesCompatible(port.type, p.type) : portTypesCompatible(p.type, port.type)));
    if (!match.length) continue;
    // Prefer the first required, exactly-typed terminal as the one that gets plugged in.
    const best = [...match].sort((p, q) => Number(q.type === port.type) - Number(p.type === port.type) || Number(p.optional) - Number(q.optional))[0]!;
    out.push({ cls, port: best, exact: best.type === port.type });
  }
  return out.sort((p, q) => Number(q.exact) - Number(p.exact) || p.cls.category.localeCompare(q.cls.category) || p.cls.displayName.localeCompare(q.cls.displayName));
}

/** Adds the class beside the terminal's module (to its right for an output, to its left for an input) and plugs it in. */
export function attachNew(a: ApplicationAssembly, end: Endpoint, cls: ModuleClass, portId: string): { assembly: ApplicationAssembly; id: string } | { error: string } {
  const host = a.spec.modules.find((x) => x.id === end.moduleId);
  const layout = a.editor.nodes.find((n) => n.id === end.moduleId);
  const port = portOf(a, end);
  if (!host || !layout || !port) return { error: "That terminal no longer exists." };
  const hostSize = effectiveSize(layout, host.ports, a.editor.density);
  const size = DENSITY_SIZE[a.editor.density];
  const rightSide = port.direction === "output";
  const spot = findFreeSpot(a, { x: rightSide ? layout.x + hostSize.width + 110 : layout.x - size.width - 110, y: layout.y }, size);
  const added = addModule(a, cls, spot);
  const other: Endpoint = { moduleId: added.id, portId };
  const r = rightSide ? connect(added.assembly, end, other) : connect(added.assembly, other, end);
  if ("error" in r) return { error: `${r.error.message}${r.error.hint ? ` ${r.error.hint}` : ""}` };
  return { assembly: r.assembly, id: added.id };
}

/** Ports that a cable dragged from `from` could legally land on. Used to light up compatible sockets while dragging. */
export function compatibleTargets(a: ApplicationAssembly, from: Endpoint): Endpoint[] {
  const p = portOf(a, from);
  if (!p) return [];
  const out: Endpoint[] = [];
  for (const app of a.spec.modules) {
    if (app.id === from.moduleId) continue;
    for (const q of app.ports) {
      if (q.direction === p.direction) continue;
      const ok = p.direction === "output" ? portTypesCompatible(p.type, q.type) : portTypesCompatible(q.type, p.type);
      if (ok) out.push({ moduleId: app.id, portId: q.id });
    }
  }
  return out;
}

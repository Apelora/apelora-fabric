import { effectiveSize } from "./geometry";
import { portOf } from "./tags";
import { shapeContents, zoneRectAround } from "./shapes";
import type { ApplicationAssembly } from "./schema";

const H_GAP = 96;
const V_GAP = 36;

/**
 * Arranges appliances left to right following the data flow (a cable runs from a column to a later one),
 * keeps the top-left corner where it was, puts unconnected appliances in a row below, and re-fits every zone
 * around the appliances it contained. Network-type cables are ignored for ordering, so a network segment does
 * not push the whole diagram one column to the right.
 */
export function tidyLayout(a: ApplicationAssembly, only?: readonly string[]): ApplicationAssembly {
  const scope = new Set(only && only.length > 1 ? only : a.spec.appliances.map((x) => x.id));
  const nodes = a.editor.nodes.filter((n) => scope.has(n.id));
  if (nodes.length < 2) return a;

  const size = new Map(nodes.map((n) => {
    const app = a.spec.appliances.find((x) => x.id === n.id);
    return [n.id, effectiveSize(n, app?.ports ?? [], a.editor.density)] as const;
  }));
  const originX = Math.min(...nodes.map((n) => n.x));
  const originY = Math.min(...nodes.map((n) => n.y));

  // Flow edges between in-scope appliances, without network cables and duplicates.
  const out = new Map<string, Set<string>>();
  const incoming = new Map<string, Set<string>>();
  const touched = new Set<string>();
  const edge = (s: string, t: string) => {
    if (s === t || !scope.has(s) || !scope.has(t)) return;
    (out.get(s) ?? out.set(s, new Set()).get(s)!).add(t);
    (incoming.get(t) ?? incoming.set(t, new Set()).get(t)!).add(s);
    touched.add(s); touched.add(t);
  };
  for (const c of a.spec.connections) if (portOf(a, c.source)?.type !== "network") edge(c.source.applianceId, c.target.applianceId);

  // Break cycles: depth-first search, dropping the edge that closes a loop.
  const state = new Map<string, 1 | 2>();
  const dag = new Map<string, Set<string>>();
  const visit = (u: string) => {
    state.set(u, 1);
    for (const v of [...(out.get(u) ?? [])].sort()) {
      if (state.get(v) === 1) continue;
      (dag.get(u) ?? dag.set(u, new Set()).get(u)!).add(v);
      if (!state.has(v)) visit(v);
    }
    state.set(u, 2);
  };
  for (const n of nodes.map((x) => x.id).sort()) if (touched.has(n) && !state.has(n)) visit(n);

  // Longest-path layering over the acyclic graph.
  const layer = new Map<string, number>();
  const depth = (u: string): number => {
    const known = layer.get(u);
    if (known !== undefined) return known;
    layer.set(u, 0);
    let best = 0;
    for (const [p, targets] of dag) if (targets.has(u)) best = Math.max(best, depth(p) + 1);
    layer.set(u, best);
    return best;
  };
  const connected = nodes.filter((n) => touched.has(n.id)).map((n) => n.id);
  for (const id of connected) depth(id);

  const columns: string[][] = [];
  for (const id of connected) (columns[layer.get(id)!] ??= []).push(id);
  const order = new Map<string, number>();
  const sweep = (down: boolean) => {
    const cols = down ? columns : [...columns].reverse();
    for (const col of cols) {
      if (!col) continue;
      const score = (id: string) => {
        const nb = [...((down ? incoming : out).get(id) ?? [])].filter((x) => order.has(x));
        return nb.length ? nb.reduce((s, x) => s + order.get(x)!, 0) / nb.length : order.get(id) ?? 0;
      };
      col.sort((p, q) => score(p) - score(q) || (p < q ? -1 : 1));
      col.forEach((id, i) => order.set(id, i));
    }
  };
  columns.forEach((col) => col?.sort().forEach((id, i) => order.set(id, i)));
  for (let i = 0; i < 3; i++) { sweep(true); sweep(false); }

  const pos = new Map<string, { x: number; y: number }>();
  let x = originX;
  let bottom = originY;
  for (const col of columns) {
    if (!col?.length) continue;
    let y = originY;
    let w = 0;
    for (const id of col) {
      const sz = size.get(id)!;
      pos.set(id, { x, y });
      y += sz.height + V_GAP;
      w = Math.max(w, sz.width);
    }
    bottom = Math.max(bottom, y - V_GAP);
    x += w + H_GAP;
  }
  // Unconnected appliances wrap into rows below the flow.
  const loose = nodes.filter((n) => !touched.has(n.id)).map((n) => n.id).sort();
  const rowWidth = Math.max(x - H_GAP - originX, 640);
  let lx = originX;
  let ly = connected.length ? bottom + V_GAP * 2 : originY;
  let rowH = 0;
  for (const id of loose) {
    const sz = size.get(id)!;
    if (lx > originX && lx + sz.width - originX > rowWidth) { lx = originX; ly += rowH + V_GAP; rowH = 0; }
    pos.set(id, { x: lx, y: ly });
    lx += sz.width + H_GAP / 2;
    rowH = Math.max(rowH, sz.height);
  }

  // Zones are re-fitted around the appliances they held before the move.
  const held = new Map((a.editor.shapes ?? []).filter((s) => s.kind !== "note").map((s) => [s.id, shapeContents(a, s.id).filter((id) => scope.has(id))] as const));
  const editorNodes = a.editor.nodes.map((n) => { const p = pos.get(n.id); return p ? { ...n, x: Math.round(p.x), y: Math.round(p.y) } : n; });
  const shapes = a.editor.shapes?.map((s) => {
    const ids = held.get(s.id) ?? [];
    if (!ids.length) return s;
    const rect = zoneRectAround(editorNodes.filter((n) => ids.includes(n.id)).map((n) => ({ ...n, ...size.get(n.id)! })), s.kind);
    return { ...s, x: Math.round(rect.x), y: Math.round(rect.y), width: Math.round(rect.width), height: Math.round(rect.height) };
  });
  return { ...a, editor: { ...a.editor, nodes: editorNodes, ...(shapes ? { shapes } : {}) } };
}

/**
 * Snaps a dragged appliance to the edges and centres of the others when they are within `threshold` flow units,
 * and reports the guide lines to draw. Only the nearest match on each axis is used.
 */
export function alignmentGuides(a: ApplicationAssembly, id: string, pos: { x: number; y: number }, threshold = 6): { pos: { x: number; y: number }; guides: { x: number[]; y: number[] } } {
  const sizeOf = (n: ApplicationAssembly["editor"]["nodes"][number]) => effectiveSize(n, a.spec.appliances.find((x) => x.id === n.id)?.ports ?? [], a.editor.density);
  const me = a.editor.nodes.find((n) => n.id === id);
  if (!me) return { pos, guides: { x: [], y: [] } };
  const { width: w, height: h } = sizeOf(me);
  const others = a.editor.nodes.filter((n) => n.id !== id).map((n) => ({ n, ...sizeOf(n) }));
  const best = (mine: number[], theirs: number[]): { delta: number; at: number } | null => {
    let found: { delta: number; at: number } | null = null;
    for (const m of mine) for (const t of theirs) { const d = t - m; if (Math.abs(d) <= threshold && (!found || Math.abs(d) < Math.abs(found.delta))) found = { delta: d, at: t }; }
    return found;
  };
  const bx = best([pos.x, pos.x + w / 2, pos.x + w], others.flatMap((o) => [o.n.x, o.n.x + o.width / 2, o.n.x + o.width]));
  const by = best([pos.y, pos.y + h / 2, pos.y + h], others.flatMap((o) => [o.n.y, o.n.y + o.height / 2, o.n.y + o.height]));
  return { pos: { x: pos.x + (bx?.delta ?? 0), y: pos.y + (by?.delta ?? 0) }, guides: { x: bx ? [bx.at] : [], y: by ? [by.at] : [] } };
}

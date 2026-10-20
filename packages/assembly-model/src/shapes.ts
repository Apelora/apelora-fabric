import type { ApplicationAssembly, Shape, ShapeKind } from "./schema";

/** Modern, muted zone colours (fill and border share a hue; the fill is drawn translucent). */
export const SHAPE_PALETTE: readonly { name: string; color: string }[] = [
  { name: "Slate", color: "#64748b" },
  { name: "Blue", color: "#3b82f6" },
  { name: "Teal", color: "#14b8a6" },
  { name: "Green", color: "#22c55e" },
  { name: "Amber", color: "#f59e0b" },
  { name: "Rose", color: "#f43f5e" },
  { name: "Violet", color: "#8b5cf6" },
  { name: "Gray", color: "#9ca3af" },
];

const shapesOf = (a: ApplicationAssembly): Shape[] => a.editor.shapes ?? [];
const withShapes = (a: ApplicationAssembly, shapes: Shape[]): ApplicationAssembly => ({ ...a, editor: { ...a.editor, shapes: shapes.length ? shapes : undefined } });

function nextShapeId(a: ApplicationAssembly): string {
  const taken = new Set(shapesOf(a).map((s) => s.id));
  let n = taken.size + 1;
  while (taken.has(`shape-${String(n).padStart(2, "0")}`)) n++;
  return `shape-${String(n).padStart(2, "0")}`;
}

export const DEFAULT_LABEL: Record<ShapeKind, string> = { rect: "Zone", ellipse: "Sector", rounded: "Zone", triangle: "Sector", note: "" };

/** Sticky-note look: warm paper fill, thin amber border. Notes carry free text and never wrap appliances. */
const NOTE_STYLE = { fill: "#fde68a", fillOpacity: 0.92, stroke: "#d97706", strokeWidth: 1, radius: 4, text: "Note" } as const;

export function makeShape(a: ApplicationAssembly, kind: ShapeKind, rect: { x: number; y: number; width: number; height: number }, over: Partial<Shape> = {}): Shape {
  const color = SHAPE_PALETTE[shapesOf(a).length % SHAPE_PALETTE.length]!.color;
  if (kind === "note")
    return { id: nextShapeId(a), kind, x: Math.round(rect.x), y: Math.round(rect.y), width: Math.round(rect.width), height: Math.round(rect.height), label: "", labelPos: "top-left", fontSize: 12, strokeStyle: "solid", locked: false, ...NOTE_STYLE, ...over };
  return {
    id: nextShapeId(a), kind, x: Math.round(rect.x), y: Math.round(rect.y), width: Math.round(rect.width), height: Math.round(rect.height),
    label: DEFAULT_LABEL[kind], labelPos: kind === "triangle" ? "center" : "top-left", fontSize: 12,
    fill: color, fillOpacity: 0.12, stroke: color, strokeWidth: 1.5, strokeStyle: "solid", radius: kind === "rounded" ? 18 : 0, locked: false,
    ...over,
  };
}

export function addShape(a: ApplicationAssembly, kind: ShapeKind, rect: { x: number; y: number; width: number; height: number }, over: Partial<Shape> = {}): { assembly: ApplicationAssembly; id: string } {
  const s = makeShape(a, kind, rect, over);
  return { id: s.id, assembly: withShapes(a, [...shapesOf(a), s]) };
}

/** Rectangle a zone needs around the given nodes, with room for its label. */
export function zoneRectAround(nodes: readonly { x: number; y: number; width: number; height: number }[], kind: ShapeKind): { x: number; y: number; width: number; height: number } {
  const pad = kind === "ellipse" || kind === "triangle" ? 56 : 30;
  const minX = Math.min(...nodes.map((n) => n.x));
  const minY = Math.min(...nodes.map((n) => n.y));
  const maxX = Math.max(...nodes.map((n) => n.x + n.width));
  const maxY = Math.max(...nodes.map((n) => n.y + n.height));
  const rect = { x: minX - pad, y: minY - pad - 14, width: maxX - minX + pad * 2, height: maxY - minY + pad * 2 + 14 };
  if (kind === "triangle") return { x: rect.x - rect.width * 0.25, y: rect.y - rect.height * 0.4, width: rect.width * 1.5, height: rect.height * 1.6 };
  return rect;
}

/** A zone wrapped around the given appliances, with room for its label. */
export function zoneAround(a: ApplicationAssembly, ids: readonly string[], kind: ShapeKind): { assembly: ApplicationAssembly; id: string } | null {
  const nodes = a.editor.nodes.filter((n) => ids.includes(n.id));
  if (nodes.length === 0) return null;
  return addShape(a, kind, zoneRectAround(nodes, kind));
}

export function updateShape(a: ApplicationAssembly, id: string, patch: Partial<Omit<Shape, "id">>): ApplicationAssembly {
  return withShapes(a, shapesOf(a).map((s) => (s.id === id ? { ...s, ...patch } : s)));
}

export function removeShapes(a: ApplicationAssembly, ids: readonly string[]): ApplicationAssembly {
  return withShapes(a, shapesOf(a).filter((s) => !ids.includes(s.id)));
}

export function moveShapes(a: ApplicationAssembly, moves: ReadonlyMap<string, { x: number; y: number }>): ApplicationAssembly {
  return withShapes(a, shapesOf(a).map((s) => { const m = moves.get(s.id); return m ? { ...s, x: Math.round(m.x), y: Math.round(m.y) } : s; }));
}

export function resizeShape(a: ApplicationAssembly, id: string, rect: { width: number; height: number; x?: number; y?: number }): ApplicationAssembly {
  return withShapes(a, shapesOf(a).map((s) => (s.id === id ? { ...s, width: Math.max(40, Math.round(rect.width)), height: Math.max(30, Math.round(rect.height)), x: rect.x === undefined ? s.x : Math.round(rect.x), y: rect.y === undefined ? s.y : Math.round(rect.y) } : s)));
}

/** Moves a shape to the front or back among shapes (shapes always stay behind appliances). */
export function orderShape(a: ApplicationAssembly, id: string, to: "front" | "back"): ApplicationAssembly {
  const list = shapesOf(a);
  const me = list.find((s) => s.id === id);
  if (!me) return a;
  const rest = list.filter((s) => s.id !== id);
  return withShapes(a, to === "front" ? [...rest, me] : [me, ...rest]);
}

export function duplicateShape(a: ApplicationAssembly, id: string): { assembly: ApplicationAssembly; id: string } | null {
  const me = shapesOf(a).find((s) => s.id === id);
  if (!me) return null;
  const copy = makeShape(a, me.kind, { x: me.x + 32, y: me.y + 32, width: me.width, height: me.height }, { label: me.label, ...(me.text !== undefined ? { text: me.text } : {}), labelPos: me.labelPos, fontSize: me.fontSize, fill: me.fill, fillOpacity: me.fillOpacity, stroke: me.stroke, strokeWidth: me.strokeWidth, strokeStyle: me.strokeStyle, radius: me.radius });
  return { id: copy.id, assembly: withShapes(a, [...shapesOf(a), copy]) };
}

/** Appliances whose centre lies inside a shape: they travel with it when it is dragged. */
export function shapeContents(a: ApplicationAssembly, shapeId: string): string[] {
  const s = shapesOf(a).find((x) => x.id === shapeId);
  if (!s || s.kind === "note") return [];
  return a.editor.nodes.filter((n) => {
    const cx = n.x + n.width / 2;
    const cy = n.y + n.height / 2;
    if (cx < s.x || cx > s.x + s.width || cy < s.y || cy > s.y + s.height) return false;
    if (s.kind === "ellipse") { const dx = (cx - s.x - s.width / 2) / (s.width / 2); const dy = (cy - s.y - s.height / 2) / (s.height / 2); return dx * dx + dy * dy <= 1; }
    if (s.kind === "triangle") { const t = (cy - s.y) / s.height; const half = (s.width / 2) * t; return Math.abs(cx - s.x - s.width / 2) <= half; }
    return true;
  }).map((n) => n.id);
}

import { NODE, effectiveSize, portCenterY, portOf, tagNet, type ModuleClass, type ApplicationAssembly, type Shape } from "@apelora/assembly-model";
import { logoUrl, lookupClass } from "@apelora/module-catalog";
import { cableColor } from "../../components/canvas/cables";
import { tagColor } from "../../components/canvas/TagStub";
import { categoryColor } from "../editor/categories";

const esc = (s: string): string => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
const PAD = 48;
const INK = "#1f2937";
const MUTED = "#6b7280";

export interface ImageOptions {
  /** Class → data URI of its icon. Missing icons are simply left out. */
  icons?: ReadonlyMap<string, string>;
  routing?: "curved" | "angled";
  /** Title drawn in the top-left corner. */
  title?: boolean;
}

/** oklch() is not understood by every SVG consumer, so resolve it to a plain colour with the browser when one is available. */
const flat = (color: string): string => {
  if (typeof document === "undefined" || !color.includes("oklch") && !color.includes("color-mix")) return color;
  const c = document.createElement("canvas").getContext("2d");
  if (!c) return color;
  c.fillStyle = "#000";
  c.fillStyle = color;
  const hex = c.fillStyle;
  return hex.startsWith("#") || hex.startsWith("rgb") ? hex : color;
};

function dash(s: Shape): string {
  const w = s.strokeWidth;
  return s.strokeStyle === "dashed" ? ` stroke-dasharray="${Math.max(6, w * 4)} ${Math.max(4, w * 3)}"` : s.strokeStyle === "dotted" ? ` stroke-dasharray="${Math.max(1, w)} ${Math.max(4, w * 2.5)}"` : "";
}

function shapeSvg(s: Shape, ox: number, oy: number): string {
  const x = s.x - ox, y = s.y - oy, w = s.width, h = s.height;
  if (s.kind === "note") {
    const lines = (s.text ?? "").split("\n").flatMap((l) => wrap(l, Math.max(8, Math.floor((w - 20) / (s.fontSize * 0.55)))));
    const text = lines.slice(0, Math.max(1, Math.floor((h - 16) / (s.fontSize * 1.35)))).map((l, i) => `<tspan x="${x + 10}" dy="${i === 0 ? 0 : s.fontSize * 1.35}">${esc(l)}</tspan>`).join("");
    return `<g><rect x="${x}" y="${y}" width="${w}" height="${h}" rx="4" fill="${s.fill}" stroke="${s.stroke}" stroke-width="1"/><text x="${x + 10}" y="${y + 10 + s.fontSize}" font-size="${s.fontSize}" fill="#3f3a2c">${text}</text></g>`;
  }
  const common = `fill="${s.fill}" fill-opacity="${s.fillOpacity}" stroke="${s.strokeWidth > 0 ? s.stroke : "none"}" stroke-width="${s.strokeWidth}"${dash(s)} stroke-linejoin="round"`;
  const half = s.strokeWidth / 2;
  let body: string;
  if (s.kind === "ellipse") body = `<ellipse cx="${x + w / 2}" cy="${y + h / 2}" rx="${w / 2 - half}" ry="${h / 2 - half}" ${common}/>`;
  else if (s.kind === "triangle") body = `<polygon points="${x + w / 2},${y + half} ${x + w - half},${y + h - half} ${x + half},${y + h - half}" ${common}/>`;
  else body = `<rect x="${x + half}" y="${y + half}" width="${w - s.strokeWidth}" height="${h - s.strokeWidth}" rx="${s.kind === "rounded" ? Math.min(s.radius, w / 2, h / 2) : 0}" ${common}/>`;
  let label = "";
  if (s.label) {
    const tri = s.kind === "triangle";
    const cx = s.labelPos === "top-left" && !tri ? x + 12 : x + w / 2;
    const cy = s.labelPos === "bottom" ? y + h - 12 : s.labelPos === "center" ? y + h / 2 + s.fontSize / 3 : tri ? y + h * 0.38 : y + 10 + s.fontSize;
    const anchor = s.labelPos === "top-left" && !tri ? "start" : "middle";
    label = `<text x="${cx}" y="${tri && s.labelPos === "center" ? y + h * 0.66 : cy}" font-size="${s.fontSize}" font-weight="600" fill="${INK}" fill-opacity="0.8" text-anchor="${anchor}">${esc(s.label)}</text>`;
  }
  return `<g>${body}${label}</g>`;
}

function wrap(text: string, max: number): string[] {
  const out: string[] = [];
  let line = "";
  for (const word of text.split(/\s+/).filter(Boolean)) {
    if ((line + " " + word).trim().length > max && line) { out.push(line); line = word; } else line = (line + " " + word).trim();
  }
  out.push(line);
  return out;
}

/** Renders the diagram as a self-contained SVG: zones and notes, cables, modules with their ports and icons. */
export function renderAssemblySvg(a: ApplicationAssembly, o: ImageOptions = {}): { svg: string; width: number; height: number } {
  const boxes = a.editor.nodes.map((n) => {
    const app = a.spec.modules.find((x) => x.id === n.id);
    return app ? { n, app, ...effectiveSize(n, app.ports, a.editor.density) } : null;
  }).filter((b): b is NonNullable<typeof b> => b !== null);
  const shapes = a.editor.shapes ?? [];
  const xs = [...boxes.flatMap((b) => [b.n.x, b.n.x + b.width]), ...shapes.flatMap((s) => [s.x, s.x + s.width])];
  const ys = [...boxes.flatMap((b) => [b.n.y, b.n.y + b.height]), ...shapes.flatMap((s) => [s.y, s.y + s.height])];
  const minX = xs.length ? Math.min(...xs) : 0, minY = ys.length ? Math.min(...ys) : 0;
  const ox = minX - PAD, oy = minY - PAD - (o.title ? 18 : 0);
  const width = Math.max(240, (xs.length ? Math.max(...xs) : 0) - minX + PAD * 2);
  const height = Math.max(160, (ys.length ? Math.max(...ys) : 0) - minY + PAD * 2 + (o.title ? 18 : 0));
  const pos = new Map(boxes.map((b) => [b.n.id, b]));
  const out: string[] = [];
  out.push(`<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}" viewBox="0 0 ${width} ${height}" font-family="Inter, 'Segoe UI', system-ui, sans-serif">`);
  out.push(`<title>${esc(a.metadata.name)}</title><rect width="100%" height="100%" fill="#ffffff"/>`);
  if (o.title) out.push(`<text x="${PAD}" y="${PAD - 10}" font-size="14" font-weight="600" fill="${INK}">${esc(a.metadata.name)}</text>`);
  for (const s of shapes) out.push(shapeSvg(s, ox, oy));

  const portY = (b: NonNullable<ReturnType<typeof pos.get>>, portId: string): number => {
    const ports = b.app.ports.filter((p) => p.direction === (b.app.ports.find((q) => q.id === portId)?.direction));
    return b.n.y - oy + portCenterY(Math.max(0, ports.findIndex((p) => p.id === portId)));
  };
  for (const c of a.spec.connections) {
    const s = pos.get(c.source.moduleId), t = pos.get(c.target.moduleId);
    if (!s || !t) continue;
    const sp = portOf(a, c.source);
    const sx = s.n.x - ox + s.width + 5, sy = portY(s, c.source.portId);
    const tx = t.n.x - ox - 5, ty = portY(t, c.target.portId);
    const color = flat(cableColor(sp?.type ?? "tcp"));
    let d: string;
    if (o.routing === "angled") { const mx = sx + Math.max(14, (tx - sx) / 2); d = `M ${sx} ${sy} H ${mx} V ${ty} H ${tx}`; }
    else { const k = Math.max(40, Math.abs(tx - sx) / 2); d = `M ${sx} ${sy} C ${sx + k} ${sy}, ${tx - k} ${ty}, ${tx} ${ty}`; }
    out.push(`<path d="${d}" fill="none" stroke="${color}" stroke-width="2" stroke-linecap="round"/><path d="M ${tx - 8} ${ty - 3} L ${tx - 1} ${ty} L ${tx - 8} ${ty + 3} Z" fill="${color}"/>`);
  }

  for (const b of boxes) {
    const x = b.n.x - ox, y = b.n.y - oy, cls: ModuleClass | undefined = lookupClass(b.app.classRef.name, b.app.classRef.version);
    const cat = flat(categoryColor(cls?.category ?? "Custom"));
    out.push(`<g><rect x="${x}" y="${y}" width="${b.width}" height="${b.height}" rx="8" fill="#ffffff" stroke="#d1d5db"/>`);
    out.push(`<path d="M ${x} ${y + NODE.header} V ${y + 8} Q ${x} ${y} ${x + 8} ${y} H ${x + b.width - 8} Q ${x + b.width} ${y} ${x + b.width} ${y + 8} V ${y + NODE.header} Z" fill="${cat}" fill-opacity="0.22"/>`);
    out.push(`<text x="${x + 8}" y="${y + 16}" font-size="11" font-weight="500" fill="${INK}">${esc(b.app.name.length > 18 ? b.app.name.slice(0, 17) + "…" : b.app.name)}</text>`);
    const icon = cls ? o.icons?.get(cls.iconAssetPath) : undefined;
    if (icon) out.push(`<image href="${icon}" x="${x + b.width / 2 - NODE.logo / 2}" y="${y + NODE.header + (b.height - NODE.header - NODE.logo) / 2 - 6}" width="${NODE.logo}" height="${NODE.logo}"/>`);
    out.push(`<text x="${x + b.width / 2}" y="${y + b.height - 7}" font-size="9" fill="${MUTED}" text-anchor="middle">${esc(cls?.displayName ?? b.app.classRef.name)}</text>`);
    for (const dir of ["input", "output"] as const) {
      b.app.ports.filter((p) => p.direction === dir).forEach((p, i) => {
        const cy = y + portCenterY(i), px = dir === "input" ? x : x + b.width;
        const tag = (a.spec.links ?? []).find((l) => l.endpoint.moduleId === b.app.id && l.endpoint.portId === p.id);
        const plugged = a.spec.connections.some((c) => (c.source.moduleId === b.app.id && c.source.portId === p.id) || (c.target.moduleId === b.app.id && c.target.portId === p.id));
        out.push(`<rect x="${px - 4.5}" y="${cy - 4.5}" width="9" height="9" rx="1.5" fill="${plugged || tag ? flat(cableColor(p.type)) : "#ffffff"}" stroke="#4b5563"/>`);
        out.push(`<text x="${dir === "input" ? x + 8 : x + b.width - 8}" y="${cy + 3}" font-size="9" fill="${MUTED}" text-anchor="${dir === "input" ? "start" : "end"}">${esc(p.name)}</text>`);
        if (tag) {
          const label = tag.tag, tw = label.length * 5.2 + 10, tx = dir === "input" ? px - 10 - tw : px + 10;
          const members = tagNet(a, tag.tag).length;
          out.push(`<rect x="${tx}" y="${cy - 7}" width="${tw}" height="14" rx="7" fill="${flat(tagColor(tag.tag))}"/><text x="${tx + tw / 2}" y="${cy + 3}" font-size="9" fill="#ffffff" text-anchor="middle">${esc(label)}${members > 2 ? ` ${members}` : ""}</text>`);
        }
      });
    }
    out.push("</g>");
  }
  out.push("</svg>");
  return { svg: out.join("\n"), width, height };
}

async function toDataUri(url: string): Promise<string | null> {
  try {
    const r = await fetch(url);
    if (!r.ok) return null;
    const b = await r.blob();
    return await new Promise<string>((resolve, reject) => { const f = new FileReader(); f.onload = () => resolve(String(f.result)); f.onerror = () => reject(f.error); f.readAsDataURL(b); });
  } catch { return null; }
}

/** Fetches the icons of every class the diagram uses and embeds them. */
export async function collectIcons(a: ApplicationAssembly): Promise<Map<string, string>> {
  const paths = new Set<string>();
  for (const app of a.spec.modules) { const c = lookupClass(app.classRef.name, app.classRef.version); if (c) paths.add(c.iconAssetPath); }
  const map = new Map<string, string>();
  await Promise.all([...paths].map(async (p) => { const u = logoUrl(p); const uri = u ? await toDataUri(u) : null; if (uri) map.set(p, uri); }));
  return map;
}

export async function assemblySvg(a: ApplicationAssembly, routing: "curved" | "angled"): Promise<{ svg: string; width: number; height: number }> {
  return renderAssemblySvg(a, { icons: await collectIcons(a), routing, title: true });
}

export async function svgToPng(svg: string, width: number, height: number, scale = 2): Promise<Blob> {
  const url = URL.createObjectURL(new Blob([svg], { type: "image/svg+xml" }));
  try {
    const img = new Image();
    img.decoding = "async";
    await new Promise<void>((resolve, reject) => { img.onload = () => resolve(); img.onerror = () => reject(new Error("The image could not be rendered.")); img.src = url; });
    const max = 8000;
    const k = Math.min(scale, max / width, max / height);
    const canvas = document.createElement("canvas");
    canvas.width = Math.max(1, Math.round(width * k));
    canvas.height = Math.max(1, Math.round(height * k));
    const ctx = canvas.getContext("2d");
    if (!ctx) throw new Error("Canvas is not available.");
    ctx.fillStyle = "#ffffff";
    ctx.fillRect(0, 0, canvas.width, canvas.height);
    ctx.drawImage(img, 0, 0, canvas.width, canvas.height);
    return await new Promise<Blob>((resolve, reject) => canvas.toBlob((b) => (b ? resolve(b) : reject(new Error("PNG encoding failed."))), "image/png"));
  } finally { URL.revokeObjectURL(url); }
}

export function saveBlob(blob: Blob, filename: string): void {
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = filename;
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

/** Opens the browser's print dialog for the diagram, in a throwaway frame so the app itself is not printed. */
export function printSvg(svg: string, title: string): void {
  const frame = document.createElement("iframe");
  frame.setAttribute("aria-hidden", "true");
  frame.style.cssText = "position:fixed;right:0;bottom:0;width:0;height:0;border:0";
  document.body.appendChild(frame);
  const doc = frame.contentDocument;
  if (!doc) { frame.remove(); return; }
  doc.open();
  doc.write(`<!doctype html><title>${esc(title)}</title><style>@page{size:landscape;margin:12mm}body{margin:0}svg{width:100%;height:auto}</style>${svg}`);
  doc.close();
  frame.contentWindow?.addEventListener("afterprint", () => frame.remove());
  setTimeout(() => { frame.contentWindow?.focus(); frame.contentWindow?.print(); }, 150);
  setTimeout(() => frame.remove(), 60_000);
}

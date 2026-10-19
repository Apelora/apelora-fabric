import { LABEL_POSITIONS, SHAPE_KINDS, SHAPE_PALETTE, STROKE_STYLES, shapeContents, type Shape } from "@apelora/assembly-model";
import { useStudio, useStudioApi } from "../../features/editor/context";
import { IconCircle, IconRounded, IconSquare, IconTrash, IconTriangle } from "../icons";

const KIND_LABEL: Record<Shape["kind"], string> = { rect: "Square", ellipse: "Circle", rounded: "Rounded", triangle: "Triangle" };
const KIND_ICON = { rect: IconSquare, ellipse: IconCircle, rounded: IconRounded, triangle: IconTriangle } as const;
const POS_LABEL: Record<Shape["labelPos"], string> = { "top-left": "Top left", top: "Top", center: "Center", bottom: "Bottom" };

/** Palette of colour swatches plus a custom picker. `value` may be empty when no colour is chosen. */
export function Swatches({ value, onPick, label, testId }: { value: string; onPick: (c: string) => void; label: string; testId: string }) {
  return (
    <div role="radiogroup" aria-label={label} className="flex flex-wrap items-center gap-1.5" data-testid={testId}>
      {SHAPE_PALETTE.map((p) => (
        <button key={p.color} type="button" role="radio" aria-checked={value.toLowerCase() === p.color} aria-label={p.name} title={p.name} data-color={p.color}
          className={`size-6 rounded-full border-2 transition-transform hover:scale-110 ${value.toLowerCase() === p.color ? "border-base-content" : "border-base-100 ring-1 ring-base-300"}`} style={{ background: p.color }} onClick={() => onPick(p.color)} />
      ))}
      <label className="relative size-6 cursor-pointer overflow-hidden rounded-full border border-dashed border-base-content/40" title="Custom colour">
        <span className="absolute inset-0 flex items-center justify-center text-[10px]">+</span>
        <input type="color" aria-label={`${label}: custom colour`} className="absolute inset-0 size-full cursor-pointer opacity-0" value={value || "#64748b"} onChange={(e) => onPick(e.target.value)} />
      </label>
    </div>
  );
}

function Slider({ label, value, min, max, step, unit = "", onChange, testId }: { label: string; value: number; min: number; max: number; step: number; unit?: string; onChange: (v: number) => void; testId: string }) {
  return (
    <label className="block">
      <span className="mb-0.5 flex justify-between text-[11px] font-semibold"><span>{label}</span><span className="font-normal tabular-nums text-base-content/60">{Math.round(value * 100) / 100}{unit}</span></span>
      <input type="range" className="range range-xs w-full" min={min} max={max} step={step} value={value} data-testid={testId} onChange={(e) => onChange(Number(e.target.value))} />
    </label>
  );
}

function Seg<T extends string>({ value, options, label, onPick, testId }: { value: T; options: readonly T[]; label: string; onPick: (v: T) => void; testId: string }) {
  return (
    <div role="radiogroup" aria-label={label} className="join w-full" data-testid={testId}>
      {options.map((o) => <button key={o} type="button" role="radio" aria-checked={value === o} data-value={o} className={`btn btn-xs join-item flex-1 capitalize ${value === o ? "btn-active" : ""}`} onClick={() => onPick(o)}>{(label === "Label position" ? POS_LABEL[o as Shape["labelPos"]] : o) as string}</button>)}
    </div>
  );
}

function Tabs({ tab, onTab }: { tab: string; onTab: (t: "style" | "label" | "layout") => void }) {
  return (
    <div role="tablist" className="tabs tabs-border px-3" data-testid="props-tabs">
      {(["style", "label", "layout"] as const).map((t) => <button key={t} role="tab" type="button" aria-selected={tab === t} data-testid={`props-tab-${t}`} className={`tab h-9 text-xs capitalize ${tab === t ? "tab-active" : ""}`} onClick={() => onTab(t)}>{t}</button>)}
    </div>
  );
}

/** Zone/sector editor: shape kind, colours, border, label and layout. Edits apply live and merge into one undo step per control. */
export function ShapeProperties({ id, tab }: { id: string; tab: "style" | "label" | "layout" }) {
  const api = useStudioApi();
  const assembly = useStudio((s) => s.assembly);
  const sh = (assembly.editor.shapes ?? []).find((x) => x.id === id);
  if (!sh) return null;
  const set = (patch: Partial<Omit<Shape, "id">>) => api.getState().editShape(id, patch);
  const inside = shapeContents(assembly, id).length;

  if (tab === "label") {
    return (
      <div className="flex flex-col gap-4" data-testid="shape-label-tab">
        <label className="fieldset"><span className="fieldset-legend text-[11px]">Label</span>
          <input className="input input-sm w-full" data-testid="shape-label-input" value={sh.label} maxLength={60} placeholder="e.g. DMZ, Zone A, Production" onChange={(e) => set({ label: e.target.value })} />
        </label>
        <div><span className="mb-1 block text-[11px] font-semibold">Label position</span><Seg value={sh.labelPos} options={LABEL_POSITIONS} label="Label position" testId="shape-label-pos" onPick={(v) => set({ labelPos: v })} /></div>
        <Slider label="Font size" value={sh.fontSize} min={8} max={36} step={1} unit=" px" onChange={(v) => set({ fontSize: v })} testId="shape-font-size" />
      </div>
    );
  }
  if (tab === "layout") {
    const num = (key: "x" | "y" | "width" | "height", label: string, min: number) => (
      <label className="fieldset min-w-0 flex-1"><span className="fieldset-legend text-[11px]">{label}</span>
        <input key={sh[key]} type="number" min={min} defaultValue={sh[key]} className="input input-sm w-full" data-testid={`shape-${key}`}
          onBlur={(e) => { const n = Number(e.currentTarget.value); if (Number.isFinite(n) && n >= min) { if (key === "width" || key === "height") api.getState().resizeShapeTransient(id, { width: key === "width" ? n : sh.width, height: key === "height" ? n : sh.height }); else set({ [key]: Math.round(n) }); } else e.currentTarget.value = String(sh[key]); }}
          onKeyDown={(e) => { if (e.key === "Enter") e.currentTarget.blur(); }} />
      </label>
    );
    return (
      <div className="flex flex-col gap-4" data-testid="shape-layout-tab">
        <div className="flex gap-2">{num("x", "X", -100000)}{num("y", "Y", -100000)}</div>
        <div className="flex gap-2">{num("width", "Width", 40)}{num("height", "Height", 30)}</div>
        <label className="flex items-center gap-2 text-xs"><input type="checkbox" className="toggle toggle-sm" data-testid="shape-locked" checked={sh.locked} onChange={(e) => set({ locked: e.target.checked })} />Lock position and size</label>
        <p className="text-xs text-base-content/60" data-testid="shape-contents">{inside} appliance{inside === 1 ? "" : "s"} inside. They move with the zone when you drag it.</p>
        <div className="flex gap-2">
          <button className="btn btn-sm btn-outline flex-1" data-testid="shape-front" onClick={() => api.getState().orderShapeAction(id, "front")}>Bring forward</button>
          <button className="btn btn-sm btn-outline flex-1" data-testid="shape-back" onClick={() => api.getState().orderShapeAction(id, "back")}>Send back</button>
        </div>
        <div className="flex gap-2">
          <button className="btn btn-sm btn-outline flex-1" data-testid="shape-duplicate" onClick={() => api.getState().duplicateShapeAction(id)}>Duplicate</button>
          <button className="btn btn-sm btn-outline" data-testid="shape-delete" onClick={() => api.getState().deleteShapes([id])}><IconTrash size={14} />Delete</button>
        </div>
      </div>
    );
  }
  return (
    <div className="flex flex-col gap-4" data-testid="shape-style-tab">
      <div><span className="mb-1 block text-[11px] font-semibold">Shape</span>
        <div role="radiogroup" aria-label="Shape" className="grid grid-cols-4 gap-1.5" data-testid="shape-kind">
          {SHAPE_KINDS.map((k) => { const I = KIND_ICON[k]; return (
            <button key={k} type="button" role="radio" aria-checked={sh.kind === k} data-kind={k} className={`flex flex-col items-center gap-1 rounded-box border px-1 py-1.5 text-[10px] ${sh.kind === k ? "border-base-content bg-base-200" : "border-base-300 hover:bg-base-200"}`} onClick={() => set({ kind: k, ...(k === "rounded" && sh.radius === 0 ? { radius: 18 } : {}) })}><I size={18} />{KIND_LABEL[k]}</button>
          ); })}
        </div>
      </div>
      <div><span className="mb-1 block text-[11px] font-semibold">Fill colour</span><Swatches value={sh.fill} onPick={(c) => set({ fill: c })} label="Fill colour" testId="shape-fill" /></div>
      <Slider label="Fill opacity" value={Math.round(sh.fillOpacity * 100)} min={0} max={100} step={1} unit=" %" onChange={(v) => set({ fillOpacity: v / 100 })} testId="shape-fill-opacity" />
      <div><span className="mb-1 block text-[11px] font-semibold">Border colour</span><Swatches value={sh.stroke} onPick={(c) => set({ stroke: c })} label="Border colour" testId="shape-stroke" /></div>
      <Slider label="Border width" value={sh.strokeWidth} min={0} max={8} step={0.5} unit=" px" onChange={(v) => set({ strokeWidth: v })} testId="shape-stroke-width" />
      <div><span className="mb-1 block text-[11px] font-semibold">Border style</span><Seg value={sh.strokeStyle} options={STROKE_STYLES} label="Border style" testId="shape-stroke-style" onPick={(v) => set({ strokeStyle: v })} /></div>
      {sh.kind === "rounded" && <Slider label="Corner radius" value={sh.radius} min={0} max={80} step={1} unit=" px" onChange={(v) => set({ radius: v })} testId="shape-radius" />}
    </div>
  );
}
ShapeProperties.Tabs = Tabs;

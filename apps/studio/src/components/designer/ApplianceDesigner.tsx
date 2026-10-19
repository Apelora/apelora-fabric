import { useEffect, useMemo, useState } from "react";
import { GENERIC_ICONS, logoUrl } from "@apelora/appliance-catalog";
import { CATEGORIES, NODE, PORT_TYPES, minApplianceHeight, portCenterY, type Category } from "@apelora/assembly-model";
import { classFromDraft, emptyProp, newPortKey, useCustomCatalog, type Draft, type DraftPort, type DraftProp } from "../../features/catalog/customCatalog";
import { useStudio, useStudioApi } from "../../features/editor/context";
import { Jack } from "../canvas/Jack";
import { IconPlus, IconTrash, IconUp, IconDown } from "../icons";

function Preview({ d }: { d: Draft }) {
  const inputs = d.ports.filter((p) => p.direction === "input");
  const outputs = d.ports.filter((p) => p.direction === "output");
  const h = Math.max(minApplianceHeight(inputs.length, outputs.length), NODE.height);
  const url = logoUrl(d.icon);
  const side = (list: DraftPort[], kind: "input" | "output") =>
    list.map((p, i) => (
      <div key={p.key} className="absolute" style={{ top: portCenterY(i) - 5, [kind === "input" ? "left" : "right"]: -5, width: 10, height: 10, color: "color-mix(in oklab, var(--color-base-content) 70%, transparent)" }}>
        <Jack kind={kind} />
        <span className="absolute top-1/2 -translate-y-1/2 overflow-hidden text-ellipsis whitespace-nowrap text-[9px] leading-none text-base-content/70" style={{ [kind === "input" ? "left" : "right"]: 14, maxWidth: 35, textAlign: kind === "input" ? "left" : "right" }}>{p.name || "…"}</span>
      </div>
    ));
  return (
    <div className="flex justify-center rounded-box border border-base-300 py-6" style={{ background: "var(--apl-canvas)" }} aria-label="Appliance preview">
      <div className="apl-node" style={{ width: NODE.width, height: h }} data-testid="designer-preview">
        <div className="apl-node-header flex items-center px-2 text-[11px] font-medium leading-none"><span className="truncate">{d.name.trim() || "new-appliance"}</span></div>
        <div className="flex items-center justify-center" style={{ height: h - NODE.header - 2 }}>{url && <img src={url} alt="" style={{ width: NODE.logo, height: NODE.logo }} />}</div>
        {side(inputs, "input")}
        {side(outputs, "output")}
      </div>
    </div>
  );
}

export function ApplianceDesigner() {
  const api = useStudioApi();
  const draft = useCustomCatalog((s) => s.draft);
  const classes = useCustomCatalog((s) => s.classes);
  const assembly = useStudio((s) => s.assembly);
  const [d, setD] = useState<Draft | null>(draft);
  const [errors, setErrors] = useState<string[]>([]);
  const [tab, setTab] = useState<"ports" | "config">("ports");
  useEffect(() => { setD(draft); setErrors([]); setTab("ports"); }, [draft]);
  const store = useCustomCatalog;

  const inUse = useMemo(() => !!d?.editingId && assembly.spec.appliances.some((a) => a.classRef.name === d.editingId), [d?.editingId, assembly]);
  const existing = d?.editingId ? classes.find((c) => c.id === d.editingId) : undefined;

  if (!d) return <dialog className="modal" data-testid="designer" />;
  const patch = (p: Partial<Draft>) => setD({ ...d, ...p });
  const setPort = (key: string, p: Partial<DraftPort>) => patch({ ports: d.ports.map((x) => (x.key === key ? { ...x, ...p } : x)) });
  const move = (key: string, delta: -1 | 1) => {
    const me = d.ports.find((p) => p.key === key)!;
    const side = d.ports.filter((p) => p.direction === me.direction);
    const i = side.findIndex((p) => p.key === key);
    const j = i + delta;
    if (j < 0 || j >= side.length) return;
    const re = [...side]; [re[i], re[j]] = [re[j]!, re[i]!];
    let k = 0;
    patch({ ports: d.ports.map((p) => (p.direction === me.direction ? re[k++]! : p)) });
  };
  const addPort = (direction: DraftPort["direction"]) => patch({ ports: [...d.ports, { key: newPortKey(), name: direction === "input" ? `in${d.ports.filter((p) => p.direction === "input").length + 1}` : `out${d.ports.filter((p) => p.direction === "output").length + 1}`, direction, type: "tcp", optional: true, multiplicity: "many", description: "" }] });

  const save = () => {
    const r = classFromDraft(d);
    if (!r.ok) { setErrors(r.errors); return; }
    store.getState().save(r.cls);
    store.getState().closeDesigner();
    api.getState().toast("success", `${existing ? "Updated" : "Created"} appliance “${r.cls.displayName}”. It is in the catalog under ${r.cls.category}.`);
  };
  const del = () => {
    if (!d.editingId) return;
    store.getState().remove(d.editingId);
    store.getState().closeDesigner();
    api.getState().toast("info", "Custom appliance deleted from the catalog.");
  };

  const section = (direction: DraftPort["direction"], title: string) => (
    <section aria-label={title}>
      <div className="mb-1 flex items-center justify-between">
        <h4 className="text-[11px] font-semibold uppercase tracking-wide text-base-content/60">{title}</h4>
        <button type="button" className="btn btn-ghost btn-xs" data-testid={`designer-add-${direction}`} onClick={() => addPort(direction)}><IconPlus size={12} />Add</button>
      </div>
      <ul className="flex flex-col gap-1.5">
        {d.ports.filter((p) => p.direction === direction).map((p, i, arr) => (
          <li key={p.key} className="rounded-box border border-base-300 p-1.5" data-testid="designer-port">
            <div className="flex items-center gap-1">
              <input className="input input-xs min-w-0 flex-1" aria-label="Port name" placeholder="short name, e.g. db" maxLength={48} value={p.name} onChange={(e) => setPort(p.key, { name: e.target.value })} />
              <select className="select select-xs w-24" aria-label="Port type" value={p.type} onChange={(e) => setPort(p.key, { type: e.target.value as DraftPort["type"] })}>{PORT_TYPES.map((t) => <option key={t}>{t}</option>)}</select>
              <button type="button" className="btn btn-ghost btn-xs btn-square" aria-label="Move up" disabled={i === 0} onClick={() => move(p.key, -1)}><IconUp size={12} /></button>
              <button type="button" className="btn btn-ghost btn-xs btn-square" aria-label="Move down" disabled={i === arr.length - 1} onClick={() => move(p.key, 1)}><IconDown size={12} /></button>
              <button type="button" className="btn btn-ghost btn-xs btn-square" aria-label="Delete port" onClick={() => patch({ ports: d.ports.filter((x) => x.key !== p.key) })}><IconTrash size={12} /></button>
            </div>
            <div className="mt-1 flex items-center gap-2">
              <input className="input input-xs min-w-0 flex-1" aria-label="Port description" placeholder="what it carries (tooltip)" maxLength={160} value={p.description} onChange={(e) => setPort(p.key, { description: e.target.value })} />
              <label className="flex items-center gap-1 text-[11px]"><input type="checkbox" className="checkbox checkbox-xs" checked={!p.optional} onChange={(e) => setPort(p.key, { optional: !e.target.checked })} />required</label>
              <label className="flex items-center gap-1 text-[11px]"><input type="checkbox" className="checkbox checkbox-xs" checked={p.multiplicity === "many"} onChange={(e) => setPort(p.key, { multiplicity: e.target.checked ? "many" : "one" })} />many</label>
            </div>
          </li>
        ))}
        {d.ports.filter((p) => p.direction === direction).length === 0 && <li className="text-xs text-base-content/50">None.</li>}
      </ul>
    </section>
  );

  const setProp = (key: string, p: Partial<DraftProp>) => patch({ properties: d.properties.map((x) => (x.key === key ? { ...x, ...p } : x)) });
  const configSection = (
    <section aria-label="Configuration fields" data-testid="designer-config">
      <div className="mb-1 flex items-center justify-between">
        <p className="text-[11px] text-base-content/60">Fields users fill in on each instance (shown in the inspector). Secrets hold a reference name only.</p>
        <button type="button" className="btn btn-ghost btn-xs" data-testid="designer-add-prop" onClick={() => patch({ properties: [...d.properties, emptyProp()] })}><IconPlus size={12} />Add field</button>
      </div>
      <ul className="flex flex-col gap-1.5">
        {d.properties.map((p) => (
          <li key={p.key} className="rounded-box border border-base-300 p-1.5" data-testid="designer-prop">
            <div className="flex items-center gap-1">
              <input className="input input-xs min-w-0 flex-1" aria-label="Field label" placeholder="label, e.g. Max connections" maxLength={48} value={p.label} onChange={(e) => setProp(p.key, { label: e.target.value })} />
              <select className="select select-xs w-24" aria-label="Field type" value={p.type} onChange={(e) => setProp(p.key, { type: e.target.value as DraftProp["type"] })}>{["string", "int", "bool", "enum", "secret"].map((t) => <option key={t}>{t}</option>)}</select>
              <button type="button" className="btn btn-ghost btn-xs btn-square" aria-label="Delete field" onClick={() => patch({ properties: d.properties.filter((x) => x.key !== p.key) })}><IconTrash size={12} /></button>
            </div>
            <div className="mt-1 grid grid-cols-2 gap-1">
              {p.type === "bool"
                ? <select className="select select-xs" aria-label="Default" value={p.default} onChange={(e) => setProp(p.key, { default: e.target.value })}><option value="">(no default)</option><option value="true">on</option><option value="false">off</option></select>
                : <input className="input input-xs" aria-label="Default value" placeholder={p.type === "secret" ? "default reference name" : "default value"} value={p.default} onChange={(e) => setProp(p.key, { default: e.target.value })} />}
              <input className="input input-xs" aria-label="Group" placeholder="group (optional)" maxLength={32} value={p.group} onChange={(e) => setProp(p.key, { group: e.target.value })} />
              {p.type === "enum" && <input className="input input-xs col-span-2" aria-label="Options" placeholder="options, comma separated: a, b, c" value={p.options} onChange={(e) => setProp(p.key, { options: e.target.value })} />}
              {p.type === "int" && <><input className="input input-xs" aria-label="Minimum" placeholder="min" value={p.min} onChange={(e) => setProp(p.key, { min: e.target.value })} /><input className="input input-xs" aria-label="Maximum" placeholder="max" value={p.max} onChange={(e) => setProp(p.key, { max: e.target.value })} /></>}
              <input className="input input-xs col-span-2" aria-label="Description" placeholder="help text (optional)" maxLength={160} value={p.description} onChange={(e) => setProp(p.key, { description: e.target.value })} />
            </div>
            <label className="mt-1 flex items-center gap-1 text-[11px]"><input type="checkbox" className="checkbox checkbox-xs" checked={p.required} onChange={(e) => setProp(p.key, { required: e.target.checked })} />required</label>
          </li>
        ))}
        {d.properties.length === 0 && <li className="text-xs text-base-content/50">No configuration fields yet.</li>}
      </ul>
    </section>
  );

  return (
    <dialog className="modal modal-open" data-testid="designer" aria-labelledby="designer-title">
      <div className="modal-box flex max-h-[92vh] w-full max-w-4xl flex-col p-0">
        <div className="border-b border-base-300 px-5 py-3">
          <h3 id="designer-title" className="text-base font-semibold">{existing ? "Edit custom appliance" : "New custom appliance"}</h3>
          <p className="text-xs text-base-content/60">Define the appliance's terminals (typed ports), resources and look. It is saved in this browser and embedded in exported assemblies. Nothing is deployed.</p>
        </div>
        <div className="grid min-h-0 flex-1 gap-5 overflow-y-auto p-5 md:grid-cols-[minmax(0,1fr)_minmax(0,1.2fr)]">
          <div className="flex flex-col gap-3">
            <label className="fieldset"><span className="fieldset-legend text-[11px]">Name</span><input className="input input-sm w-full" data-testid="designer-name" maxLength={48} value={d.name} placeholder="e.g. Session cache" onChange={(e) => patch({ name: e.target.value })} /></label>
            <label className="fieldset"><span className="fieldset-legend text-[11px]">Category</span>
              <select className="select select-sm w-full" value={d.category} onChange={(e) => patch({ category: e.target.value as Category })}>{CATEGORIES.map((c) => <option key={c}>{c}</option>)}</select></label>
            <label className="fieldset"><span className="fieldset-legend text-[11px]">Summary</span><input className="input input-sm w-full" maxLength={200} value={d.summary} onChange={(e) => patch({ summary: e.target.value })} /></label>
            <div className="flex gap-2">
              <label className="fieldset min-w-0 flex-1"><span className="fieldset-legend text-[11px]">vCPU</span><input type="number" min={1} className="input input-sm w-full" value={d.cpu} onChange={(e) => patch({ cpu: Number(e.target.value) })} /></label>
              <label className="fieldset min-w-0 flex-1"><span className="fieldset-legend text-[11px]">Memory MiB</span><input type="number" min={64} step={64} className="input input-sm w-full" value={d.memoryMiB} onChange={(e) => patch({ memoryMiB: Number(e.target.value) })} /></label>
            </div>
            <fieldset>
              <legend className="mb-1 text-[11px] font-semibold">Icon</legend>
              <div className="grid grid-cols-5 gap-1.5" role="radiogroup" aria-label="Icon">
                {GENERIC_ICONS.map((i) => (
                  <button key={i} type="button" role="radio" aria-checked={d.icon === i} aria-label={i.replace("generic-", "")} className={`flex h-9 items-center justify-center rounded-box border ${d.icon === i ? "border-base-content bg-base-200" : "border-base-300 hover:bg-base-200"}`} onClick={() => patch({ icon: i })}>
                    <img src={logoUrl(i)} alt="" className="size-5" />
                  </button>
                ))}
              </div>
              <p className="mt-1 text-[11px] text-base-content/50">Generic icons only; vendor logos are reserved for catalog appliances.</p>
            </fieldset>
            <Preview d={d} />
          </div>
          <div className="flex flex-col gap-3">
            <div role="tablist" className="tabs tabs-border">
              <button role="tab" type="button" aria-selected={tab === "ports"} className={`tab ${tab === "ports" ? "tab-active" : ""}`} data-testid="designer-tab-ports" onClick={() => setTab("ports")}>Ports ({d.ports.length})</button>
              <button role="tab" type="button" aria-selected={tab === "config"} className={`tab ${tab === "config" ? "tab-active" : ""}`} data-testid="designer-tab-config" onClick={() => setTab("config")}>Configuration ({d.properties.length})</button>
            </div>
            {tab === "ports" ? <div className="flex flex-col gap-4">{section("input", "Inputs (left jacks)")}{section("output", "Outputs (right plugs)")}</div> : configSection}
          </div>
        </div>
        {errors.length > 0 && <ul className="mx-5 mb-2 list-disc rounded-box border border-error/40 bg-error/5 py-2 pl-7 pr-3 text-xs text-error" role="alert" data-testid="designer-errors">{errors.map((e, i) => <li key={i}>{e}</li>)}</ul>}
        <div className="flex items-center gap-2 border-t border-base-300 px-5 py-3">
          {existing && <button type="button" className="btn btn-sm btn-ghost text-error" data-testid="designer-delete" disabled={inUse} title={inUse ? "In use by the current assembly" : "Delete this appliance from the catalog"} onClick={del}><IconTrash size={14} />Delete</button>}
          <div className="ml-auto flex gap-2">
            <button type="button" className="btn btn-sm" onClick={() => store.getState().closeDesigner()}>Cancel</button>
            <button type="button" className="btn btn-sm btn-primary" data-testid="designer-save" onClick={save}>{existing ? "Save changes" : "Create appliance"}</button>
          </div>
        </div>
      </div>
      <div className="modal-backdrop" onClick={() => store.getState().closeDesigner()} />
    </dialog>
  );
}

import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { effectiveSize, portOf, tagNet, type ApplicationAssembly } from "@apelora/assembly-model";
import { lookupClass } from "@apelora/appliance-catalog";
import { useStudio, useStudioApi } from "../../features/editor/context";
import { diagnosticsFor, type PropsTarget } from "../../features/editor/store";
import { draftFromPorts, useCustomCatalog } from "../../features/catalog/customCatalog";
import { categoryColor } from "../../features/editor/categories";
import { IconBox, IconTrash, IconX } from "../icons";
import { ConfigForm } from "./ConfigForm";
import { NumberField, PortSection, nicStatus } from "./PropertiesInspector";
import { ShapeProperties } from "./ShapeProperties";

const W = 420;

type Api = ReturnType<typeof useStudioApi>;

function targetRect(t: PropsTarget): DOMRect | null {
  const sel = t.kind === "appliance" ? `[data-testid=appliance-node][data-node-id="${t.id}"]` : t.kind === "shape" ? `[data-testid=shape-node][data-shape-id="${t.id}"]` : null;
  return sel ? document.querySelector(sel)?.getBoundingClientRect() ?? null : null;
}

function Section({ title, children, right }: { title: string; children: React.ReactNode; right?: React.ReactNode }) {
  return (
    <section>
      <div className="mb-1.5 flex items-center justify-between"><h4 className="text-[11px] font-semibold uppercase tracking-wide text-base-content/60">{title}</h4>{right}</div>
      {children}
    </section>
  );
}

function ApplianceBody({ id, tab }: { id: string; tab: "general" | "config" | "ports" | "issues" }) {
  const api = useStudioApi();
  const assembly = useStudio((s) => s.assembly);
  const diagnostics = useMemo(() => diagnosticsFor(assembly), [assembly]);
  const app = assembly.spec.appliances.find((a) => a.id === id);
  const layout = assembly.editor.nodes.find((n) => n.id === id);
  if (!app || !layout) return null;
  const cls = lookupClass(app.classRef.name, app.classRef.version);
  const size = effectiveSize(layout, app.ports, assembly.editor.density);
  const mine = diagnostics.filter((d) => d.applianceId === app.id);

  if (tab === "config") return cls?.properties?.length ? <ConfigForm app={app} /> : <p className="text-xs text-base-content/60" data-testid="no-config">This appliance has no configuration fields.</p>;
  if (tab === "ports") return <div className="flex flex-col gap-4" data-testid="ports-tab"><PortSection appId={app.id} ports={app.ports} direction="input" /><PortSection appId={app.id} ports={app.ports} direction="output" /></div>;
  if (tab === "issues") {
    return mine.length === 0 ? <p className="text-xs text-base-content/60" data-testid="no-issues">No issues for this appliance.</p> : (
      <ul className="flex flex-col gap-2" data-testid="inspector-diagnostics">{mine.map((d, i) => <li key={i} className="text-xs"><span className={`badge badge-xs mr-1 ${d.severity === "error" ? "badge-error" : "badge-warning"}`}>{d.severity}</span>{d.message}{d.hint && <span className="block pl-1 text-base-content/60">{d.hint}</span>}</li>)}</ul>
    );
  }
  return (
    <div className="flex flex-col gap-4" data-testid="inspector-appliance">
      <label className="fieldset">
        <span className="fieldset-legend text-[11px]">Instance name</span>
        <input key={app.name} defaultValue={app.name} data-testid="instance-name" className="input input-sm w-full" onBlur={(e) => api.getState().rename(app.id, e.currentTarget.value)} onKeyDown={(e) => { if (e.key === "Enter") e.currentTarget.blur(); }} />
      </label>
      <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5 text-xs">
        <dt className="text-base-content/60">Class</dt><dd className="truncate">{cls?.displayName ?? app.classRef.name} <span className="text-base-content/50">v{app.classRef.version}</span></dd>
        <dt className="text-base-content/60">Id</dt><dd className="truncate font-mono">{app.id}</dd>
        <dt className="text-base-content/60">Network</dt><dd className="truncate" data-testid="nic-status">{nicStatus(assembly, app.id)}</dd>
        <dt className="text-base-content/60">Size</dt><dd className="tabular-nums" data-testid="node-size">{size.width} × {size.height}{layout.custom && <button className="link ml-2" onClick={() => api.getState().resetSize(app.id)}>reset</button>}</dd>
      </dl>
      <Section title="Resources (request)">
        <div className="flex gap-2">
          <NumberField label="vCPU" value={app.resources.cpu} min={1} step={1} testId="res-cpu" onCommit={(n) => api.getState().setRes(app.id, { cpu: n })} />
          <NumberField label="Memory MiB" value={app.resources.memoryMiB} min={64} step={64} testId="res-mem" onCommit={(n) => api.getState().setRes(app.id, { memoryMiB: n })} />
        </div>
      </Section>
      {cls?.composition && (
        <Section title={`Contents (${cls.composition.appliances.length})`} right={<button className="btn btn-ghost btn-xs" data-testid="ungroup" onClick={() => api.getState().ungroupSelection(app.id)}>Ungroup</button>}>
          <div data-testid="component-contents">
            <ul className="flex flex-col gap-0.5 text-xs">{cls.composition.appliances.map((x) => <li key={x.id} className="truncate">{x.name} <span className="text-base-content/50">· {lookupClass(x.classRef.name, x.classRef.version)?.displayName ?? x.classRef.name}</span></li>)}</ul>
            <p className="mt-1 text-[10px] text-base-content/50">Its <span className="font-mono">net</span> plugs every inner appliance into the network.</p>
          </div>
        </Section>
      )}
      <Section title="Health policy"><p className="text-xs text-base-content/50">Placeholder: health checks are defined in a later milestone.</p></Section>
      <div className="flex gap-2">
        <button className="btn btn-sm btn-outline flex-1" data-testid="save-as-class" title="Turn this appliance, with its ports, into a reusable catalog appliance" disabled={!!cls?.composition} onClick={() => useCustomCatalog.getState().openDesigner(draftFromPorts(app.name, app.ports, app.resources, cls?.iconAssetPath ?? "generic-box", cls?.properties, app.config))}><IconBox size={14} />Save as custom appliance…</button>
        <button className="btn btn-sm btn-outline" data-testid="delete-appliance" onClick={() => { api.getState().deleteElements([app.id], []); }}><IconTrash size={14} />Delete</button>
      </div>
    </div>
  );
}

function CableBody({ id, api, assembly }: { id: string; api: Api; assembly: ApplicationAssembly }) {
  const conn = assembly.spec.connections.find((c) => c.id === id);
  if (!conn) return null;
  const sa = assembly.spec.appliances.find((a) => a.id === conn.source.applianceId);
  const ta = assembly.spec.appliances.find((a) => a.id === conn.target.applianceId);
  const sp = sa?.ports.find((p) => p.id === conn.source.portId);
  const tp = ta?.ports.find((p) => p.id === conn.target.portId);
  return (
    <div className="flex flex-col gap-3 text-xs" data-testid="inspector-connection">
      <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5">
        <dt className="text-base-content/60">From</dt><dd>{sa?.name} · {sp?.name} <span className="text-base-content/50">({sp?.type})</span></dd>
        <dt className="text-base-content/60">To</dt><dd>{ta?.name} · {tp?.name} <span className="text-base-content/50">({tp?.type})</span></dd>
        <dt className="text-base-content/60">Protocol</dt><dd>{sp?.type}</dd>
        <dt className="text-base-content/60">Id</dt><dd className="font-mono">{conn.id}</dd>
      </dl>
      <div className="flex gap-2">
        <button className="btn btn-sm btn-outline flex-1" data-testid="cable-to-tag" onClick={() => { api.getState().cableToTagAction(conn.id); }}>Convert to link tag</button>
        <button className="btn btn-sm btn-outline" data-testid="delete-connection" onClick={() => api.getState().deleteElements([], [conn.id])}><IconTrash size={14} />Unplug</button>
      </div>
    </div>
  );
}

function TagBody({ name, api, assembly }: { name: string; api: Api; assembly: ApplicationAssembly }) {
  const members = tagNet(assembly, name);
  if (members.length === 0) return null;
  const outs = members.filter((m) => portOf(assembly, m.endpoint)?.direction === "output").length;
  return (
    <div className="flex flex-col gap-3 text-xs" data-testid="inspector-tag">
      <label className="fieldset"><span className="fieldset-legend text-[11px]">Link tag</span>
        <input key={name} defaultValue={name} data-testid="tag-name" className="input input-sm w-full font-mono" maxLength={32}
          onBlur={(e) => { const v = e.currentTarget.value.trim().toLowerCase(); if (v && v !== name && !api.getState().renameTagAction(name, v)) e.currentTarget.value = name; }}
          onKeyDown={(e) => { if (e.key === "Enter") e.currentTarget.blur(); }} />
      </label>
      <p className="text-base-content/60">Terminals with the same name are linked without a drawn cable. {members.length} terminals: {outs} output{outs === 1 ? "" : "s"}, {members.length - outs} input{members.length - outs === 1 ? "" : "s"}.</p>
      <ul className="flex flex-col gap-1" data-testid="tag-members">
        {members.map((m) => { const a = assembly.spec.appliances.find((x) => x.id === m.endpoint.applianceId); const p = portOf(assembly, m.endpoint); return (
          <li key={m.id}><button type="button" className="btn btn-ghost btn-xs w-full justify-start font-normal" onClick={() => api.getState().selectNode(m.endpoint.applianceId)}><span className="badge badge-xs">{p?.direction === "output" ? "out" : "in"}</span>{a?.name}.{p?.name} <span className="text-base-content/50">({p?.type})</span></button></li>
        ); })}
      </ul>
      <div className="flex gap-2">
        <button className="btn btn-sm btn-outline flex-1" data-testid="tag-to-cable" disabled={!(outs === 1 && members.length - outs === 1)} title="Only a tag with one output and one input can become a cable" onClick={() => api.getState().tagToCableAction(name)}>Convert to cable</button>
        <button className="btn btn-sm btn-outline" data-testid="tag-remove" onClick={() => api.getState().removeTagAction(name)}><IconTrash size={14} />Remove tag</button>
      </div>
    </div>
  );
}

function AssemblyBody({ assembly }: { assembly: ApplicationAssembly }) {
  const d = useMemo(() => diagnosticsFor(assembly), [assembly]);
  const errors = d.filter((x) => x.severity === "error").length;
  return (
    <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5 text-xs" data-testid="inspector-assembly">
      <dt className="text-base-content/60">Assembly</dt><dd className="truncate">{assembly.metadata.name}</dd>
      <dt className="text-base-content/60">Appliances</dt><dd>{assembly.spec.appliances.length}</dd>
      <dt className="text-base-content/60">Connections</dt><dd>{assembly.spec.connections.length}</dd>
      <dt className="text-base-content/60">Link tags</dt><dd>{(assembly.spec.links ?? []).length}</dd>
      <dt className="text-base-content/60">Zones and shapes</dt><dd>{(assembly.editor.shapes ?? []).length}</dd>
      <dt className="text-base-content/60">Issues</dt><dd>{errors} errors, {d.length - errors} warnings</dd>
      <dt className="text-base-content/60">Format</dt><dd className="truncate font-mono">{assembly.apiVersion}</dd>
    </dl>
  );
}

const TABS: { id: "general" | "config" | "ports" | "issues"; label: string }[] = [
  { id: "general", label: "General" }, { id: "config", label: "Configuration" }, { id: "ports", label: "Ports" }, { id: "issues", label: "Issues" },
];

/** Floating, draggable properties window opened from the context menu, a double-click, or Enter. It does not block the canvas. */
export function PropertiesWindow() {
  const api = useStudioApi();
  const win = useStudio((s) => s.propsWin);
  const savedPos = useStudio((s) => s.propsPos);
  const assembly = useStudio((s) => s.assembly);
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ x: number; y: number } | null>(null);
  const key = win ? `${win.kind}:${"id" in win ? win.id : ""}` : "";

  // Place near the target the first time it opens (or at the position the user last dragged it to).
  useLayoutEffect(() => {
    if (!win) { setPos(null); return; }
    if (savedPos) { setPos(savedPos); return; }
    const r = targetRect(win);
    const w = Math.min(W, window.innerWidth - 16);
    let x = r ? r.right + 16 : window.innerWidth - w - 24;
    if (x + w > window.innerWidth - 8) x = r ? Math.max(8, r.left - w - 16) : 8;
    setPos({ x: Math.max(8, x), y: Math.max(56, Math.min(r ? r.top - 24 : 90, window.innerHeight - 460)) });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);

  // Close when the thing it shows disappears, or on Escape.
  useEffect(() => {
    if (!win) return;
    const gone =
      (win.kind === "appliance" && !assembly.spec.appliances.some((a) => a.id === win.id)) ||
      (win.kind === "cable" && !assembly.spec.connections.some((c) => c.id === win.id)) ||
      (win.kind === "tag" && tagNet(assembly, win.id).length === 0) ||
      (win.kind === "shape" && !(assembly.editor.shapes ?? []).some((x) => x.id === win.id));
    if (gone) api.getState().closeProps();
  }, [win, assembly, api]);
  useEffect(() => {
    if (!win) return;
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape" && !api.getState().menu && !api.getState().groupDialog && !api.getState().importErrors && !useCustomCatalog.getState().draft) api.getState().closeProps(); };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [win, api]);

  if (!win || !pos) return null;

  const drag = (e: React.PointerEvent) => {
    if ((e.target as HTMLElement).closest("button")) return;
    const start = { x: e.clientX - pos.x, y: e.clientY - pos.y };
    const move = (ev: PointerEvent) => setPos({ x: Math.max(0, Math.min(window.innerWidth - 120, ev.clientX - start.x)), y: Math.max(0, Math.min(window.innerHeight - 60, ev.clientY - start.y)) });
    const up = (ev: PointerEvent) => {
      window.removeEventListener("pointermove", move); window.removeEventListener("pointerup", up);
      api.getState().setPropsPos({ x: Math.max(0, Math.min(window.innerWidth - 120, ev.clientX - start.x)), y: Math.max(0, Math.min(window.innerHeight - 60, ev.clientY - start.y)) });
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  let title = "Properties";
  let subtitle = "";
  let accent: string | undefined;
  let body: React.ReactNode = null;
  let tabs: React.ReactNode = null;

  if (win.kind === "appliance") {
    const app = assembly.spec.appliances.find((a) => a.id === win.id);
    const cls = app ? lookupClass(app.classRef.name, app.classRef.version) : undefined;
    const tab = win.tab ?? "general";
    const nIssues = diagnosticsFor(assembly).filter((d) => d.applianceId === win.id).length;
    title = app?.name ?? win.id;
    subtitle = `${cls?.displayName ?? app?.classRef.name ?? ""} · ${win.id}`;
    accent = cls ? categoryColor(cls.category) : undefined;
    tabs = (
      <div role="tablist" className="tabs tabs-border px-3" data-testid="props-tabs">
        {TABS.map((t) => (
          <button key={t.id} role="tab" type="button" aria-selected={tab === t.id} data-testid={`props-tab-${t.id}`} className={`tab h-9 text-xs ${tab === t.id ? "tab-active" : ""}`} onClick={() => api.getState().setPropsTab(t.id)}>
            {t.label}{t.id === "config" && cls?.properties?.length ? ` (${cls.properties.length})` : ""}{t.id === "ports" && app ? ` (${app.ports.length})` : ""}{t.id === "issues" && nIssues > 0 ? <span className="badge badge-error badge-xs ml-1">{nIssues}</span> : null}
          </button>
        ))}
      </div>
    );
    body = <ApplianceBody id={win.id} tab={tab} />;
  } else if (win.kind === "cable") { title = "Cable"; subtitle = win.id; body = <CableBody id={win.id} api={api} assembly={assembly} />; }
  else if (win.kind === "tag") { title = `Link tag “${win.id}”`; body = <TagBody name={win.id} api={api} assembly={assembly} />; }
  else if (win.kind === "assembly") { title = "Assembly"; subtitle = assembly.metadata.name; body = <AssemblyBody assembly={assembly} />; }
  else if (win.kind === "shape") {
    const sh = (assembly.editor.shapes ?? []).find((x) => x.id === win.id);
    title = sh?.label || "Zone"; subtitle = `${sh?.kind ?? ""} · ${win.id}`;
    tabs = <ShapeProperties.Tabs tab={win.tab ?? "style"} onTab={(t) => api.getState().setPropsTab(t)} />;
    body = <ShapeProperties id={win.id} tab={win.tab ?? "style"} />;
  }

  return (
    <div
      ref={ref}
      role="dialog"
      aria-label={`Properties: ${title}`}
      data-testid="inspector"
      data-kind={win.kind}
      className="apl-props fixed z-40 flex flex-col overflow-hidden rounded-box border border-base-300 bg-base-100 shadow-xl"
      style={{ left: pos.x, top: pos.y, width: `min(${W}px, calc(100vw - 16px))`, maxHeight: `min(720px, calc(100vh - ${Math.round(pos.y)}px - 12px))` }}
    >
      <div className="flex cursor-move select-none items-center gap-2 border-b border-base-300 px-3 py-2" onPointerDown={drag} data-testid="props-header" style={accent ? { boxShadow: `inset 0 3px 0 ${accent}` } : undefined}>
        <div className="min-w-0 flex-1">
          <h2 className="truncate text-sm font-semibold" data-testid="props-title">{title}</h2>
          {subtitle && <p className="truncate font-mono text-[10px] text-base-content/50">{subtitle}</p>}
        </div>
        <button type="button" className="btn btn-ghost btn-xs btn-square" aria-label="Close properties" data-testid="props-close" onClick={() => api.getState().closeProps()}><IconX size={14} /></button>
      </div>
      {tabs}
      <div className="min-h-0 flex-1 overflow-y-auto p-4" data-testid="props-body">{body}</div>
    </div>
  );
}

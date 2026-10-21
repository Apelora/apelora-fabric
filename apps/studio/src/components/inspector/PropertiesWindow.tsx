import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { ModuleInstanceSchema, MAX_VOLUMES, MAX_REPLICAS, PLATFORMS, PLATFORM_INFO, assemblyTotals, availabilityOf, cableColorOf, effectiveSize, formatMemory, platformOf, portOf, replicasOf, tagNet, volumesOf, type ModuleClass, type ApplicationAssembly, type InternetAccess, type PlatformId, type Volume } from "@apelora/assembly-model";
import { lookupClass } from "@apelora/module-catalog";
import { useStudio, useStudioApi } from "../../features/editor/context";
import { diagnosticsFor, type PropsTarget } from "../../features/editor/store";
import { draftFromPorts, useCustomCatalog } from "../../features/catalog/customCatalog";
import { categoryColor } from "../../features/editor/categories";
import { IconBox, IconTrash, IconX } from "../icons";
import { ConfigForm } from "./ConfigForm";
import { MetricsSection } from "./MetricsSection";
import { NumberField, PortSection, nicStatus } from "./PropertiesInspector";
import { ShapeProperties, Swatches } from "./ShapeProperties";

const W = 420;

type Api = ReturnType<typeof useStudioApi>;

function targetRect(t: PropsTarget): DOMRect | null {
  const sel = t.kind === "module" ? `[data-testid=module-node][data-node-id="${t.id}"]` : t.kind === "shape" ? `[data-testid=shape-node][data-shape-id="${t.id}"]` : null;
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

/** A volume name not used yet: `data`, then `data-2`… */
const freeVolumeName = (vs: Volume[]) => {
  const names = new Set(vs.map((v) => v.name));
  for (let i = 1; ; i++) { const n = i === 1 ? "data" : `data-${i}`; if (!names.has(n)) return n; }
};

/**
 * The module's persistent volumes. Every change is checked against the schema before it is
 * kept; an invalid one is refused with the reason and the field goes back to its value.
 */
function VolumesSection({ appId, cls, volumes }: { appId: string; cls: ModuleClass; volumes: Volume[] }) {
  const api = useStudioApi();
  const [error, setError] = useState<string | null>(null);
  const stateless = availabilityOf(cls) === "stateless";
  const commit = (next: Volume[]): boolean => {
    const r = ModuleInstanceSchema.shape.volumes.safeParse(next);
    if (!r.success) { setError(r.error.issues[0]?.message ?? "invalid volume"); return false; }
    setError(null);
    api.getState().setModuleVolumes(appId, next);
    return true;
  };
  const change = (i: number, patch: Partial<Volume>) => commit(volumes.map((v, j) => (j === i ? { ...v, ...patch } : v)));
  if (stateless) {
    return <Section title="Volumes"><p className="text-xs text-base-content/60" data-testid="volumes-stateless">Stateless modules keep no data: their replicas can be recreated on any server.</p></Section>;
  }
  return (
    <Section title={`Volumes (${volumes.length})`} right={volumes.length < MAX_VOLUMES ? (
      <button className="btn btn-ghost btn-xs" data-testid="add-volume" onClick={() => commit([...volumes, { name: freeVolumeName(volumes), sizeMiB: 1024, path: `/srv/${freeVolumeName(volumes)}` }])}>Add volume</button>
    ) : undefined}>
      <div data-testid="volumes">
        {volumes.length === 0 ? <p className="text-xs text-base-content/60">None. Add one for data that must survive restarts and redeployments.</p> : (
          <ul className="flex flex-col gap-1.5">
            {volumes.map((v, i) => (
              <li key={`${i}-${v.name}`} className="rounded-box border border-base-300 p-1.5" data-testid="volume-row">
                <div className="flex items-center gap-1">
                  <input key={v.name} defaultValue={v.name} aria-label={`Name of volume ${i + 1}`} className="input input-xs min-w-0 flex-1 font-mono" data-testid="volume-name"
                    onBlur={(e) => { const n = e.currentTarget.value.trim(); if (n !== v.name && !change(i, { name: n })) e.currentTarget.value = v.name; }}
                    onKeyDown={(e) => { if (e.key === "Enter") e.currentTarget.blur(); }} />
                  <button className="btn btn-ghost btn-xs" aria-label={`Remove volume ${v.name}`} data-testid="remove-volume" onClick={() => commit(volumes.filter((_, j) => j !== i))}><IconTrash size={12} /></button>
                </div>
                <div className="mt-1 flex gap-1">
                  <input key={v.path} defaultValue={v.path} aria-label={`Mount path of ${v.name}`} className="input input-xs min-w-0 flex-[2] font-mono" data-testid="volume-path"
                    onBlur={(e) => { const p = e.currentTarget.value.trim(); if (p !== v.path && !change(i, { path: p })) e.currentTarget.value = v.path; }}
                    onKeyDown={(e) => { if (e.key === "Enter") e.currentTarget.blur(); }} />
                  <label className="flex min-w-0 flex-1 items-center gap-1 text-[11px] text-base-content/70">
                    <input key={v.sizeMiB} type="number" min={1} step={1} defaultValue={v.sizeMiB / 1024} aria-label={`Size of ${v.name} in GiB`} className="input input-xs min-w-0 flex-1 tabular-nums" data-testid="volume-size"
                      onBlur={(e) => { const g = Number(e.currentTarget.value); const mib = Math.round(g * 1024); if (mib !== v.sizeMiB && (!Number.isFinite(g) || !change(i, { sizeMiB: mib }))) e.currentTarget.value = String(v.sizeMiB / 1024); }}
                      onKeyDown={(e) => { if (e.key === "Enter") e.currentTarget.blur(); }} />
                    GiB
                  </label>
                </div>
              </li>
            ))}
          </ul>
        )}
        {error && <p role="alert" className="mt-1 text-xs text-error" data-testid="volume-error">{error}</p>}
        <p className="mt-1 text-xs text-base-content/60">Each instance gets its own. Removing the module keeps the data; delete it in the Fabric Console.</p>
      </div>
    </Section>
  );
}

function ModuleBody({ id, tab }: { id: string; tab: "general" | "config" | "ports" | "issues" }) {
  const api = useStudioApi();
  const assembly = useStudio((s) => s.assembly);
  const diagnostics = useMemo(() => diagnosticsFor(assembly), [assembly]);
  const app = assembly.spec.modules.find((a) => a.id === id);
  const layout = assembly.editor.nodes.find((n) => n.id === id);
  if (!app || !layout) return null;
  const cls = lookupClass(app.classRef.name, app.classRef.version);
  const size = effectiveSize(layout, app.ports, assembly.editor.density);
  const mine = diagnostics.filter((d) => d.moduleId === app.id);

  if (tab === "config") return cls?.properties?.length ? <ConfigForm app={app} /> : <p className="text-xs text-base-content/60" data-testid="no-config">This module has no configuration fields.</p>;
  if (tab === "ports") return <div className="flex flex-col gap-4" data-testid="ports-tab"><PortSection appId={app.id} ports={app.ports} direction="input" /><PortSection appId={app.id} ports={app.ports} direction="output" /></div>;
  if (tab === "issues") {
    return mine.length === 0 ? <p className="text-xs text-base-content/60" data-testid="no-issues">No issues for this module.</p> : (
      <ul className="flex flex-col gap-2" data-testid="inspector-diagnostics">{mine.map((d, i) => <li key={i} className="text-xs"><span className={`badge badge-xs mr-1 ${d.severity === "error" ? "badge-error" : "badge-warning"}`}>{d.severity}</span>{d.message}{d.hint && <span className="block pl-1 text-base-content/60">{d.hint}</span>}</li>)}</ul>
    );
  }
  return (
    <div className="flex flex-col gap-4" data-testid="inspector-module">
      <label className="fieldset">
        <span className="fieldset-legend text-[11px]">Instance name</span>
        <input key={app.name} defaultValue={app.name} data-testid="instance-name" className="input input-sm w-full" onBlur={(e) => api.getState().rename(app.id, e.currentTarget.value)} onKeyDown={(e) => { if (e.key === "Enter") e.currentTarget.blur(); }} />
      </label>
      <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5 text-xs">
        <dt className="text-base-content/60">Class</dt><dd className="truncate">{cls?.displayName ?? app.classRef.name} <span className="text-base-content/70">v{app.classRef.version}</span></dd>
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
      {cls?.runtime && (
        <Section title="Availability">
          {availabilityOf(cls) === "stateless" ? (
            <div data-testid="availability">
              <div className="flex gap-2">
                <NumberField label="Replicas" value={replicasOf(cls, app)} min={1} max={MAX_REPLICAS} step={1} testId="replicas" onCommit={(n) => api.getState().setReplicaCount(app.id, n)} />
                <div className="flex-1" />
              </div>
              <p className="mt-1 text-xs text-base-content/60">Stateless: replicas run on different cluster members, and a lost one is recreated automatically.</p>
            </div>
          ) : (
            <p className="text-xs text-base-content/60" data-testid="availability">
              {availabilityOf(cls) === "stateful" ? "Stateful" : "Singleton"}: runs as one instance. A lost instance is reported, not recreated automatically.
            </p>
          )}
          {cls.runtime.kind === "incus-vm" && (
            <div className="mt-2" data-testid="live-migration">
              <label className="flex items-center gap-2 text-xs">
                <input type="checkbox" className="checkbox checkbox-xs" checked={!!app.liveMigration} disabled={volumesOf(cls, app).length > 0} data-testid="live-migration-toggle" onChange={(e) => api.getState().setModuleLiveMigration(app.id, e.currentTarget.checked)} />
                Keep running when moved (live migration)
              </label>
              <p className="mt-1 text-xs text-base-content/60">
                {volumesOf(cls, app).length > 0
                  ? "Not with volumes on its server's own pool: they cannot follow a running VM."
                  : "Moves to another server with a pause of under a second, also during server maintenance."}
              </p>
            </div>
          )}
        </Section>
      )}
      {cls?.runtime && <VolumesSection appId={app.id} cls={cls} volumes={volumesOf(cls, app)} />}
      {cls?.runtime && (
        <Section title="Internet access">
          <label className="fieldset">
            <span className="sr-only">Internet access</span>
            <select className="select select-sm w-full" value={app.internet ?? "none"} data-testid="internet-access" onChange={(e) => api.getState().setInternetAccess(app.id, e.target.value as InternetAccess)}>
              <option value="none">None</option>
              <option value="web">Web and DNS (ports 80, 443, 53)</option>
              <option value="any">Any port</option>
            </select>
          </label>
          <p className="mt-1 text-xs text-base-content/60">Outside the application only; inside it, only cables allow traffic.</p>
        </Section>
      )}
      {cls?.composition && (
        <Section title={`Contents (${cls.composition.modules.length})`} right={<button className="btn btn-ghost btn-xs" data-testid="ungroup" onClick={() => api.getState().ungroupSelection(app.id)}>Ungroup</button>}>
          <div data-testid="component-contents">
            <ul className="flex flex-col gap-0.5 text-xs">{cls.composition.modules.map((x) => <li key={x.id} className="truncate">{x.name} <span className="text-base-content/70">· {lookupClass(x.classRef.name, x.classRef.version)?.displayName ?? x.classRef.name}</span></li>)}</ul>
            <p className="mt-1 text-[10px] text-base-content/70">Its <span className="font-mono">net</span> plugs every inner module into the network.</p>
          </div>
        </Section>
      )}
      {cls && <MetricsSection cls={cls} app={app} />}
      <Section title="Health policy"><p className="text-xs text-base-content/70">Placeholder: health checks are defined in a later milestone.</p></Section>
      <div className="flex gap-2">
        <button className="btn btn-sm btn-outline flex-1" data-testid="save-as-class" title="Turn this module, with its ports, into a reusable catalog module" disabled={!!cls?.composition} onClick={() => useCustomCatalog.getState().openDesigner(draftFromPorts(app.name, app.ports, app.resources, cls?.iconAssetPath ?? "generic-box", cls?.properties, app.config))}><IconBox size={14} />Save as custom module…</button>
        <button className="btn btn-sm btn-outline" data-testid="delete-module" onClick={() => { api.getState().deleteElements([app.id], []); }}><IconTrash size={14} />Delete</button>
      </div>
    </div>
  );
}

function CableBody({ id, api, assembly }: { id: string; api: Api; assembly: ApplicationAssembly }) {
  const conn = assembly.spec.connections.find((c) => c.id === id);
  if (!conn) return null;
  const sa = assembly.spec.modules.find((a) => a.id === conn.source.moduleId);
  const ta = assembly.spec.modules.find((a) => a.id === conn.target.moduleId);
  const sp = sa?.ports.find((p) => p.id === conn.source.portId);
  const tp = ta?.ports.find((p) => p.id === conn.target.portId);
  const custom = cableColorOf(assembly, conn.id);
  return (
    <div className="flex flex-col gap-3 text-xs" data-testid="inspector-connection">
      <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5">
        <dt className="text-base-content/60">From</dt><dd>{sa?.name} · {sp?.name} <span className="text-base-content/70">({sp?.type})</span></dd>
        <dt className="text-base-content/60">To</dt><dd>{ta?.name} · {tp?.name} <span className="text-base-content/70">({tp?.type})</span></dd>
        <dt className="text-base-content/60">Protocol</dt><dd>{sp?.type}</dd>
        <dt className="text-base-content/60">Id</dt><dd className="font-mono">{conn.id}</dd>
      </dl>
      <div className="flex flex-col gap-1.5">
        <span className="text-base-content/60">Colour</span>
        <Swatches value={custom ?? ""} label="Cable colour" testId="cable-colour" onPick={(c) => api.getState().setCableColorAction(conn.id, c)} />
        <button type="button" className="btn btn-xs btn-ghost self-start" data-testid="cable-colour-reset" disabled={!custom} onClick={() => api.getState().setCableColorAction(conn.id, null)}>
          {custom ? "Use protocol colour" : "Using protocol colour"}
        </button>
      </div>
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
        {members.map((m) => { const a = assembly.spec.modules.find((x) => x.id === m.endpoint.moduleId); const p = portOf(assembly, m.endpoint); return (
          <li key={m.id}><button type="button" className="btn btn-ghost btn-xs w-full justify-start font-normal" onClick={() => api.getState().selectNode(m.endpoint.moduleId)}><span className="badge badge-xs">{p?.direction === "output" ? "out" : "in"}</span>{a?.name}.{p?.name} <span className="text-base-content/70">({p?.type})</span></button></li>
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
  const api = useStudioApi();
  const d = useMemo(() => diagnosticsFor(assembly), [assembly]);
  const errors = d.filter((x) => x.severity === "error").length;
  const totals = assemblyTotals(assembly);
  const platform = platformOf(assembly);
  const info = PLATFORM_INFO[platform];
  return (
    <div className="flex flex-col gap-4">
      <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5 text-xs" data-testid="inspector-assembly">
        <dt className="text-base-content/60">Assembly</dt><dd className="truncate">{assembly.metadata.name}</dd>
        <dt className="text-base-content/60">Modules</dt><dd>{totals.modules}</dd>
        <dt className="text-base-content/60">Connections</dt><dd>{assembly.spec.connections.length}</dd>
        <dt className="text-base-content/60">Link tags</dt><dd>{(assembly.spec.links ?? []).length}</dd>
        <dt className="text-base-content/60">Zones and shapes</dt><dd>{(assembly.editor.shapes ?? []).length}</dd>
        <dt className="text-base-content/60">Requested</dt><dd data-testid="assembly-totals">{totals.cpu} vCPU · {formatMemory(totals.memoryMiB)}</dd>
        <dt className="text-base-content/60">Issues</dt><dd>{errors} errors, {d.length - errors} warnings</dd>
        <dt className="text-base-content/60">Format</dt><dd className="truncate font-mono">{assembly.apiVersion}</dd>
      </dl>
      <section aria-label="Target platform" className="rounded-box border border-base-300 p-3" data-testid="platform-section">
        <label className="fieldset"><span className="fieldset-legend text-[11px]">Target platform</span>
          <select className="select select-sm w-full" data-testid="platform-select" value={platform} onChange={(e) => api.getState().setTarget(e.target.value as PlatformId)}>
            {PLATFORMS.map((p) => <option key={p} value={p}>{PLATFORM_INFO[p].name}{PLATFORM_INFO[p].status === "planned" ? " (planned)" : ""}</option>)}
          </select>
        </label>
        <p className="mt-2 text-xs text-base-content/70">{info.summary}</p>
        <dl className="mt-2 grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-[11px]" data-testid="platform-facts">
          <dt className="text-base-content/60">Guest agent</dt><dd>{info.guestAgent}</dd>
          <dt className="text-base-content/60">Disk formats</dt><dd>{info.diskFormats.join(", ")}</dd>
          <dt className="text-base-content/60">NIC models</dt><dd>{info.nicModels.join(", ")}</dd>
          <dt className="text-base-content/60">Packaging</dt><dd>{info.packaging}</dd>
        </dl>
        <p className="mt-2 text-[11px] text-base-content/70">The modules, ports and cables in your diagram stay the same whichever platform you pick; only these provider details change.</p>
      </section>
    </div>
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
      (win.kind === "module" && !assembly.spec.modules.some((a) => a.id === win.id)) ||
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

  if (win.kind === "module") {
    const app = assembly.spec.modules.find((a) => a.id === win.id);
    const cls = app ? lookupClass(app.classRef.name, app.classRef.version) : undefined;
    const tab = win.tab ?? "general";
    const nIssues = diagnosticsFor(assembly).filter((d) => d.moduleId === win.id).length;
    title = app?.name ?? win.id;
    subtitle = `${cls?.displayName ?? app?.classRef.name ?? ""} · ${win.id}`;
    accent = cls ? categoryColor(cls.category) : undefined;
    tabs = (
      <div role="tablist" className="tabs tabs-border px-3" data-testid="props-tabs">
        {TABS.map((t) => (
          <button key={t.id} role="tab" type="button" aria-selected={tab === t.id} data-testid={`props-tab-${t.id}`} className={`tab h-9 text-xs ${tab === t.id ? "tab-active" : "text-base-content/70"}`} onClick={() => api.getState().setPropsTab(t.id)}>
            {t.label}{t.id === "config" && cls?.properties?.length ? ` (${cls.properties.length})` : ""}{t.id === "ports" && app ? ` (${app.ports.length})` : ""}{t.id === "issues" && nIssues > 0 ? <span className="badge badge-error badge-xs ml-1">{nIssues}</span> : null}
          </button>
        ))}
      </div>
    );
    body = <ModuleBody id={win.id} tab={tab} />;
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
          {subtitle && <p className="truncate font-mono text-[10px] text-base-content/70">{subtitle}</p>}
        </div>
        <button type="button" className="btn btn-ghost btn-xs btn-square" aria-label="Close properties" data-testid="props-close" onClick={() => api.getState().closeProps()}><IconX size={14} /></button>
      </div>
      {tabs}
      <div className="min-h-0 flex-1 overflow-y-auto p-4" data-testid="props-body">{body}</div>
    </div>
  );
}

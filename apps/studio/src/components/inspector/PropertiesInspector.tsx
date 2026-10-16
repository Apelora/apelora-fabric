import { PORT_TYPES, type Port, type PortDirection } from "@apelora/assembly-model";
import { lookupClass } from "@apelora/appliance-catalog";
import { useStudio, useStudioApi } from "../../features/editor/context";
import { diagnosticsFor } from "../../features/editor/store";
import { IconDown, IconTrash, IconUp, IconPlus, IconBox } from "../icons";
import { draftFromPorts, useCustomCatalog } from "../../features/catalog/customCatalog";
import { useMemo } from "react";
import { effectiveSize } from "@apelora/assembly-model";

function NumberField({ label, value, min, step, onCommit, testId }: { label: string; value: number; min: number; step: number; onCommit: (n: number) => void; testId: string }) {
  return (
    <label className="fieldset min-w-0 flex-1">
      <span className="fieldset-legend text-[11px]">{label}</span>
      <input
        key={value}
        type="number" min={min} step={step} defaultValue={value} data-testid={testId}
        className="input input-sm w-full"
        onBlur={(e) => { const n = Number(e.currentTarget.value); if (Number.isFinite(n) && n >= min && n !== value) onCommit(n); else e.currentTarget.value = String(value); }}
        onKeyDown={(e) => { if (e.key === "Enter") e.currentTarget.blur(); }}
      />
    </label>
  );
}

function PortRow({ appId, port, first, last }: { appId: string; port: Port; first: boolean; last: boolean }) {
  const api = useStudioApi();
  const s = () => api.getState();
  return (
    <li className="rounded-box border border-base-300 p-1.5" data-testid="port-row" data-port-id={port.id}>
      <div className="flex items-center gap-1">
        <input key={port.name} defaultValue={port.name} aria-label={`Name of ${port.id}`} className="input input-xs min-w-0 flex-1"
          onBlur={(e) => { if (e.currentTarget.value.trim() && e.currentTarget.value !== port.name) s().editPort(appId, port.id, { name: e.currentTarget.value }); else e.currentTarget.value = port.name; }}
          onKeyDown={(e) => { if (e.key === "Enter") e.currentTarget.blur(); }} />
        <button className="btn btn-ghost btn-xs btn-square" aria-label={`Move ${port.name} up`} disabled={first} onClick={() => s().shiftPort(appId, port.id, -1)}><IconUp size={12} /></button>
        <button className="btn btn-ghost btn-xs btn-square" aria-label={`Move ${port.name} down`} disabled={last} onClick={() => s().shiftPort(appId, port.id, 1)}><IconDown size={12} /></button>
        <button className="btn btn-ghost btn-xs btn-square" aria-label={`Delete port ${port.name}`} data-testid="delete-port" onClick={() => s().deletePort(appId, port.id)}><IconTrash size={12} /></button>
      </div>
      <div className="mt-1 flex items-center gap-2">
        <select className="select select-xs min-w-0 flex-1" aria-label={`Type of ${port.name}`} value={port.type} onChange={(e) => s().editPort(appId, port.id, { type: e.target.value as Port["type"] })}>
          {PORT_TYPES.map((t) => <option key={t} value={t}>{t}</option>)}
        </select>
        <label className="flex items-center gap-1 text-[11px]" title="Allow several cables to plug into this port"><input type="checkbox" className="checkbox checkbox-xs" data-testid="port-multi" checked={port.multiplicity === "many"} onChange={(e) => s().editPort(appId, port.id, { multiplicity: e.target.checked ? "many" : "one" })} />multi</label>
        <label className="flex items-center gap-1 text-[11px]"><input type="checkbox" className="checkbox checkbox-xs" checked={!port.optional} onChange={(e) => s().editPort(appId, port.id, { optional: !e.target.checked })} />required</label>
      </div>
    </li>
  );
}

function PortSection({ appId, ports, direction }: { appId: string; ports: Port[]; direction: PortDirection }) {
  const api = useStudioApi();
  const side = ports.filter((p) => p.direction === direction);
  return (
    <section aria-label={direction === "input" ? "Input ports" : "Output ports"}>
      <div className="mb-1 flex items-center justify-between">
        <h4 className="text-[11px] font-semibold uppercase tracking-wide text-base-content/60">{direction === "input" ? "Inputs (left)" : "Outputs (right)"}</h4>
        <button className="btn btn-ghost btn-xs" data-testid={`add-${direction}`} onClick={() => api.getState().addPortTo(appId, direction)}><IconPlus size={12} />Add</button>
      </div>
      <ul className="flex flex-col gap-1.5">
        {side.map((p, i) => <PortRow key={p.id} appId={appId} port={p} first={i === 0} last={i === side.length - 1} />)}
        {side.length === 0 && <li className="text-xs text-base-content/50">No {direction} ports.</li>}
      </ul>
    </section>
  );
}

export function PropertiesInspector() {
  const api = useStudioApi();
  const assembly = useStudio((s) => s.assembly);
  const selection = useStudio((s) => s.selection);
  const diagnostics = useMemo(() => diagnosticsFor(assembly), [assembly]);

  const nodeId = selection.nodes.length === 1 ? selection.nodes[0] : undefined;
  const app = assembly.spec.appliances.find((a) => a.id === nodeId);
  const layout = assembly.editor.nodes.find((n) => n.id === nodeId);
  const conn = selection.nodes.length === 0 && selection.edges.length === 1 ? assembly.spec.connections.find((c) => c.id === selection.edges[0]) : undefined;

  let body;
  if (app && layout) {
    const cls = lookupClass(app.classRef.name, app.classRef.version);
    const size = effectiveSize(layout, app.ports, assembly.editor.density);
    const mine = diagnostics.filter((d) => d.applianceId === app.id);
    body = (
      <div className="flex flex-col gap-4" data-testid="inspector-appliance">
        <label className="fieldset">
          <span className="fieldset-legend text-[11px]">Instance name</span>
          <input key={app.name} defaultValue={app.name} data-testid="instance-name" className="input input-sm w-full"
            onBlur={(e) => api.getState().rename(app.id, e.currentTarget.value)} onKeyDown={(e) => { if (e.key === "Enter") e.currentTarget.blur(); }} />
        </label>
        <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-xs">
          <dt className="text-base-content/60">Class</dt><dd className="truncate">{cls?.displayName ?? app.classRef.name} <span className="text-base-content/50">v{app.classRef.version}</span></dd>
          <dt className="text-base-content/60">Id</dt><dd className="truncate font-mono">{app.id}</dd>
          <dt className="text-base-content/60">Size</dt><dd className="tabular-nums" data-testid="node-size">{size.width} × {size.height}{layout.custom && <button className="link ml-2" onClick={() => api.getState().resetSize(app.id)}>reset</button>}</dd>
        </dl>
        <section>
          <h4 className="mb-1 text-[11px] font-semibold uppercase tracking-wide text-base-content/60">Resources (request)</h4>
          <div className="flex gap-2">
            <NumberField label="vCPU" value={app.resources.cpu} min={1} step={1} testId="res-cpu" onCommit={(n) => api.getState().setRes(app.id, { cpu: n })} />
            <NumberField label="Memory MiB" value={app.resources.memoryMiB} min={64} step={64} testId="res-mem" onCommit={(n) => api.getState().setRes(app.id, { memoryMiB: n })} />
          </div>
        </section>
        <PortSection appId={app.id} ports={app.ports} direction="input" />
        <PortSection appId={app.id} ports={app.ports} direction="output" />
        <section>
          <h4 className="mb-1 text-[11px] font-semibold uppercase tracking-wide text-base-content/60">Health policy</h4>
          <p className="text-xs text-base-content/50">Placeholder: health checks are defined in a later milestone.</p>
        </section>
        {mine.length > 0 && (
          <section data-testid="inspector-diagnostics">
            <h4 className="mb-1 text-[11px] font-semibold uppercase tracking-wide text-base-content/60">Issues</h4>
            <ul className="flex flex-col gap-1">{mine.map((d, i) => <li key={i} className="text-xs"><span className={`badge badge-xs mr-1 ${d.severity === "error" ? "badge-error" : "badge-warning"}`}>{d.severity}</span>{d.message}{d.hint && <span className="block pl-1 text-base-content/60">{d.hint}</span>}</li>)}</ul>
          </section>
        )}
        <button className="btn btn-sm btn-outline" data-testid="save-as-class" title="Turn this appliance, with its ports, into a reusable catalog appliance" onClick={() => useCustomCatalog.getState().openDesigner(draftFromPorts(app.name, app.ports, app.resources, cls?.iconAssetPath ?? "generic-box"))}><IconBox size={14} />Save as custom appliance…</button>
        <button className="btn btn-sm btn-outline" data-testid="delete-appliance" onClick={() => api.getState().deleteElements([app.id], [])}><IconTrash size={14} />Delete appliance</button>
      </div>
    );
  } else if (conn) {
    const sa = assembly.spec.appliances.find((a) => a.id === conn.source.applianceId);
    const ta = assembly.spec.appliances.find((a) => a.id === conn.target.applianceId);
    const sp = sa?.ports.find((p) => p.id === conn.source.portId);
    const tp = ta?.ports.find((p) => p.id === conn.target.portId);
    body = (
      <div className="flex flex-col gap-3 text-xs" data-testid="inspector-connection">
        <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1">
          <dt className="text-base-content/60">From</dt><dd>{sa?.name} · {sp?.name} <span className="text-base-content/50">({sp?.type})</span></dd>
          <dt className="text-base-content/60">To</dt><dd>{ta?.name} · {tp?.name} <span className="text-base-content/50">({tp?.type})</span></dd>
          <dt className="text-base-content/60">Id</dt><dd className="font-mono">{conn.id}</dd>
        </dl>
        <button className="btn btn-sm btn-outline" data-testid="delete-connection" onClick={() => api.getState().deleteElements([], [conn.id])}><IconTrash size={14} />Delete connection</button>
      </div>
    );
  } else {
    const errors = diagnostics.filter((d) => d.severity === "error").length;
    body = (
      <div className="flex flex-col gap-4 text-xs" data-testid="inspector-assembly">
        <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1">
          <dt className="text-base-content/60">Assembly</dt><dd className="truncate">{assembly.metadata.name}</dd>
          <dt className="text-base-content/60">Appliances</dt><dd>{assembly.spec.appliances.length}</dd>
          <dt className="text-base-content/60">Connections</dt><dd>{assembly.spec.connections.length}</dd>
          <dt className="text-base-content/60">Issues</dt><dd>{errors} errors, {diagnostics.length - errors} warnings</dd>
          <dt className="text-base-content/60">Format</dt><dd className="truncate font-mono">{assembly.apiVersion}</dd>
        </dl>
        <p className="text-base-content/60">Select an appliance or connection to edit its properties. {selection.nodes.length > 1 && `${selection.nodes.length} appliances selected.`}</p>
      </div>
    );
  }

  return (
    <aside className="w-56 shrink-0 overflow-y-auto border-l border-base-300 bg-base-100 p-3 lg:w-64" aria-label="Properties inspector" data-testid="inspector">
      <h2 className="mb-3 text-xs font-semibold uppercase tracking-wide text-base-content/70">Properties</h2>
      {body}
    </aside>
  );
}

import { PORT_TYPES, portOf, tagNames, tagNet, tagOf, type Port, type PortDirection } from "@apelora/assembly-model";
import { useStudio, useStudioApi } from "../../features/editor/context";
import { IconDown, IconTrash, IconUp, IconPlus } from "../icons";
import type { ApplicationAssembly } from "@apelora/assembly-model";

export function NumberField({ label, value, min, step, onCommit, testId }: { label: string; value: number; min: number; step: number; onCommit: (n: number) => void; testId: string }) {
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
  const assembly = useStudio((st) => st.assembly);
  const tag = tagOf(assembly, { applianceId: appId, portId: port.id })?.tag ?? "";
  const listId = `tags-${appId}-${port.id}`;
  return (
    <li className="rounded-box border border-base-300 p-1.5" data-testid="port-row" data-port-id={port.id}>
      <div className="flex items-center gap-1">
        <input key={port.name} defaultValue={port.name} aria-label={`Name of ${port.id}`} className="input input-xs min-w-0 flex-1"
          onBlur={(e) => { if (e.currentTarget.value.trim() && e.currentTarget.value !== port.name) s().editPort(appId, port.id, { name: e.currentTarget.value }); else e.currentTarget.value = port.name; }}
          onKeyDown={(e) => { if (e.key === "Enter") e.currentTarget.blur(); }} />
        <button className="btn btn-ghost btn-xs btn-square" aria-label={`Move ${port.name} up`} disabled={first} onClick={() => s().shiftPort(appId, port.id, -1)}><IconUp size={12} /></button>
        <button className="btn btn-ghost btn-xs btn-square" aria-label={`Move ${port.name} down`} disabled={last} onClick={() => s().shiftPort(appId, port.id, 1)}><IconDown size={12} /></button>
        <button className="btn btn-ghost btn-xs btn-square" aria-label={`Delete port ${port.name}`} data-testid="delete-port" disabled={port.id === "net"} title={port.id === "net" ? "Every appliance keeps its network interface" : undefined} onClick={() => s().deletePort(appId, port.id)}><IconTrash size={12} /></button>
      </div>
      <div className="mt-1 flex items-center gap-1">
        <input key={tag} defaultValue={tag} list={listId} aria-label={`Link tag of ${port.name}`} data-testid="port-tag" placeholder="link tag (name)" maxLength={32} className="input input-xs min-w-0 flex-1 font-mono"
          onBlur={(e) => { const v = e.currentTarget.value.trim().toLowerCase(); if (v === tag) return; if (v === "") s().untagPortAction({ applianceId: appId, portId: port.id }); else if (!s().tagPortTo({ applianceId: appId, portId: port.id }, v)) e.currentTarget.value = tag; }}
          onKeyDown={(e) => { if (e.key === "Enter") e.currentTarget.blur(); }} />
        <datalist id={listId}>{tagNames(assembly).map((n) => <option key={n} value={n} />)}</datalist>
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

export function PortSection({ appId, ports, direction }: { appId: string; ports: Port[]; direction: PortDirection }) {
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

/** Where an appliance's net interface is plugged: a network appliance (by cable or tag) or the default network. */
export function nicStatus(a: ReturnType<typeof useStudio<ApplicationAssembly>>, id: string): string {
  const e = { applianceId: id, portId: "net" };
  if (!portOf(a, e)) return "no interface";
  const cable = a.spec.connections.find((c) => c.target.applianceId === id && c.target.portId === "net");
  if (cable) return `${a.spec.appliances.find((x) => x.id === cable.source.applianceId)?.name ?? "?"} (cable)`;
  const t = tagOf(a, e);
  if (t) {
    const src = tagNet(a, t.tag).find((l) => portOf(a, l.endpoint)?.direction === "output");
    return src ? `${a.spec.appliances.find((x) => x.id === src.endpoint.applianceId)?.name ?? "?"} (tag ${t.tag})` : `tag ${t.tag} (no network yet)`;
  }
  return "default network";
}


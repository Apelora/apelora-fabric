import { useEffect, useMemo, useState } from "react";
import { groupCandidates, type GroupCandidate } from "@apelora/assembly-model";
import { useStudio, useStudioApi } from "../../features/editor/context";

interface Row { key: string; c: GroupCandidate; on: boolean; name: string }

/** Packages the selected modules into one reusable component; the user picks which inner terminals become its ports. */
export function ComponentDialog() {
  const api = useStudioApi();
  const ids = useStudio((s) => s.groupDialog);
  const assembly = useStudio((s) => s.assembly);
  const [name, setName] = useState("");
  const [rows, setRows] = useState<Row[]>([]);

  const cands = useMemo(() => (ids ? groupCandidates(assembly, ids) : []), [ids, assembly]);
  useEffect(() => {
    if (!ids) return;
    const taken = new Map<string, number>();
    for (const c of cands) taken.set(`${c.port.direction}:${c.port.name}`, (taken.get(`${c.port.direction}:${c.port.name}`) ?? 0) + 1);
    setName("");
    setRows(cands.map((c) => ({ key: `${c.module.id}:${c.port.id}`, c, on: c.external, name: (taken.get(`${c.port.direction}:${c.port.name}`) ?? 0) > 1 ? `${c.module.name}-${c.port.name}` : c.port.name })));
  }, [ids, cands]);

  if (!ids) return null;
  const patch = (key: string, p: Partial<Row>) => setRows((rs) => rs.map((r) => (r.key === key ? { ...r, ...p } : r)));
  const byApp = ids.map((id) => ({ app: assembly.spec.modules.find((a) => a.id === id)!, rows: rows.filter((r) => r.c.module.id === id) })).filter((g) => g.app);
  const create = () => api.getState().groupSelection(name, rows.filter((r) => r.on).map((r) => ({ inner: { moduleId: r.c.module.id, portId: r.c.port.id }, name: r.name })));

  return (
    <dialog className="modal modal-open" data-testid="component-dialog" aria-labelledby="comp-title">
      <div className="modal-box flex max-h-[90vh] w-full max-w-xl flex-col p-0">
        <div className="border-b border-base-300 px-5 py-3">
          <h3 id="comp-title" className="text-base font-semibold">Group into a component</h3>
          <p className="text-xs text-base-content/60">{ids.length} modules become one reusable module. Choose which inner terminals appear as its ports; terminals linked to something outside the selection are required. The component's <span className="font-mono">net</span> plugs every inner module into the network.</p>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto p-5">
          <label className="fieldset"><span className="fieldset-legend text-[11px]">Component name</span><input className="input input-sm w-full" data-testid="component-name" maxLength={48} value={name} placeholder="e.g. Web tier" onChange={(e) => setName(e.target.value)} /></label>
          <div className="mt-4 flex flex-col gap-3" data-testid="component-ports">
            {byApp.map(({ app, rows: rs }) => (
              <section key={app.id} aria-label={app.name}>
                <h4 className="mb-1 text-[11px] font-semibold uppercase tracking-wide text-base-content/60">{app.name}</h4>
                <ul className="flex flex-col gap-1">
                  {rs.map((r) => (
                    <li key={r.key} className="flex items-center gap-2 rounded-box border border-base-300 px-2 py-1" data-testid="component-port" data-external={r.c.external}>
                      <input type="checkbox" className="checkbox checkbox-xs" aria-label={`Expose ${app.name} ${r.c.port.name}`} checked={r.on} disabled={r.c.external} onChange={(e) => patch(r.key, { on: e.target.checked })} />
                      <span className={`badge badge-xs ${r.c.port.direction === "input" ? "" : "badge-neutral"}`}>{r.c.port.direction === "input" ? "in" : "out"}</span>
                      <span className="w-20 truncate text-xs">{r.c.port.name}</span>
                      <span className="text-[10px] text-base-content/50">{r.c.port.type}</span>
                      <input className="input input-xs ml-auto w-32" aria-label={`Port name for ${app.name} ${r.c.port.name}`} value={r.name} disabled={!r.on} maxLength={48} onChange={(e) => patch(r.key, { name: e.target.value })} />
                      {r.c.external && <span className="badge badge-xs badge-warning" title="Linked outside the selection, so it must be exposed">linked</span>}
                    </li>
                  ))}
                  {rs.length === 0 && <li className="text-xs text-base-content/50">No terminals.</li>}
                </ul>
              </section>
            ))}
          </div>
        </div>
        <div className="flex items-center justify-end gap-2 border-t border-base-300 px-5 py-3">
          <button type="button" className="btn btn-sm" onClick={() => api.getState().closeGroupDialog()}>Cancel</button>
          <button type="button" className="btn btn-sm btn-primary" data-testid="component-create" onClick={create}>Create component</button>
        </div>
      </div>
      <div className="modal-backdrop" onClick={() => api.getState().closeGroupDialog()} />
    </dialog>
  );
}

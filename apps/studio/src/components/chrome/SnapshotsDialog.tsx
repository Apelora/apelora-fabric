import { useMemo, useState } from "react";
import { diffAssemblies } from "@apelora/assembly-model";
import { useStudio, useStudioApi } from "../../features/editor/context";
import { addSnapshot, listSnapshots, readSnapshot, removeSnapshot, type Snapshot } from "../../features/snapshots/snapshots";
import { Modal } from "./Modal";

/** Named restore points kept in this browser, with a plain-language comparison against the current diagram. */
export function SnapshotsDialog() {
  const api = useStudioApi();
  const open = useStudio((s) => s.dialog === "snapshots");
  const assembly = useStudio((s) => s.assembly);
  const [list, setList] = useState<Snapshot[]>(() => listSnapshots());
  const [name, setName] = useState("");
  const [compare, setCompare] = useState<string | null>(null);
  const diff = useMemo(() => {
    const s = list.find((x) => x.id === compare);
    const old = s && readSnapshot(s);
    return old ? diffAssemblies(old, assembly) : null;
  }, [compare, list, assembly]);
  if (!open) return null;
  const close = () => api.getState().setDialog(null);
  const save = () => {
    const r = addSnapshot(name || `${assembly.metadata.name} ${new Date().toLocaleString()}`, assembly);
    if (!r.ok) { api.getState().toast("warning", r.error); return; }
    setList(r.list); setName("");
    api.getState().toast("success", "Snapshot saved in this browser.");
  };
  const restore = (s: Snapshot) => {
    const a = readSnapshot(s);
    if (!a) { api.getState().toast("error", "This snapshot could not be read."); return; }
    api.getState().replace(a);
    api.getState().setDialog(null);
    api.getState().toast("success", `Restored “${s.name}”. Undo returns to what you had.`);
  };
  return (
    <Modal testId="snapshots-dialog" title="Snapshots" hint="Save the diagram before a risky change, compare later, and restore with one click. Snapshots live in this browser only." onClose={close}>
      <div className="mb-4 flex gap-2">
        <input className="input input-sm min-w-0 flex-1" data-testid="snapshot-name" value={name} maxLength={60} placeholder="Name, e.g. Before adding Kafka" onChange={(e) => setName(e.target.value)} onKeyDown={(e) => { if (e.key === "Enter") save(); }} aria-label="Snapshot name" />
        <button type="button" className="btn btn-sm btn-primary" data-testid="snapshot-save" onClick={save}>Save snapshot</button>
      </div>
      {list.length === 0 && <p className="text-sm text-base-content/60" data-testid="snapshots-empty">No snapshots yet.</p>}
      <ul className="flex flex-col gap-2">
        {[...list].reverse().map((s) => (
          <li key={s.id} className="rounded-box border border-base-300 p-2" data-testid="snapshot-row">
            <div className="flex items-center gap-2">
              <div className="min-w-0 flex-1"><div className="truncate text-sm font-medium">{s.name}</div><div className="text-[11px] text-base-content/60">{new Date(s.at).toLocaleString()}</div></div>
              <button type="button" className="btn btn-xs" data-testid="snapshot-compare" aria-pressed={compare === s.id} onClick={() => setCompare(compare === s.id ? null : s.id)}>Compare</button>
              <button type="button" className="btn btn-xs" data-testid="snapshot-restore" onClick={() => restore(s)}>Restore</button>
              <button type="button" className="btn btn-xs btn-ghost" aria-label={`Delete snapshot ${s.name}`} data-testid="snapshot-delete" onClick={() => { setList(removeSnapshot(s.id)); if (compare === s.id) setCompare(null); }}>Delete</button>
            </div>
            {compare === s.id && diff && (
              <div className="mt-2 rounded-box bg-base-200 p-2 text-xs" data-testid="snapshot-diff">
                {diff.identical ? <p>Identical to the current diagram.</p> : (
                  <ul className="list-disc space-y-0.5 pl-4">
                    {diff.addedModules.length > 0 && <li>Added since: {diff.addedModules.join(", ")}</li>}
                    {diff.removedModules.length > 0 && <li>Removed since: {diff.removedModules.join(", ")}</li>}
                    {diff.changedModules.map((c) => <li key={c.name}>{c.name}: {c.changes.join("; ")}</li>)}
                    {diff.addedCables.length > 0 && <li>New cables: {diff.addedCables.join("; ")}</li>}
                    {diff.removedCables.length > 0 && <li>Removed cables: {diff.removedCables.join("; ")}</li>}
                    {diff.movedModules > 0 && <li>{diff.movedModules} module{diff.movedModules > 1 ? "s" : ""} moved</li>}
                    {diff.zonesBefore !== diff.zonesAfter && <li>Zones and notes: {diff.zonesBefore} → {diff.zonesAfter}</li>}
                  </ul>
                )}
                <p className="mt-1 text-base-content/60">Compared with the snapshot, so “added” means added after it was saved.</p>
              </div>
            )}
          </li>
        ))}
      </ul>
    </Modal>
  );
}

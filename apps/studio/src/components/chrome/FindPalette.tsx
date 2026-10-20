import { useEffect, useMemo, useRef, useState } from "react";
import { useReactFlow } from "@xyflow/react";
import { lookupClass } from "@apelora/module-catalog";
import { useStudio, useStudioApi } from "../../features/editor/context";
import { ms } from "../../features/editor/motion";
import { Modal } from "./Modal";

/** Ctrl+K: jump to a module, zone or note by name, class or id. */
export function FindPalette() {
  const api = useStudioApi();
  const open = useStudio((s) => s.dialog === "find");
  const assembly = useStudio((s) => s.assembly);
  const { fitView } = useReactFlow();
  const [q, setQ] = useState("");
  const [idx, setIdx] = useState(0);
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => { if (open) { setQ(""); setIdx(0); } }, [open]);
  const results = useMemo(() => {
    const needle = q.trim().toLowerCase();
    const apps = assembly.spec.modules.map((a) => ({ kind: "module" as const, id: a.id, title: a.name, sub: lookupClass(a.classRef.name, a.classRef.version)?.displayName ?? a.classRef.name, hay: `${a.name} ${a.id} ${a.classRef.name} ${lookupClass(a.classRef.name, a.classRef.version)?.displayName ?? ""}`.toLowerCase() }));
    const shapes = (assembly.editor.shapes ?? []).map((s) => ({ kind: "shape" as const, id: s.id, title: s.kind === "note" ? (s.text ?? "").split("\n")[0]!.slice(0, 40) || "Note" : s.label || "Zone", sub: s.kind === "note" ? "Note" : "Zone", hay: `${s.label} ${s.text ?? ""} ${s.kind}`.toLowerCase() }));
    return [...apps, ...shapes].filter((r) => !needle || r.hay.includes(needle)).slice(0, 30);
  }, [q, assembly]);
  if (!open) return null;
  const close = () => api.getState().setDialog(null);
  const go = (r: (typeof results)[number] | undefined) => {
    if (!r) return;
    if (r.kind === "module") api.getState().selectNode(r.id); else api.getState().selectShape(r.id);
    api.getState().setDialog(null);
    requestAnimationFrame(() => void fitView({ nodes: [{ id: r.id }], padding: 0.8, maxZoom: 1.3, duration: ms(250) }));
  };
  return (
    <Modal testId="find-dialog" title="Find" hint="Type part of a name, class or id. Arrow keys choose, Enter jumps." onClose={close} size="max-w-lg">
      <input ref={input} autoFocus className="input input-sm mb-3 w-full" data-testid="find-input" aria-label="Find module" placeholder="e.g. postgres" value={q}
        onChange={(e) => { setQ(e.target.value); setIdx(0); }}
        onKeyDown={(e) => { if (e.key === "ArrowDown") { e.preventDefault(); setIdx(Math.min(idx + 1, results.length - 1)); } else if (e.key === "ArrowUp") { e.preventDefault(); setIdx(Math.max(idx - 1, 0)); } else if (e.key === "Enter") go(results[idx]); }} />
      <ul className="flex max-h-72 flex-col overflow-y-auto" role="listbox" aria-label="Results">
        {results.map((r, i) => (
          <li key={r.id} role="option" aria-selected={i === idx}>
            <button type="button" tabIndex={-1} data-testid="find-result" data-id={r.id} className={`flex w-full items-center justify-between rounded px-2 py-1.5 text-left text-sm ${i === idx ? "bg-base-200" : ""}`} onMouseEnter={() => setIdx(i)} onClick={() => go(r)}>
              <span className="truncate">{r.title}</span><span className="ml-2 shrink-0 text-xs text-base-content/50">{r.sub}</span>
            </button>
          </li>
        ))}
        {results.length === 0 && <li className="px-2 py-3 text-sm text-base-content/60" data-testid="find-empty">Nothing matches.</li>}
      </ul>
    </Modal>
  );
}

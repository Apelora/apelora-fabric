import { useRef, useState } from "react";
import { importCompose, type ComposeResult } from "@apelora/module-catalog";
import { useStudio, useStudioApi } from "../../features/editor/context";
import { Modal } from "./Modal";

/** Paste or open a Docker Compose file; each service becomes a module. Nothing is run or fetched. */
export function ComposeDialog() {
  const api = useStudioApi();
  const open = useStudio((s) => s.dialog === "compose");
  const [text, setText] = useState("");
  const [result, setResult] = useState<ComposeResult | null>(null);
  const file = useRef<HTMLInputElement>(null);
  if (!open) return null;
  const close = () => { api.getState().setDialog(null); setResult(null); setText(""); };
  const analyse = (t: string) => { setText(t); setResult(t.trim() ? importCompose(t) : null); };
  return (
    <Modal testId="compose-dialog" title="Import from Docker Compose" hint="Services become modules and depends_on becomes cables where the protocols fit. Unknown images become Linux base modules. The file is read as text only." onClose={close}
      footer={<button type="button" className="btn btn-sm btn-primary" data-testid="compose-apply" disabled={!result?.ok} onClick={() => { if (result?.ok) { api.getState().replace(result.assembly); api.getState().toast("success", `Imported ${result.assembly.spec.modules.length} services. Undo restores the previous diagram.`); close(); } }}>Open as diagram</button>}>
      <div className="mb-2 flex items-center gap-2">
        <button type="button" className="btn btn-sm" onClick={() => file.current?.click()}>Choose file…</button>
        <input ref={file} type="file" accept=".yml,.yaml,text/yaml,text/plain" hidden data-testid="compose-file" onChange={async (e) => { const f = e.target.files?.[0]; if (f) analyse(await f.text()); e.target.value = ""; }} />
        <span className="text-xs text-base-content/60">or paste the YAML below</span>
      </div>
      <textarea className="textarea textarea-sm h-44 w-full font-mono text-xs" data-testid="compose-text" aria-label="Compose YAML" value={text} placeholder={"services:\n  web:\n    image: nginx\n    depends_on: [db]\n  db:\n    image: postgres:17"} onChange={(e) => analyse(e.target.value)} />
      {result && !result.ok && <ul className="mt-2 list-disc pl-5 text-xs text-error" data-testid="compose-errors">{result.errors.map((e, i) => <li key={i}>{e}</li>)}</ul>}
      {result?.ok && (
        <div className="mt-2 text-xs" data-testid="compose-summary">
          <p><b>{result.assembly.spec.modules.length}</b> modules, <b>{result.assembly.spec.connections.length}</b> cables.</p>
          {result.notes.length > 0 && <ul className="mt-1 list-disc space-y-0.5 pl-5 text-base-content/70" data-testid="compose-notes">{result.notes.map((n, i) => <li key={i}>{n}</li>)}</ul>}
        </div>
      )}
    </Modal>
  );
}

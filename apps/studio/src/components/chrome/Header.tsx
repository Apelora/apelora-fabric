import { useRef } from "react";
import { createEmptyAssembly, ensureNics, parseAssembly, serializeAssembly, type Density } from "@apelora/assembly-model";
import { lookupClass, sampleCommerceStack, sampleNetworkedStack } from "@apelora/appliance-catalog";
import { useCustomCatalog } from "../../features/catalog/customCatalog";
import { useStudio, useStudioApi } from "../../features/editor/context";
import { IconDownload, IconFile, IconRedo, IconUndo, IconUpload } from "../icons";

const DENSITIES: { id: Density; label: string; hint: string }[] = [
  { id: "compact", label: "Compact", hint: "128 × 96" },
  { id: "comfortable", label: "Comfortable", hint: "152 × 112" },
  { id: "expanded", label: "Expanded", hint: "178 × 150" },
];
const THEMES = ["light", "winter", "night"];

export function Header({ theme, onTheme, saved }: { theme: string; onTheme: (t: string) => void; saved: boolean }) {
  const api = useStudioApi();
  const name = useStudio((s) => s.assembly.metadata.name);
  const density = useStudio((s) => s.assembly.editor.density);
  const canUndo = useStudio((s) => s.past.length > 0);
  const canRedo = useStudio((s) => s.future.length > 0);
  const file = useRef<HTMLInputElement>(null);

  const doExport = () => {
    const base = api.getState().assembly;
    // Embed the custom appliance classes this assembly uses so the file is self-contained.
    const used = [...new Map(base.spec.appliances.flatMap((x) => { const c = lookupClass(x.classRef.name, x.classRef.version); return c?.custom ? [[`${c.id}@${c.version}`, c] as const] : []; })).values()];
    const a = { ...base, ...(used.length ? { catalog: { classes: used } } : { catalog: undefined }) };
    const url = URL.createObjectURL(new Blob([serializeAssembly(a)], { type: "application/json" }));
    const link = document.createElement("a");
    link.href = url;
    link.download = `${a.metadata.name}.assembly.json`;
    link.click();
    URL.revokeObjectURL(url);
    api.getState().toast("success", `Exported ${link.download}`);
  };

  const doImport = async (f: File) => {
    const r = parseAssembly(await f.text());
    if (!r.ok) { api.getState().setImportErrors(r.errors); return; }
    const added = useCustomCatalog.getState().addMissing(r.assembly.catalog?.classes ?? []);
    const assembly = ensureNics(r.assembly, lookupClass);
    api.getState().replace(assembly);
    api.getState().toast("success", `Imported “${r.assembly.metadata.name}”${added ? ` and ${added} custom appliance${added > 1 ? "s" : ""}` : ""}. Undo restores the previous assembly.`);
  };

  return (
    <header className="flex h-12 shrink-0 items-center gap-3 border-b border-base-300 bg-base-100 px-3" data-testid="header">
      <div className="flex shrink-0 items-center gap-2 whitespace-nowrap">
        <img src="/logo.svg" alt="" className="size-6 rounded" />
        <span className="text-sm font-semibold">Apelora Studio</span>
        <span className="badge badge-ghost badge-sm" title="Nothing here is deployed to a real host." data-testid="mock-badge">Mock · Design only</span>
      </div>
      <nav className="hidden min-w-0 items-center gap-1 whitespace-nowrap text-xs text-base-content/60 min-[1400px]:flex" aria-label="Breadcrumb">
        <span>Assemblies</span><span>/</span><span className="max-w-40 truncate text-base-content" data-testid="breadcrumb-name">{name}</span>
        <span className="ml-2" data-testid="save-state">{saved ? "Saved locally" : "Saving…"}</span>
      </nav>
      <div className="ml-auto flex items-center gap-2">
        <div className="join" role="group" aria-label="History">
          <button className="btn btn-sm join-item" title="Undo (Ctrl+Z)" aria-label="Undo" data-testid="undo" disabled={!canUndo} onClick={() => api.getState().undo()}><IconUndo /></button>
          <button className="btn btn-sm join-item" title="Redo (Ctrl+Shift+Z)" aria-label="Redo" data-testid="redo" disabled={!canRedo} onClick={() => api.getState().redo()}><IconRedo /></button>
        </div>
        <div className="join" role="group" aria-label="Node density">
          {DENSITIES.map((d) => (
            <button key={d.id} className={`btn btn-sm join-item ${density === d.id ? "btn-active" : ""}`} aria-pressed={density === d.id} title={`${d.label} nodes (${d.hint})`} data-testid={`density-${d.id}`} onClick={() => api.getState().changeDensity(d.id)}>{d.label}</button>
          ))}
        </div>
        <div className="dropdown dropdown-end">
          <button tabIndex={0} className="btn btn-sm" data-testid="new-menu"><IconFile />New</button>
          <ul tabIndex={0} className="menu dropdown-content z-30 mt-1 w-52 rounded-box border border-base-300 bg-base-100 p-1 shadow-sm">
            <li><button data-testid="new-empty" onClick={() => { api.getState().replace(createEmptyAssembly("untitled-assembly")); (document.activeElement as HTMLElement | null)?.blur(); }}>Empty assembly</button></li>
            <li><button data-testid="new-sample-network" onClick={() => { api.getState().replace(sampleNetworkedStack()); (document.activeElement as HTMLElement | null)?.blur(); }}>Commerce stack + network</button></li>
            <li><button data-testid="new-sample" onClick={() => { api.getState().replace(sampleCommerceStack()); (document.activeElement as HTMLElement | null)?.blur(); }}>Sample commerce stack</button></li>
          </ul>
        </div>
        <button className="btn btn-sm" data-testid="import" onClick={() => file.current?.click()}><IconUpload />Import</button>
        <input ref={file} type="file" accept="application/json,.json" hidden data-testid="import-input" onChange={(e) => { const f = e.target.files?.[0]; if (f) void doImport(f); e.target.value = ""; }} />
        <button className="btn btn-sm btn-primary" data-testid="export" onClick={doExport}><IconDownload />Export JSON</button>
        <select className="select select-sm w-24" aria-label="Theme" value={theme} onChange={(e) => onTheme(e.target.value)}>{THEMES.map((t) => <option key={t} value={t}>{t}</option>)}</select>
      </div>
    </header>
  );
}

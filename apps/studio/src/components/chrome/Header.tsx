import { useRef, useState } from "react";
import { PLATFORM_INFO, createEmptyAssembly, ensureNics, parseAssembly, platformOf, serializeAssembly, usedCustomClasses, type Density } from "@apelora/assembly-model";
import { lookupClass, sampleCommerceStack, sampleNetworkedStack } from "@apelora/appliance-catalog";
import { useCustomCatalog } from "../../features/catalog/customCatalog";
import { useLive } from "../../features/deploy/live";
import { useStudio, useStudioApi } from "../../features/editor/context";
import type { DesignerBinding, DraftSync } from "../../features/applications/draftSync";
import { onLinkClick, paths } from "../../features/router";
import { DeployDialog } from "../deploy/DeployDialog";
import { IconChevron, IconClock, IconDownload, IconFile, IconHelp, IconImage, IconRedo, IconSearch, IconShare, IconUndo, IconUpload } from "../icons";
import { assemblySvg, printSvg, saveBlob, svgToPng } from "../../features/export/image";

const DENSITIES: { id: Density; label: string; hint: string }[] = [
  { id: "compact", label: "Compact", hint: "128 × 96" },
  { id: "comfortable", label: "Comfortable", hint: "152 × 112" },
  { id: "expanded", label: "Expanded", hint: "178 × 150" },
];
const THEMES = ["light", "winter", "night"];

const SYNC_TEXT: Record<DraftSync["kind"], string> = { saved: "Draft saved", dirty: "Unsaved changes…", saving: "Saving draft…", conflict: "Draft changed elsewhere", error: "Could not save the draft" };

export function Header({ theme, onTheme, saved, designer }: { theme: string; onTheme: (t: string) => void; saved: boolean; designer?: { binding: DesignerBinding; sync: DraftSync } }) {
  const api = useStudioApi();
  const name = useStudio((s) => s.assembly.metadata.name);
  const density = useStudio((s) => s.assembly.editor.density);
  const canUndo = useStudio((s) => s.past.length > 0);
  const canRedo = useStudio((s) => s.future.length > 0);
  const platform = useStudio((s) => platformOf(s.assembly));
  const file = useRef<HTMLInputElement>(null);
  const [deploying, setDeploying] = useState(false);
  const liveApp = useLive((s) => s.application);
  const liveProvider = useLive((s) => s.provider);
  const running = useLive((s) => Object.values(s.byAppliance).flat().filter((a) => a.status === "Running").length);
  const live = liveApp ? { application: liveApp, provider: liveProvider, count: running } : null;

  /** Publishing and deploying use what is saved: wait until the draft on the server equals the screen. */
  const afterSave = (next: () => void) => {
    if (!designer) return;
    void designer.sync.flush().then((ok) => { if (ok) next(); else api.getState().toast("error", "The draft is not saved yet, so it cannot be published or deployed. Resolve the save problem first."); });
  };

  const doImage = async (kind: "svg" | "png" | "print") => {
    (document.activeElement as HTMLElement | null)?.blur();
    const st = api.getState();
    try {
      const img = await assemblySvg(st.assembly, st.routing);
      const base = st.assembly.metadata.name;
      if (kind === "svg") { saveBlob(new Blob([img.svg], { type: "image/svg+xml" }), `${base}.svg`); st.toast("success", `Exported ${base}.svg`); }
      else if (kind === "png") { saveBlob(await svgToPng(img.svg, img.width, img.height), `${base}.png`); st.toast("success", `Exported ${base}.png`); }
      else printSvg(img.svg, base);
    } catch (e) { st.toast("error", `Could not create the image: ${(e as Error).message}`); }
  };

  const doExport = () => {
    const base = api.getState().assembly;
    // Embed the custom appliance classes this assembly uses so the file is self-contained.
    const used = usedCustomClasses(base, lookupClass);
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
    <header className="flex min-h-12 shrink-0 flex-wrap items-center gap-x-3 gap-y-1 border-b border-base-300 bg-base-100 px-3 py-1" data-testid="header">
      <div className="flex shrink-0 items-center gap-2 whitespace-nowrap">
        <img src="/logo.svg" alt="" className="size-6 rounded" />
        <span className="text-sm font-semibold">Apelora Studio</span>
        {live ? (
          <span className="badge badge-ghost badge-sm" title={`Showing the state of ${live.application} ${live.provider === "incus" ? "from Incus" : "from mock data, not a real cluster"}.`} data-testid="mock-badge">
            {live.provider === "incus" ? `Live · ${live.count} running in Incus` : live.provider === "mock" ? "Mock data" : "Connecting…"}
          </span>
        ) : (
          <span className="badge badge-ghost badge-sm" title="Nothing here is deployed to a real host." data-testid="mock-badge">Mock · Design only</span>
        )}
        <button type="button" className={`badge badge-sm ${PLATFORM_INFO[platform].status === "supported" ? "badge-outline" : "badge-warning badge-soft"}`} data-testid="platform-badge" title={`Design target: ${PLATFORM_INFO[platform].name}. Click to change.`} onClick={() => api.getState().openProps({ kind: "assembly" })}>{PLATFORM_INFO[platform].name}{PLATFORM_INFO[platform].status === "planned" ? " (planned)" : ""}</button>
      </div>
      {designer ? (
        <nav className="flex min-w-0 items-center gap-1 whitespace-nowrap text-xs text-base-content/60" aria-label="Breadcrumb" data-testid="designer-breadcrumb">
          <a href={paths.applications()} onClick={onLinkClick} className="hover:underline">Applications</a><span>/</span>
          <a href={paths.application(designer.binding.name)} onClick={onLinkClick} className="max-w-40 truncate hover:underline" data-testid="designer-app-link">{designer.binding.name}</a><span>/</span>
          <span className="text-base-content">Designer</span>
          <span className={`ml-2 ${designer.sync.kind === "error" || designer.sync.kind === "conflict" ? "text-error" : ""}`} data-testid="save-state" data-sync={designer.sync.kind} role="status" title={designer.sync.message ?? undefined}>{SYNC_TEXT[designer.sync.kind]}</span>
        </nav>
      ) : (
        <nav className="hidden min-w-0 items-center gap-1 whitespace-nowrap text-xs text-base-content/60 min-[1400px]:flex" aria-label="Breadcrumb">
          <span>Assemblies</span><span>/</span><span className="max-w-40 truncate text-base-content" data-testid="breadcrumb-name">{name}</span>
          <span className="ml-2" data-testid="save-state">{saved ? "Saved locally" : "Saving…"}</span>
        </nav>
      )}
      <div className="ml-auto flex flex-wrap items-center justify-end gap-2">
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
            <li className="menu-title px-2 pt-1 text-[10px]">More</li>
            <li><button data-testid="new-template" onClick={() => { api.getState().setDialog("templates"); (document.activeElement as HTMLElement | null)?.blur(); }}>From template…</button></li>
            <li><button data-testid="new-compose" onClick={() => { api.getState().setDialog("compose"); (document.activeElement as HTMLElement | null)?.blur(); }}>From Docker Compose…</button></li>
          </ul>
        </div>
        <button className="btn btn-sm" data-testid="import" onClick={() => file.current?.click()}><IconUpload />Import</button>
        <input ref={file} type="file" accept="application/json,.json" hidden data-testid="import-input" onChange={(e) => { const f = e.target.files?.[0]; if (f) void doImport(f); e.target.value = ""; }} />
        {designer ? (
          <>
            <button className="btn btn-sm" data-testid="publish-open" onClick={() => afterSave(designer.binding.onPublish)} title="Freeze the saved draft as an immutable version">Publish version…</button>
            <button className="btn btn-sm" data-testid="deploy-version-open" onClick={() => afterSave(designer.binding.onDeploy)} title="Deploy a published version of this application">Deploy version…</button>
          </>
        ) : (
          <button className="btn btn-sm" data-testid="deploy-open" onClick={() => setDeploying(true)} title="Run this design on the cluster through the Apelora control plane">Deploy…</button>
        )}
        <div className="join">
          <button className="btn btn-sm btn-primary join-item" data-testid="export" onClick={doExport}><IconDownload />Export JSON</button>
          <div className="dropdown dropdown-end">
            <button tabIndex={0} className="btn btn-sm btn-primary join-item px-2" aria-label="More export options" data-testid="export-menu"><IconChevron size={14} /></button>
            <ul tabIndex={0} className="menu dropdown-content z-30 mt-1 w-48 rounded-box border border-base-300 bg-base-100 p-1 shadow-sm">
              <li><button data-testid="export-svg" onClick={() => void doImage("svg")}><IconImage size={14} />Image (SVG)</button></li>
              <li><button data-testid="export-png" onClick={() => void doImage("png")}><IconImage size={14} />Image (PNG)</button></li>
              <li><button data-testid="export-print" onClick={() => void doImage("print")}><IconDownload size={14} />Print…</button></li>
              <li><button data-testid="export-share" onClick={() => { api.getState().setDialog("share"); (document.activeElement as HTMLElement | null)?.blur(); }}><IconShare size={14} />Share link…</button></li>
            </ul>
          </div>
        </div>
        <div className="join" role="group" aria-label="Tools">
          <button className="btn btn-sm join-item" aria-label="Find (Ctrl+K)" title="Find an appliance (Ctrl+K)" data-testid="find-open" onClick={() => api.getState().setDialog("find")}><IconSearch size={14} /></button>
          <button className="btn btn-sm join-item" aria-label="Snapshots" title="Snapshots: save, compare and restore" data-testid="snapshots-open" onClick={() => api.getState().setDialog("snapshots")}><IconClock size={14} /></button>
          <button className="btn btn-sm join-item" aria-label="Keyboard shortcuts" title="Keyboard shortcuts (?)" data-testid="help-open" onClick={() => api.getState().setDialog("help")}><IconHelp size={14} /></button>
        </div>
        <select className="select select-sm w-24" aria-label="Theme" value={theme} onChange={(e) => onTheme(e.target.value)}>{THEMES.map((t) => <option key={t} value={t}>{t}</option>)}</select>
      </div>
      {!designer && <DeployDialog open={deploying} onClose={() => setDeploying(false)} />}
    </header>
  );
}

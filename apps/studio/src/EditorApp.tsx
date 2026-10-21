import { useEffect, useMemo, useState } from "react";
import { ReactFlowProvider, useReactFlow } from "@xyflow/react";
import { sampleCommerceStack } from "@apelora/module-catalog";
import { createStudioStore } from "./features/editor/store";
import { StudioContext, useStudioApi } from "./features/editor/context";
import { loadAssembly, loadView, saveAssembly, saveView } from "./features/editor/persistence";
import { useTheme } from "./features/theme";
import { useDraftSync, type DesignerBinding } from "./features/applications/draftSync";
import { DraftConflictDialog } from "./components/applications/DraftConflictDialog";
import type { ApplicationAssembly } from "@apelora/assembly-model";
import { Header } from "./components/chrome/Header";
import { Toasts } from "./components/chrome/Toasts";
import { ModuleCatalog } from "./components/catalog/ModuleCatalog";
import { AssemblyWorkspace } from "./components/canvas/AssemblyWorkspace";
import { renameSelected } from "./features/editor/menus";
import { PropertiesWindow } from "./components/inspector/PropertiesWindow";
import { ModuleDesigner } from "./components/designer/ModuleDesigner";
import { ComponentDialog } from "./components/designer/ComponentDialog";
import { ms } from "./features/editor/motion";
import { TemplatesDialog } from "./components/chrome/TemplatesDialog";
import { SnapshotsDialog } from "./components/chrome/SnapshotsDialog";
import { ComposeDialog } from "./components/chrome/ComposeDialog";
import { ShareDialog } from "./components/chrome/ShareDialog";
import { HelpDialog } from "./components/chrome/HelpDialog";
import { FindPalette } from "./components/chrome/FindPalette";
import { Tour } from "./components/chrome/Tour";
import { decodeShare, SHARE_PREFIX } from "./features/share/link";
import { useCustomCatalog } from "./features/catalog/customCatalog";
import { addSnapshot } from "./features/snapshots/snapshots";

function Shortcuts() {
  const api = useStudioApi();
  const { zoomIn, zoomOut, fitView, screenToFlowPosition } = useReactFlow();
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT" || t.isContentEditable)) return;
      const mod = e.ctrlKey || e.metaKey;
      const k = e.key.toLowerCase();
      const st0 = api.getState();
      if (st0.dialog && !mod) return;
      if (mod && k === "z") { e.preventDefault(); if (e.shiftKey) api.getState().redo(); else api.getState().undo(); }
      else if (mod && k === "y") { e.preventDefault(); api.getState().redo(); }
      else if (mod && k === "c") { e.preventDefault(); api.getState().copySelection(); }
      else if (mod && k === "v") { e.preventDefault(); api.getState().pasteAt(); }
      else if (mod && k === "g") { e.preventDefault(); if (e.shiftKey) api.getState().ungroupSelection(); else api.getState().openGroupDialog(); }
      else if (mod && k === "d") { e.preventDefault(); api.getState().duplicateSelection(); }
      else if (mod && k === "k") { e.preventDefault(); api.getState().setDialog("find"); }
      else if (!mod && e.key === "?") { e.preventDefault(); api.getState().setDialog("help"); }
      else if (!mod && e.shiftKey && k === "t") { e.preventDefault(); api.getState().tidy(); }
      else if (!mod && e.key.startsWith("Arrow") && !st0.menu && (api.getState().selection.nodes.length > 0 || (api.getState().selection.shapes?.length ?? 0) > 0)) {
        e.preventDefault();
        const st = api.getState();
        const d = e.shiftKey ? 40 : 8;
        const dx = e.key === "ArrowLeft" ? -d : e.key === "ArrowRight" ? d : 0;
        const dy = e.key === "ArrowUp" ? -d : e.key === "ArrowDown" ? d : 0;
        st.nudge(dx, dy);
      }
      else if (mod && k === "a") { e.preventDefault(); api.getState().selectAll(); }
      else if (e.key === "F2") { e.preventDefault(); if (api.getState().selection.nodes.length === 1) renameSelected(api); }
      else if (e.key === "ContextMenu" || (e.shiftKey && e.key === "F10")) {
        e.preventDefault();
        const st = api.getState();
        const id = st.selection.nodes[0];
        const el = id ? document.querySelector<HTMLElement>(`[data-testid=module-node][data-node-id="${id}"]`) : document.querySelector<HTMLElement>("[data-testid=canvas]");
        const r = el?.getBoundingClientRect();
        if (r) st.openMenu({ x: r.left + r.width / 2, y: r.top + r.height / 2, keyboard: true, target: id ? { kind: "node", id } : { kind: "pane", flow: screenToFlowPosition({ x: r.left + r.width / 2, y: r.top + r.height / 2 }) } });
      }
      else if (!mod && !e.shiftKey && k === "v") api.getState().setTool("select");
      else if (!mod && !e.shiftKey && k === "h") api.getState().setTool("pan");
      else if (!mod && e.key === "Enter") { const st = api.getState(); if (st.selection.nodes.length === 1 && !st.menu) { e.preventDefault(); st.openProps({ kind: "module", id: st.selection.nodes[0]! }); } }
      else if (!mod && (e.key === "+" || e.key === "=")) void zoomIn({ duration: ms(120) });
      else if (!mod && e.key === "-") void zoomOut({ duration: ms(120) });
      else if (!mod && k === "f") void fitView({ padding: 0.2, maxZoom: 1.2, duration: ms(250) });
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [api, zoomIn, zoomOut, fitView, screenToFlowPosition]);
  return null;
}

/**
 * The editor. Without a `binding` it is the Assembly Library: one diagram kept in this browser. With
 * one it is the Designer of an application: the same canvas, whose document is that application's
 * draft, saved to its backend and never to the library.
 */
export function EditorApp({ initial, binding }: { initial?: ApplicationAssembly; binding?: DesignerBinding }) {
  const { store, loadError } = useMemo(() => {
    if (initial) return { store: createStudioStore(initial, loadView()), loadError: undefined };
    const r = loadAssembly();
    return { store: createStudioStore(r.assembly ?? sampleCommerceStack(), loadView()), loadError: r.error };
  }, [initial]);
  const [theme, setTheme] = useTheme();
  if (import.meta.env.DEV) (window as unknown as { __studio?: unknown }).__studio = store;
  const [saved, setSaved] = useState(true);
  const sync = useDraftSync(store, binding);

  // Opening a share link: validate it like an import, keep the previous diagram as a snapshot, then replace.
  useEffect(() => {
    if (binding || !location.hash.startsWith(SHARE_PREFIX)) return;
    const fragment = location.hash;
    history.replaceState(null, "", location.pathname + location.search);
    void decodeShare(fragment).then((r) => {
      if (!r.ok) { store.getState().setImportErrors(r.errors); return; }
      const before = store.getState().assembly;
      if (before.spec.modules.length > 0) addSnapshot(`Before opening a shared link`, before);
      useCustomCatalog.getState().addMissing(r.customClasses);
      store.getState().replace(r.assembly);
      store.getState().toast("success", `Opened the shared diagram “${r.assembly.metadata.name}”. Your previous diagram is saved under Snapshots.`);
    });
  }, [store, binding]);
  useEffect(() => { if (loadError) store.getState().toast("warning", loadError); }, [store, loadError]);
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    let pending = false;
    const unsub = store.subscribe((s, prev) => {
      if (s.showGrid !== prev.showGrid || s.showLabels !== prev.showLabels || s.cableStyle !== prev.cableStyle || s.routing !== prev.routing || s.showCategory !== prev.showCategory || s.snap !== prev.snap || s.minimap !== prev.minimap) saveView({ showGrid: s.showGrid, showLabels: s.showLabels, cableStyle: s.cableStyle, routing: s.routing, showCategory: s.showCategory, snap: s.snap, minimap: s.minimap });
      if (s.assembly === prev.assembly || binding) return;
      setSaved(false);
      pending = true;
      clearTimeout(timer);
      timer = setTimeout(() => { pending = false; setSaved(saveAssembly(store.getState().assembly)); }, 400);
    });
    // Leaving the page (closing the tab, going to the Applications list) never loses the last edit.
    const flush = () => { if (!pending || binding) return; pending = false; clearTimeout(timer); saveAssembly(store.getState().assembly); };
    window.addEventListener("pagehide", flush);
    return () => { unsub(); window.removeEventListener("pagehide", flush); flush(); clearTimeout(timer); };
  }, [store, binding]);

  return (
    <StudioContext.Provider value={store}>
      <ReactFlowProvider>
        <Shortcuts />
        {binding && <DraftConflictDialog sync={sync} />}
        <div className="flex h-full flex-col bg-base-100 text-base-content">
          <Header theme={theme} onTheme={setTheme} saved={binding ? sync.saved : saved} designer={binding ? { binding, sync } : undefined} />
          <div className="flex min-h-0 flex-1">
            <ModuleCatalog />
            <main className="flex min-w-0 flex-1 p-3">
              <AssemblyWorkspace renamable={!binding} />
            </main>
          </div>
        </div>
        <Toasts />
        <PropertiesWindow />
        <ModuleDesigner />
        <ComponentDialog />
        <TemplatesDialog />
        <SnapshotsDialog />
        <ComposeDialog />
        <ShareDialog />
        <HelpDialog />
        <FindPalette />
        <Tour />
      </ReactFlowProvider>
    </StudioContext.Provider>
  );
}

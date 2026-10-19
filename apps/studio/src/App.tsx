import { useEffect, useMemo, useState } from "react";
import { ReactFlowProvider, useReactFlow } from "@xyflow/react";
import { sampleCommerceStack } from "@apelora/appliance-catalog";
import { createStudioStore } from "./features/editor/store";
import { StudioContext, useStudioApi } from "./features/editor/context";
import { loadAssembly, loadTheme, loadView, saveAssembly, saveTheme, saveView } from "./features/editor/persistence";
import { Header } from "./components/chrome/Header";
import { Toasts } from "./components/chrome/Toasts";
import { ApplianceCatalog } from "./components/catalog/ApplianceCatalog";
import { AssemblyWorkspace } from "./components/canvas/AssemblyWorkspace";
import { renameSelected } from "./features/editor/menus";
import { PropertiesWindow } from "./components/inspector/PropertiesWindow";
import { ApplianceDesigner } from "./components/designer/ApplianceDesigner";
import { ComponentDialog } from "./components/designer/ComponentDialog";
import { ms } from "./features/editor/motion";

function Shortcuts() {
  const api = useStudioApi();
  const { zoomIn, zoomOut, fitView, screenToFlowPosition } = useReactFlow();
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT" || t.isContentEditable)) return;
      const mod = e.ctrlKey || e.metaKey;
      const k = e.key.toLowerCase();
      if (mod && k === "z") { e.preventDefault(); if (e.shiftKey) api.getState().redo(); else api.getState().undo(); }
      else if (mod && k === "y") { e.preventDefault(); api.getState().redo(); }
      else if (mod && k === "c") { e.preventDefault(); api.getState().copySelection(); }
      else if (mod && k === "v") { e.preventDefault(); api.getState().pasteAt(); }
      else if (mod && k === "g") { e.preventDefault(); if (e.shiftKey) api.getState().ungroupSelection(); else api.getState().openGroupDialog(); }
      else if (mod && k === "d") { e.preventDefault(); api.getState().duplicateSelection(); }
      else if (mod && k === "a") { e.preventDefault(); api.getState().selectAll(); }
      else if (e.key === "F2") { e.preventDefault(); if (api.getState().selection.nodes.length === 1) renameSelected(api); }
      else if (e.key === "ContextMenu" || (e.shiftKey && e.key === "F10")) {
        e.preventDefault();
        const st = api.getState();
        const id = st.selection.nodes[0];
        const el = id ? document.querySelector<HTMLElement>(`[data-testid=appliance-node][data-node-id="${id}"]`) : document.querySelector<HTMLElement>("[data-testid=canvas]");
        const r = el?.getBoundingClientRect();
        if (r) st.openMenu({ x: r.left + r.width / 2, y: r.top + r.height / 2, keyboard: true, target: id ? { kind: "node", id } : { kind: "pane", flow: screenToFlowPosition({ x: r.left + r.width / 2, y: r.top + r.height / 2 }) } });
      }
      else if (!mod && !e.shiftKey && k === "v") api.getState().setTool("select");
      else if (!mod && !e.shiftKey && k === "h") api.getState().setTool("pan");
      else if (!mod && e.key === "Enter") { const st = api.getState(); if (st.selection.nodes.length === 1 && !st.menu) { e.preventDefault(); st.openProps({ kind: "appliance", id: st.selection.nodes[0]! }); } }
      else if (!mod && (e.key === "+" || e.key === "=")) void zoomIn({ duration: ms(120) });
      else if (!mod && e.key === "-") void zoomOut({ duration: ms(120) });
      else if (!mod && k === "f") void fitView({ padding: 0.2, maxZoom: 1.2, duration: ms(250) });
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [api, zoomIn, zoomOut, fitView, screenToFlowPosition]);
  return null;
}

export function App() {
  const { store, loadError } = useMemo(() => {
    const r = loadAssembly();
    return { store: createStudioStore(r.assembly ?? sampleCommerceStack(), loadView()), loadError: r.error };
  }, []);
  const [theme, setTheme] = useState(loadTheme);
  if (import.meta.env.DEV) (window as unknown as { __studio?: unknown }).__studio = store;
  const [saved, setSaved] = useState(true);

  useEffect(() => { document.documentElement.setAttribute("data-theme", theme); saveTheme(theme); }, [theme]);
  useEffect(() => { if (loadError) store.getState().toast("warning", loadError); }, [store, loadError]);
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const unsub = store.subscribe((s, prev) => {
      if (s.showGrid !== prev.showGrid || s.showLabels !== prev.showLabels || s.cableStyle !== prev.cableStyle || s.routing !== prev.routing || s.showCategory !== prev.showCategory || s.snap !== prev.snap) saveView({ showGrid: s.showGrid, showLabels: s.showLabels, cableStyle: s.cableStyle, routing: s.routing, showCategory: s.showCategory, snap: s.snap });
      if (s.assembly === prev.assembly) return;
      setSaved(false);
      clearTimeout(timer);
      timer = setTimeout(() => { setSaved(saveAssembly(store.getState().assembly)); }, 400);
    });
    return () => { unsub(); clearTimeout(timer); };
  }, [store]);

  return (
    <StudioContext.Provider value={store}>
      <ReactFlowProvider>
        <Shortcuts />
        <div className="flex h-full flex-col bg-base-100 text-base-content">
          <Header theme={theme} onTheme={setTheme} saved={saved} />
          <div className="flex min-h-0 flex-1">
            <ApplianceCatalog />
            <main className="flex min-w-0 flex-1 p-3">
              <AssemblyWorkspace />
            </main>
          </div>
        </div>
        <Toasts />
        <PropertiesWindow />
        <ApplianceDesigner />
        <ComponentDialog />
      </ReactFlowProvider>
    </StudioContext.Provider>
  );
}

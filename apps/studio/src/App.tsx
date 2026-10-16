import { useEffect, useMemo, useState } from "react";
import { ReactFlowProvider, useReactFlow } from "@xyflow/react";
import { sampleCommerceStack } from "@apelora/appliance-catalog";
import { createStudioStore } from "./features/editor/store";
import { StudioContext, useStudioApi } from "./features/editor/context";
import { loadAssembly, loadTheme, saveAssembly, saveTheme } from "./features/editor/persistence";
import { Header } from "./components/chrome/Header";
import { Toasts } from "./components/chrome/Toasts";
import { ApplianceCatalog } from "./components/catalog/ApplianceCatalog";
import { AssemblyWorkspace } from "./components/canvas/AssemblyWorkspace";
import { PropertiesInspector } from "./components/inspector/PropertiesInspector";
import { ApplianceDesigner } from "./components/designer/ApplianceDesigner";

function Shortcuts() {
  const api = useStudioApi();
  const { zoomIn, zoomOut, fitView } = useReactFlow();
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT" || t.isContentEditable)) return;
      const mod = e.ctrlKey || e.metaKey;
      const k = e.key.toLowerCase();
      if (mod && k === "z") { e.preventDefault(); if (e.shiftKey) api.getState().redo(); else api.getState().undo(); }
      else if (mod && k === "y") { e.preventDefault(); api.getState().redo(); }
      else if (!mod && (e.key === "+" || e.key === "=")) void zoomIn({ duration: 120 });
      else if (!mod && e.key === "-") void zoomOut({ duration: 120 });
      else if (!mod && k === "f") void fitView({ padding: 0.2, maxZoom: 1.2, duration: 250 });
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [api, zoomIn, zoomOut, fitView]);
  return null;
}

export function App() {
  const { store, loadError } = useMemo(() => {
    const r = loadAssembly();
    return { store: createStudioStore(r.assembly ?? sampleCommerceStack()), loadError: r.error };
  }, []);
  const [theme, setTheme] = useState(loadTheme);
  if (import.meta.env.DEV) (window as unknown as { __studio?: unknown }).__studio = store;
  const [saved, setSaved] = useState(true);

  useEffect(() => { document.documentElement.setAttribute("data-theme", theme); saveTheme(theme); }, [theme]);
  useEffect(() => { if (loadError) store.getState().toast("warning", loadError); }, [store, loadError]);
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const unsub = store.subscribe((s, prev) => {
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
            <PropertiesInspector />
          </div>
        </div>
        <Toasts />
        <ApplianceDesigner />
      </ReactFlowProvider>
    </StudioContext.Provider>
  );
}

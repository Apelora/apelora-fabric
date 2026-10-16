import { create } from "zustand";
import { allClasses, lookupClass, sampleCommerceStack } from "@apelora/appliance-catalog";
import {
  addAppliance, addPort, connect, reconnect, alignNodes, disconnectAppliance, disconnectPort, extractSubset, insertSubset, type AlignMode, type Subset, DENSITY_SIZE, findFreeSpot, createEmptyAssembly, disconnect, movePort, moveNodes, removeAppliances, removePort,
  renameAppliance, resetNodeSize, resizeNode, setDensity, setResources, updatePort, validateAssembly,
  type ApplicationAssembly, type Density, type Diagnostic, type Endpoint, type PortDirection, type PortType, type Resources, type Port,
} from "@apelora/assembly-model";

export interface Toast { id: number; kind: "info" | "success" | "warning" | "error"; text: string }
interface Selection { nodes: string[]; edges: string[] }

export type MenuTarget =
  | { kind: "node"; id: string }
  | { kind: "port"; nodeId: string; portId: string }
  | { kind: "edge"; id: string }
  | { kind: "pane"; flow: { x: number; y: number } };
export interface MenuState { x: number; y: number; target: MenuTarget; keyboard?: boolean }

const MAX_HISTORY = 100;
let toastSeq = 1;

export interface StudioState {
  assembly: ApplicationAssembly;
  past: ApplicationAssembly[];
  future: ApplicationAssembly[];
  gestureStart: ApplicationAssembly | null;
  selection: Selection;
  toasts: Toast[];
  /** Incremented whenever the viewport should refit (new/import/sample). */
  fitNonce: number;
  showGrid: boolean;
  showLabels: boolean;
  cableStyle: "line" | "cable";
  snap: boolean;
  importErrors: string[] | null;
  menu: MenuState | null;
  clipboard: Subset | null;

  openMenu: (m: MenuState) => void;
  closeMenu: () => void;
  selectAll: () => void;
  copySelection: () => void;
  pasteAt: (flow?: { x: number; y: number }) => void;
  duplicateSelection: () => void;
  disconnectPortCables: (id: string, portId: string) => void;
  disconnectAllCables: (id: string) => void;
  align: (mode: AlignMode) => void;

  toast: (kind: Toast["kind"], text: string) => void;
  dismissToast: (id: number) => void;
  setSelection: (s: Partial<Selection>) => void;
  selectNode: (id: string) => void;

  commit: (next: ApplicationAssembly) => void;
  beginGesture: () => void;
  transient: (next: ApplicationAssembly) => void;
  endGesture: () => void;
  undo: () => void;
  redo: () => void;
  replace: (next: ApplicationAssembly, opts?: { resetHistory?: boolean; fit?: boolean }) => void;

  addFromCatalog: (classId: string, pos: { x: number; y: number }, opts?: { avoidOverlap?: boolean }) => string | null;
  deleteElements: (nodeIds: string[], edgeIds: string[]) => void;
  moveTransient: (moves: Map<string, { x: number; y: number }>) => void;
  resizeTransient: (id: string, size: { width: number; height: number }, pos?: { x: number; y: number }) => void;
  resetSize: (id: string) => void;
  connectPorts: (s: Endpoint, t: Endpoint) => boolean;
  reconnectCable: (id: string, s: Endpoint, t: Endpoint) => void;
  rename: (id: string, name: string) => void;
  setRes: (id: string, r: Partial<Resources>) => void;
  addPortTo: (id: string, direction: PortDirection, type?: PortType) => void;
  editPort: (id: string, portId: string, patch: Partial<Pick<Port, "name" | "type" | "optional" | "multiplicity">>) => void;
  deletePort: (id: string, portId: string) => void;
  shiftPort: (id: string, portId: string, delta: -1 | 1) => void;
  changeDensity: (d: Density) => void;
  renameAssembly: (name: string) => void;
  setShowGrid: (v: boolean) => void;
  setShowLabels: (v: boolean) => void;
  setCableStyle: (v: "line" | "cable") => void;
  setSnap: (v: boolean) => void;
  setImportErrors: (e: string[] | null) => void;
}

const prune = (a: ApplicationAssembly, s: Selection): Selection => ({
  nodes: s.nodes.filter((id) => a.spec.appliances.some((x) => x.id === id)),
  edges: s.edges.filter((id) => a.spec.connections.some((c) => c.id === id)),
});

export function createStudioStore(initial: ApplicationAssembly) {
  return create<StudioState>()((set, get) => {
    const commit = (next: ApplicationAssembly) => {
      const { assembly, past, selection } = get();
      if (next === assembly) return;
      set({ assembly: next, past: [...past, assembly].slice(-MAX_HISTORY), future: [], selection: prune(next, selection) });
    };
    return {
      assembly: initial,
      past: [],
      future: [],
      gestureStart: null,
      selection: { nodes: [], edges: [] },
      toasts: [],
      fitNonce: 0,
      showGrid: true,
      showLabels: true,
      cableStyle: "line",
      snap: false,
      importErrors: null,
      menu: null,
      clipboard: null,

      openMenu: (m) => {
        const { selection } = get();
        // Right-clicking something outside the current selection selects it first.
        if (m.target.kind === "node" || m.target.kind === "port") {
          const id = m.target.kind === "node" ? m.target.id : m.target.nodeId;
          if (!selection.nodes.includes(id)) set({ selection: { nodes: [id], edges: [] } });
        } else if (m.target.kind === "edge" && !selection.edges.includes(m.target.id)) set({ selection: { nodes: [], edges: [m.target.id] } });
        set({ menu: m });
      },
      closeMenu: () => set({ menu: null }),
      selectAll: () => set({ selection: { nodes: get().assembly.spec.appliances.map((a) => a.id), edges: [] } }),
      copySelection: () => {
        const { assembly, selection } = get();
        if (!selection.nodes.length) return;
        set({ clipboard: extractSubset(assembly, selection.nodes) });
        get().toast("info", `Copied ${selection.nodes.length} appliance${selection.nodes.length > 1 ? "s" : ""}. Paste with Ctrl+V.`);
      },
      pasteAt: (flow) => {
        const { clipboard, assembly } = get();
        if (!clipboard || !clipboard.nodes.length) return;
        const minX = Math.min(...clipboard.nodes.map((n) => n.x));
        const minY = Math.min(...clipboard.nodes.map((n) => n.y));
        const delta = flow ? { x: flow.x - minX, y: flow.y - minY } : { x: 32, y: 32 };
        const r = insertSubset(assembly, clipboard, delta);
        commit(r.assembly);
        set({ selection: { nodes: r.ids, edges: [] } });
      },
      duplicateSelection: () => {
        const { assembly, selection } = get();
        if (!selection.nodes.length) return;
        const r = insertSubset(assembly, extractSubset(assembly, selection.nodes), { x: 32, y: 32 });
        commit(r.assembly);
        set({ selection: { nodes: r.ids, edges: [] } });
      },
      disconnectPortCables: (id, portId) => commit(disconnectPort(get().assembly, id, portId)),
      disconnectAllCables: (id) => commit(disconnectAppliance(get().assembly, id)),
      align: (mode) => commit(alignNodes(get().assembly, get().selection.nodes, mode)),

      toast: (kind, text) => {
        const id = toastSeq++;
        set((s) => ({ toasts: [...s.toasts.slice(-3), { id, kind, text }] }));
        setTimeout(() => get().dismissToast(id), kind === "error" ? 7000 : 4000);
      },
      dismissToast: (id) => set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) })),
      setSelection: (s) => set((st) => ({ selection: { ...st.selection, ...s } })),
      selectNode: (id) => set({ selection: { nodes: [id], edges: [] } }),

      commit,
      beginGesture: () => set({ gestureStart: get().assembly }),
      transient: (next) => set({ assembly: next }),
      endGesture: () => {
        const { gestureStart, assembly, past } = get();
        if (gestureStart && gestureStart !== assembly) set({ past: [...past, gestureStart].slice(-MAX_HISTORY), future: [] });
        set({ gestureStart: null });
      },
      undo: () => {
        const { past, future, assembly, selection } = get();
        const prev = past[past.length - 1];
        if (!prev) return;
        set({ assembly: prev, past: past.slice(0, -1), future: [assembly, ...future], selection: prune(prev, selection) });
      },
      redo: () => {
        const { past, future, assembly, selection } = get();
        const next = future[0];
        if (!next) return;
        set({ assembly: next, past: [...past, assembly], future: future.slice(1), selection: prune(next, selection) });
      },
      replace: (next, opts = {}) => {
        if (opts.resetHistory) set({ assembly: next, past: [], future: [], selection: { nodes: [], edges: [] } });
        else commit(next);
        if (opts.fit !== false) set((s) => ({ fitNonce: s.fitNonce + 1 }));
      },

      addFromCatalog: (classId, pos, opts) => {
        const cls = allClasses().find((c) => c.id === classId);
        if (!cls) return null;
        const { assembly } = get();
        const at = opts?.avoidOverlap ? findFreeSpot(assembly, pos, DENSITY_SIZE[assembly.editor.density]) : pos;
        const r = addAppliance(assembly, cls, at);
        commit(r.assembly);
        set({ selection: { nodes: [r.id], edges: [] } });
        return r.id;
      },
      deleteElements: (nodeIds, edgeIds) => {
        let a = get().assembly;
        if (edgeIds.length) a = disconnect(a, edgeIds);
        if (nodeIds.length) a = removeAppliances(a, nodeIds);
        commit(a);
      },
      moveTransient: (moves) => set({ assembly: moveNodes(get().assembly, moves) }),
      resizeTransient: (id, size, pos) => set({ assembly: resizeNode(get().assembly, id, size, pos) }),
      resetSize: (id) => commit(resetNodeSize(get().assembly, id)),
      connectPorts: (s, t) => {
        const r = connect(get().assembly, s, t);
        if ("error" in r) {
          get().toast("warning", `${r.error.message}${r.error.hint ? ` ${r.error.hint}` : ""}`);
          return false;
        }
        commit(r.assembly);
        return true;
      },
      reconnectCable: (id, s, t) => {
        const r = reconnect(get().assembly, id, s, t);
        if ("error" in r) { get().toast("warning", `${r.error.message}${r.error.hint ? ` ${r.error.hint}` : ""}`); return; }
        commit(r.assembly);
      },
      rename: (id, name) => commit(renameAppliance(get().assembly, id, name)),
      setRes: (id, r) => commit(setResources(get().assembly, id, r)),
      addPortTo: (id, direction, type) => commit(addPort(get().assembly, id, direction, type).assembly),
      editPort: (id, portId, patch) => commit(updatePort(get().assembly, id, portId, patch)),
      deletePort: (id, portId) => {
        const a = get().assembly;
        const attached = a.spec.connections.filter((c) => (c.source.applianceId === id && c.source.portId === portId) || (c.target.applianceId === id && c.target.portId === portId)).length;
        commit(removePort(a, id, portId));
        if (attached) get().toast("info", `Port removed together with ${attached} connection${attached > 1 ? "s" : ""}. Undo to restore.`);
      },
      shiftPort: (id, portId, delta) => commit(movePort(get().assembly, id, portId, delta)),
      changeDensity: (d) => commit(setDensity(get().assembly, d)),
      renameAssembly: (name) => {
        const a = get().assembly;
        const n = name.trim().slice(0, 80);
        if (n && n !== a.metadata.name) commit({ ...a, metadata: { ...a.metadata, name: n } });
      },
      setShowGrid: (v) => set({ showGrid: v }),
      setShowLabels: (v) => set({ showLabels: v }),
      setCableStyle: (v) => set({ cableStyle: v }),
      setSnap: (v) => set({ snap: v }),
      setImportErrors: (e) => set({ importErrors: e }),
    };
  });
}

export type StudioStore = ReturnType<typeof createStudioStore>;

export function diagnosticsFor(a: ApplicationAssembly): Diagnostic[] {
  return validateAssembly(a, lookupClass);
}

export { createEmptyAssembly, sampleCommerceStack };

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import type { StudioStore } from "../editor/store";
import { assemblyFromDocument, documentOf } from "./document";
import type { AppBackend } from "./model";

/** What the Designer needs to know about the application it edits. */
export interface DesignerBinding {
  name: string;
  backend: AppBackend;
  /** The draft version the editor was loaded from. */
  baseVersion: number;
  projectLabel: string;
  onPublish: () => void;
}

export type SyncKind = "saved" | "dirty" | "saving" | "conflict" | "error";

export interface DraftSync {
  kind: SyncKind;
  version: number;
  message: string | null;
  saved: boolean;
  /** Saves now and resolves true when the draft on the server equals what is on screen. */
  flush: () => Promise<boolean>;
  /** Discards the screen's changes and loads the server's newest draft. */
  loadLatest: () => Promise<void>;
  /** Keeps the screen's changes and saves them over the server's newest draft (the person chose this). */
  overwrite: () => Promise<void>;
  pause: () => void;
}

const DELAY_MS = 900;

/**
 * Saves the editor's document as the application's draft, a moment after each change. A save names
 * the version it started from; if someone else saved since, the server refuses and saving stops
 * until the person decides (load theirs, or overwrite deliberately). Nothing is lost silently.
 */
export function useDraftSync(store: StudioStore, binding: DesignerBinding | undefined): DraftSync {
  const [state, setState] = useState<{ kind: SyncKind; version: number; message: string | null }>({ kind: "saved", version: binding?.baseVersion ?? 0, message: null });
  const version = useRef(binding?.baseVersion ?? 0);
  const kind = useRef<SyncKind>("saved");
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const inflight = useRef<Promise<boolean> | null>(null);
  const dirty = useRef(false);
  const set = useCallback((k: SyncKind, message: string | null = null, v = version.current) => {
    kind.current = k;
    version.current = v;
    setState({ kind: k, version: v, message });
  }, []);

  const save = useCallback((): Promise<boolean> => {
    if (!binding) return Promise.resolve(true);
    if (inflight.current) return inflight.current.then(() => (dirty.current ? save() : kind.current === "saved"));
    if (kind.current === "conflict") return Promise.resolve(false);
    clearTimeout(timer.current);
    dirty.current = false;
    set("saving");
    const doc = documentOf(store.getState().assembly);
    const p = binding.backend
      .saveDraft(binding.name, doc, version.current || null)
      .then((r) => {
        if (r.ok) {
          set(dirty.current ? "dirty" : "saved", null, r.version);
          if (dirty.current) timer.current = setTimeout(() => void save(), DELAY_MS);
          return !dirty.current;
        }
        set("conflict", `Someone saved version ${r.currentVersion} of this draft since you opened it.`, r.currentVersion);
        return false;
      })
      .catch((e: unknown) => {
        dirty.current = true;
        set("error", e instanceof Error ? e.message : String(e));
        timer.current = setTimeout(() => void save(), 5000);
        return false;
      })
      .finally(() => { inflight.current = null; });
    inflight.current = p;
    return p;
  }, [binding, store, set]);

  useEffect(() => {
    if (!binding) return;
    const unsub = store.subscribe((s, prev) => {
      if (s.assembly === prev.assembly) return;
      dirty.current = true;
      if (kind.current === "conflict" || inflight.current) return;
      set("dirty");
      clearTimeout(timer.current);
      timer.current = setTimeout(() => void save(), DELAY_MS);
    });
    return () => { unsub(); clearTimeout(timer.current); if (dirty.current && kind.current !== "conflict") void save(); };
  }, [binding, store, save, set]);

  const loadLatest = useCallback(async () => {
    if (!binding) return;
    const d = await binding.backend.getDraft(binding.name);
    if (!d) return;
    const r = assemblyFromDocument(d.document);
    if (!r.ok) { set("error", `The newest draft cannot be read: ${r.errors[0] ?? "invalid"}`); return; }
    dirty.current = false;
    store.getState().replace(r.assembly, { resetHistory: true });
    set("saved", null, d.version);
  }, [binding, store, set]);

  const overwrite = useCallback(async () => {
    if (!binding) return;
    const d = await binding.backend.getDraft(binding.name);
    version.current = d?.version ?? version.current;
    kind.current = "dirty";
    dirty.current = true;
    await save();
  }, [binding, save]);

  return useMemo(() => ({ ...state, saved: state.kind === "saved", flush: async () => { if (dirty.current || kind.current === "dirty" || kind.current === "error") return save(); return inflight.current ? inflight.current : kind.current === "saved"; }, loadLatest, overwrite, pause: () => clearTimeout(timer.current) }), [state, save, loadLatest, overwrite]);
}

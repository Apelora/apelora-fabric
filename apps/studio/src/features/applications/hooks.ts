import { useCallback, useEffect, useRef, useState } from "react";

import { useBackend } from "./backend";
import type { AppBackend, AppDetailView, ListResult } from "./model";

export type Load<T> = { kind: "loading" } | { kind: "error"; message: string } | { kind: "ready"; data: T };

/**
 * Loads data from the current backend, refreshes it on an interval while the tab is visible
 * (runtime status changes under you), and reloads when the backend switches.
 */
export function useLoad<T>(fetcher: (b: AppBackend) => Promise<T>, deps: unknown[], intervalMs = 0): [Load<T>, () => void] {
  const { backend, status } = useBackend();
  const [state, setState] = useState<Load<T>>({ kind: "loading" });
  const seq = useRef(0);
  const run = useCallback(
    (quiet: boolean) => {
      const n = ++seq.current;
      if (!quiet) setState((s) => (s.kind === "ready" ? s : { kind: "loading" }));
      fetcher(backend).then(
        (data) => { if (n === seq.current) setState({ kind: "ready", data }); },
        (e: unknown) => { if (n === seq.current) setState((s) => (quiet && s.kind === "ready" ? s : { kind: "error", message: e instanceof Error ? e.message : String(e) })); },
      );
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps -- the fetcher closes over `deps`
    [backend, ...deps],
  );
  useEffect(() => {
    if (status !== "ready") return;
    run(false);
    if (!intervalMs) return;
    const t = setInterval(() => { if (document.visibilityState === "visible") run(true); }, intervalMs);
    // Invalidates answers still in flight; the counter itself is what is read later, so the current value is wanted.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    return () => { clearInterval(t); seq.current++; };
  }, [run, status, intervalMs]);
  return [state, () => run(true)];
}

export const useAppList = (includeArchived: boolean, intervalMs = 5000) => useLoad<ListResult>((b) => b.list(includeArchived), [includeArchived], intervalMs);
export const useAppDetail = (name: string, intervalMs = 5000) => useLoad<AppDetailView>((b) => b.get(name), [name], intervalMs);

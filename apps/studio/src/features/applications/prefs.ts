import type { DeploymentStatus, DesignStatus } from "./model";

const KEY = "apelora.studio.apps.prefs.v1";

export type ViewMode = "cards" | "list";
export type SortKey = "name" | "status" | "updated";

export interface Prefs {
  view: ViewMode;
  sort: SortKey;
  desc: boolean;
  pageSize: number;
}

const DEFAULTS: Prefs = { view: "cards", sort: "name", desc: false, pageSize: 12 };

export function loadPrefs(): Prefs {
  try {
    const v = JSON.parse(localStorage.getItem(KEY) ?? "{}") as Partial<Prefs>;
    return {
      view: v.view === "list" || v.view === "cards" ? v.view : DEFAULTS.view,
      sort: v.sort === "status" || v.sort === "updated" || v.sort === "name" ? v.sort : DEFAULTS.sort,
      desc: typeof v.desc === "boolean" ? v.desc : DEFAULTS.desc,
      pageSize: [12, 24, 48].includes(v.pageSize ?? 0) ? (v.pageSize as number) : DEFAULTS.pageSize,
    };
  } catch {
    return DEFAULTS;
  }
}

export function savePrefs(p: Prefs): void {
  try { localStorage.setItem(KEY, JSON.stringify(p)); } catch { /* storage unavailable */ }
}

/** Order for sorting by status: things that need attention first. */
export const STATUS_ORDER: Record<DeploymentStatus, number> = { failed: 0, degraded: 1, unknown: 2, starting: 3, stopping: 4, running: 5, stopped: 6, "not-deployed": 7 };
export const DESIGN_ORDER: Record<DesignStatus, number> = { draft: 0, validated: 1, published: 2 };

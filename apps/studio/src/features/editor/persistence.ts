import { ensureNics, parseAssembly, serializeAssembly, type ApplicationAssembly } from "@apelora/assembly-model";
import { lookupClass } from "@apelora/module-catalog";

const KEY = "apelora.studio.v1";
const BACKUP = "apelora.studio.v1.invalid";
const THEME = "apelora.studio.theme";

interface Envelope { version: 1; savedAt: string; assembly: string }

export type LoadResult = { assembly: ApplicationAssembly | null; error?: string };

export function loadAssembly(): LoadResult {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return { assembly: null };
    const env = JSON.parse(raw) as Partial<Envelope>;
    if (env.version !== 1 || typeof env.assembly !== "string") throw new Error("unsupported storage format");
    const r = parseAssembly(env.assembly);
    if (r.ok) return { assembly: ensureNics(r.assembly, lookupClass) };
    // Never silently discard: keep the unreadable copy so it can be recovered.
    localStorage.setItem(BACKUP, raw);
    return { assembly: null, error: `Saved work could not be read (${r.errors[0] ?? "invalid"}). A backup was kept under "${BACKUP}".` };
  } catch (e) {
    return { assembly: null, error: `Saved work could not be loaded: ${(e as Error).message}` };
  }
}

export function saveAssembly(a: ApplicationAssembly): boolean {
  try {
    const env: Envelope = { version: 1, savedAt: new Date().toISOString(), assembly: serializeAssembly(a) };
    localStorage.setItem(KEY, JSON.stringify(env));
    return true;
  } catch {
    return false;
  }
}

export const loadTheme = (): string => { try { return localStorage.getItem(THEME) ?? "light"; } catch { return "light"; } };
export const saveTheme = (t: string): void => { try { localStorage.setItem(THEME, t); } catch { /* storage unavailable */ } };

const VIEW = "apelora.studio.view.v1";
export interface ViewPrefs { showGrid: boolean; showLabels: boolean; cableStyle: "line" | "cable"; routing: "curved" | "angled"; showCategory: boolean; snap: boolean; minimap: boolean }

export function loadView(): Partial<ViewPrefs> {
  try {
    const v = JSON.parse(localStorage.getItem(VIEW) ?? "{}") as Record<string, unknown>;
    const out: Partial<ViewPrefs> = {};
    if (typeof v.showGrid === "boolean") out.showGrid = v.showGrid;
    if (typeof v.showLabels === "boolean") out.showLabels = v.showLabels;
    if (v.cableStyle === "line" || v.cableStyle === "cable") out.cableStyle = v.cableStyle;
    if (v.routing === "curved" || v.routing === "angled") out.routing = v.routing;
    if (typeof v.showCategory === "boolean") out.showCategory = v.showCategory;
    if (typeof v.snap === "boolean") out.snap = v.snap;
    if (typeof v.minimap === "boolean") out.minimap = v.minimap;
    return out;
  } catch { return {}; }
}
export function saveView(p: ViewPrefs): void { try { localStorage.setItem(VIEW, JSON.stringify(p)); } catch { /* storage unavailable */ } }

const TOUR = "apelora.studio.tour.v1";
export const tourSeen = (): boolean => { try { return localStorage.getItem(TOUR) === "1"; } catch { return true; } };
export const markTourSeen = (): void => { try { localStorage.setItem(TOUR, "1"); } catch { /* storage unavailable */ } };

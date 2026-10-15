import { parseAssembly, serializeAssembly, type ApplicationAssembly } from "@apelora/assembly-model";

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
    if (r.ok) return { assembly: r.assembly };
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

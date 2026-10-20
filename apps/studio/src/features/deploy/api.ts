import { createClient } from "@apelora/api-client";

/** The API token lives only in this browser tab (sessionStorage), never in localStorage. */
const TOKEN_KEY = "apelora.apiToken";
export const readToken = (): string | null => { try { return sessionStorage.getItem(TOKEN_KEY); } catch { return null; } };
export const writeToken = (t: string | null) => { try { if (t) sessionStorage.setItem(TOKEN_KEY, t); else sessionStorage.removeItem(TOKEN_KEY); } catch { /* storage unavailable */ } };

/** The Studio reaches only the Apelora API (proxied at `/api` by the dev server). */
export const client = createClient({ token: readToken });

export const messageOf = (e: unknown): string => (e instanceof Error ? e.message : String(e));

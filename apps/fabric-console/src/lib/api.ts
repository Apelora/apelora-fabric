import { useCallback, useEffect, useRef, useState } from "react";

import { ApiRequestError, createClient, followJob as followJobWith, newIdempotencyKey, type Job } from "@apelora/api-client";

import { log } from "../log";

/** The API token lives only in this browser tab (sessionStorage), never in localStorage. */
const TOKEN_KEY = "apelora.apiToken";
export const readToken = (): string | null => { try { return sessionStorage.getItem(TOKEN_KEY); } catch { return null; } };
export const writeToken = (t: string | null) => { try { if (t) sessionStorage.setItem(TOKEN_KEY, t); else sessionStorage.removeItem(TOKEN_KEY); } catch { /* storage unavailable */ } };

export const client = createClient({ token: readToken });

export type Load<T> = { kind: "loading" } | { kind: "ok"; data: T } | { kind: "error"; message: string };

export const messageOf = (e: unknown): string => (e instanceof Error ? e.message : String(e));

/**
 * Loads `fetcher` on mount and on `reload()`. A 401 calls `onAuth` (the token is missing or no
 * longer valid) so the shell can ask for a new one.
 */
export function useLoad<T>(fetcher: () => Promise<T>, onAuth: (message: string) => void): [Load<T>, () => void] {
  const [state, setState] = useState<Load<T>>({ kind: "loading" });
  const latest = useRef(fetcher);
  latest.current = fetcher;
  const reload = useCallback(() => {
    let current = true;
    latest.current()
      .then((data) => { if (current) setState({ kind: "ok", data }); })
      .catch((e: unknown) => {
        if (!current) return;
        if (e instanceof ApiRequestError && e.unauthenticated) onAuth(e.message);
        else log("error", "request failed", { message: messageOf(e) });
        setState({ kind: "error", message: messageOf(e) });
      });
    return () => { current = false; };
  }, [onAuth]);
  useEffect(reload, [reload]);
  return [state, reload];
}

/** Follows a job until it is final (see `followJob` in the API client). */
export const followJob = (id: string, onUpdate: (job: Job) => void) => followJobWith(client, id, onUpdate);

export const idempotencyKey = newIdempotencyKey;

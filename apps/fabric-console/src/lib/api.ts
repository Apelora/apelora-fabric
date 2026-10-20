import { useCallback, useEffect, useRef, useState } from "react";

import { ApiRequestError, createClient, type Job } from "@apelora/api-client";

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
        setState({ kind: "error", message: messageOf(e) });
      });
    return () => { current = false; };
  }, [onAuth]);
  useEffect(reload, [reload]);
  return [state, reload];
}

const FINAL = new Set(["succeeded", "failed", "cancelled"]);

/**
 * Follows a queued job until it is final, reporting each poll. The worker retries with
 * backoff, so a job can stay pending for a while; it gives up watching (not the job) after
 * `timeoutMs`.
 */
export async function followJob(id: string, onUpdate: (job: Job) => void, timeoutMs = 15 * 60_000, intervalMs = 1000): Promise<Job> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const job = await client.request("job", { params: { id } });
    onUpdate(job);
    if (FINAL.has(job.status) || Date.now() > deadline) return job;
    await new Promise((r) => setTimeout(r, intervalMs));
  }
}

/** A random key so a retried click never queues the same change twice. */
export const idempotencyKey = (): string =>
  globalThis.crypto?.randomUUID?.() ?? `${Date.now()}-${Math.random().toString(16).slice(2)}`;

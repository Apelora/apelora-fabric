import { useEffect, useId, useRef, useState } from "react";

import { ApiRequestError, type Job, type JobList } from "@apelora/api-client";

import { JobStatus, RequestId, STATUS, isActive, jobTarget, kindLabel, useJobControl } from "../components/jobs";
import { ErrorPanel, Section, SectionHeader, TableFrame } from "../components/ui";
import { client, messageOf, useLoad } from "../lib/api";
import { useDebounced } from "../lib/identity";
import { jobHref } from "../lib/routes";

const PAGE = 50;

/** The kinds of change, by the first part of their name (`instance.start` is an "instance" change). */
const AREAS: [string, string][] = [["instance.", "Instances"], ["volume.", "Volumes"], ["network.", "Networks"], ["application.", "Applications"]];

type Filters = { status: string; kind: string; application: string };
const NO_FILTERS: Filters = { status: "", kind: "", application: "" };

/** Filters from the address after `?` (`#/operations?status=failed`), so a filtered list can be shared. */
function filtersFromHash(hash: string): Filters {
  const q = new URLSearchParams(hash.includes("?") ? hash.slice(hash.indexOf("?") + 1) : "");
  const f = { ...NO_FILTERS };
  f.status = Object.hasOwn(STATUS, q.get("status") ?? "") ? (q.get("status") as string) : "";
  f.kind = AREAS.some(([k]) => k === q.get("kind")) ? (q.get("kind") as string) : "";
  f.application = (q.get("application") ?? "").slice(0, 120);
  return f;
}

function hashFor(f: Filters): string {
  const q = new URLSearchParams();
  for (const [k, v] of Object.entries(f)) if (v) q.set(k, v);
  const s = q.toString();
  return `#/operations${s ? `?${s}` : ""}`;
}

/** Jobs that are still pending or running make the list refresh itself every few seconds. */
const busy = (jobs: Job[]) => jobs.some(isActive);

/**
 * What the worker is carrying out and has carried out, newest first: filtered by state, kind
 * and application (kept in the address) and paged. Only the first page refreshes by itself.
 */
export function OperationsView({ onAuth }: { onAuth: (m: string) => void }) {
  const [filters, setFilters] = useState<Filters>(() => filtersFromHash(window.location.hash));
  const application = useDebounced(filters.application.trim(), 300);
  const query = { status: filters.status || undefined, kind: filters.kind || undefined, application: application || undefined, limit: PAGE };
  const key = JSON.stringify(query);
  const [jobs, reload] = useLoad<JobList>(() => client.request("jobs", { query: JSON.parse(key) }), onAuth);
  // Older pages, added by "Older operations"; a refresh or a changed filter starts again.
  const [older, setOlder] = useState<Job[]>([]);
  const [next, setNext] = useState<string | null>(null);
  const [loadingMore, setLoadingMore] = useState(false);
  const [moreError, setMoreError] = useState<string | null>(null);
  const id = useId();

  const first = jobs.kind === "ok" ? jobs.data : null;
  useEffect(() => { setOlder([]); setNext(first?.next ?? null); setMoreError(null); }, [first]);
  useEffect(() => {
    const target = hashFor({ ...filters, application });
    if (window.location.hash !== target) window.history.replaceState(null, "", target);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);
  useEffect(() => {
    const onHash = () => {
      if (window.location.hash.replace(/^#\/?/, "").replace(/\?.*$/, "") === "operations") setFilters(filtersFromHash(window.location.hash));
    };
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, []);
  // Reloads when the filters change (the fetcher reads the latest key); the first load is `useLoad`'s own.
  const loadedKey = useRef(key);
  useEffect(() => {
    if (loadedKey.current === key) return;
    loadedKey.current = key;
    return reload();
  }, [key, reload]);

  const active = !!first && older.length === 0 && busy(first.items);
  useEffect(() => {
    if (!active) return;
    const t = setInterval(reload, 3000);
    return () => clearInterval(t);
  }, [active, reload]);
  const control = useJobControl(() => reload());

  const loadOlder = () => {
    if (!next) return;
    setLoadingMore(true);
    setMoreError(null);
    client.request("jobs", { query: { ...JSON.parse(key), after: next } }).then(
      (r) => { setOlder((prev) => [...prev, ...r.items]); setNext(r.next ?? null); },
      (e: unknown) => {
        if (e instanceof ApiRequestError && e.unauthenticated) onAuth(e.message);
        setMoreError(messageOf(e));
      },
    ).finally(() => setLoadingMore(false));
  };

  const set = (k: keyof Filters, v: string) => setFilters((f) => ({ ...f, [k]: v }));
  const filtered = Object.values(filters).some((v) => v !== "");

  return (
    <Section title="Operations" level={1} state={jobs} testId="operations" onRetry={reload}>
      {(l) => {
        const items = [...l.items, ...older];
        return (
          <>
            <SectionHeader title="Operations" level={1} count={items.length}>
              <button type="button" className="btn btn-sm btn-ghost" onClick={reload}>Refresh</button>
            </SectionHeader>
            <p className="text-sm text-base-content/70">
              Changes the worker carries out, newest first.{active ? " Updates every few seconds while some are in progress." : ""}
            </p>
            <div className="flex flex-wrap items-end gap-3 rounded-box border border-base-300 p-3" role="group" aria-label="Filters">
              <div className="flex w-full flex-col gap-1 text-sm sm:w-40">
                <label htmlFor={`${id}-status`}>State</label>
                <select id={`${id}-status`} className="select select-sm w-full" value={filters.status} onChange={(e) => set("status", e.target.value)} data-testid="operations-status-filter">
                  <option value="">All states</option>
                  {Object.entries(STATUS).map(([k, v]) => <option key={k} value={k}>{v.label}</option>)}
                </select>
              </div>
              <div className="flex w-full flex-col gap-1 text-sm sm:w-44">
                <label htmlFor={`${id}-kind`}>Kind of change</label>
                <select id={`${id}-kind`} className="select select-sm w-full" value={filters.kind} onChange={(e) => set("kind", e.target.value)} data-testid="operations-kind-filter">
                  <option value="">All kinds</option>
                  {AREAS.map(([k, label]) => <option key={k} value={k}>{label}</option>)}
                </select>
              </div>
              <div className="flex w-full flex-col gap-1 text-sm sm:w-48">
                <label htmlFor={`${id}-application`}>Application</label>
                <input id={`${id}-application`} type="search" className="input input-sm w-full" value={filters.application} onChange={(e) => set("application", e.target.value)} maxLength={120} placeholder="Exact name" autoComplete="off" spellCheck={false} data-testid="operations-application-filter" />
              </div>
              {filtered && <button type="button" className="btn btn-sm btn-ghost self-end" onClick={() => setFilters(NO_FILTERS)} data-testid="operations-clear">Clear filters</button>}
            </div>
            {items.length === 0 ? (
              <p className="text-sm text-base-content/70" data-testid="operations-empty">{filtered ? "No operations match these filters." : "Nothing has been requested yet."}</p>
            ) : (
              <TableFrame>
                <thead><tr><th>Requested</th><th>Change</th><th className="max-sm:hidden">Target</th><th>Status</th><th className="max-sm:hidden">Attempts</th><th><span className="sr-only">Actions</span></th></tr></thead>
                <tbody>
                  {items.map((j) => {
                    const err = control.error?.id === j.id ? control.error.message : null;
                    const showRetry = j.status === "failed" && !!j.actions?.retry.allowed;
                    const showCancel = isActive(j) && !!j.actions?.cancel.allowed;
                    return (
                      <tr key={j.id} data-testid="job-row">
                        <td className="whitespace-nowrap">{new Date(j.createdAt).toLocaleString()}</td>
                        <td>
                          <a href={jobHref(j.id)} className="link link-hover font-medium" data-testid="job-link">{kindLabel(j.kind)}</a>
                          <span className="mt-1 block font-mono text-xs sm:hidden">{jobTarget(j)}</span>
                          {j.requestId && <span className="mt-1 block"><RequestId id={j.requestId} /></span>}
                        </td>
                        <td className="font-mono text-xs max-sm:hidden">{jobTarget(j)}</td>
                        <td>
                          <JobStatus job={j} />
                          {j.lastError && <span className="mt-1 block text-xs text-error" data-testid="job-error">{j.lastError}</span>}
                          {err && <span role="alert" className="mt-1 block text-xs text-error" data-testid="job-action-error">{err}</span>}
                        </td>
                        <td className="max-sm:hidden">{j.attempts} of {j.maxAttempts}</td>
                        <td className="text-right">
                          <span className="inline-flex flex-wrap justify-end gap-1">
                            {showRetry && <button type="button" className="btn btn-xs btn-ghost" aria-label={`Retry ${kindLabel(j.kind).toLowerCase()} ${jobTarget(j)}`} disabled={control.busy === j.id} onClick={() => void control.retry(j)} data-testid="job-retry">Retry</button>}
                            {showCancel && <button type="button" className="btn btn-xs btn-ghost text-error" aria-label={`Cancel ${kindLabel(j.kind).toLowerCase()} ${jobTarget(j)}`} disabled={control.busy === j.id} onClick={() => control.askCancel(j)} data-testid="job-cancel">Cancel…</button>}
                          </span>
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </TableFrame>
            )}
            {moreError && <ErrorPanel message={moreError} testId="operations-more-error" />}
            {next ? (
              <button type="button" className="btn btn-sm btn-outline self-start" onClick={loadOlder} disabled={loadingMore} data-testid="operations-older">{loadingMore ? "Loading…" : "Older operations"}</button>
            ) : items.length > 0 ? <p className="text-xs text-base-content/60" data-testid="operations-end">No older operations.</p> : null}
            {control.dialog}
          </>
        );
      }}
    </Section>
  );
}

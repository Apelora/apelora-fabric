import { useCallback, useEffect, useId, useRef, useState } from "react";

import { ApiRequestError, type AuditItem, type WhoAmI } from "@apelora/api-client";

import { Badge, ErrorPanel, Heading, TableFrame, type Tone } from "../components/ui";
import { client, messageOf } from "../lib/api";
import { actorLabel, fmtTime, useDebounced, useDirectory, type Names } from "../lib/identity";

const PAGE = 50;
type Outcome = "" | "success" | "denied" | "failed";
const OUTCOME: Record<string, { tone: Tone; label: string }> = {
  success: { tone: "ok", label: "Success" },
  denied: { tone: "error", label: "Denied" },
  failed: { tone: "warn", label: "Failed" },
};
const ACTOR_TYPE: Record<string, string> = { user: "Person", service: "Automation", token: "API token", development: "Development", cli: "Command line", system: "System", anonymous: "Not signed in" };

/** "instance.create.requested" → "Instance create requested". */
const describe = (action: string) => {
  const s = action.replace(/[._-]/g, " ");
  return s.charAt(0).toUpperCase() + s.slice(1);
};

/** The filters, as the API and the address bar name them. Times are RFC 3339 (UTC). */
export type AuditFilters = { actor: string; target: string; action: string; outcome: Outcome; from: string; to: string };
const NO_FILTERS: AuditFilters = { actor: "", target: "", action: "", outcome: "", from: "", to: "" };
const KEYS = Object.keys(NO_FILTERS) as (keyof AuditFilters)[];

/** Filters from the part of the address after `?` (`#/audit?actor=ada&outcome=denied`). */
export function filtersFromHash(hash: string): AuditFilters {
  const q = new URLSearchParams(hash.includes("?") ? hash.slice(hash.indexOf("?") + 1) : "");
  const f = { ...NO_FILTERS };
  for (const k of KEYS) f[k] = (q.get(k) ?? "").slice(0, 200) as never;
  if (!["", "success", "denied", "failed"].includes(f.outcome)) f.outcome = "";
  for (const k of ["from", "to"] as const) if (f[k] && Number.isNaN(Date.parse(f[k]))) f[k] = "";
  return f;
}

export function hashFor(path: string, f: AuditFilters): string {
  const q = new URLSearchParams();
  for (const k of KEYS) if (f[k]) q.set(k, f[k]);
  const s = q.toString();
  return `#/${path}${s ? `?${s}` : ""}`;
}

/** An RFC 3339 time as the value of a `datetime-local` field (the browser's time zone), and back. */
const toLocalInput = (iso: string) => {
  const d = new Date(iso);
  if (!iso || Number.isNaN(d.getTime())) return "";
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}T${p(d.getHours())}:${p(d.getMinutes())}`;
};
const fromLocalInput = (v: string) => (v && !Number.isNaN(Date.parse(v)) ? new Date(v).toISOString() : "");

function scopeText(scope: AuditItem["scope"], names: Names): string {
  if (!scope) return "—";
  const parts: string[] = [];
  const { organization: org, project, application: app } = scope as Record<string, unknown>;
  if (typeof org === "string") parts.push(names.org(org));
  if (typeof project === "string") parts.push(names.project(project));
  if (typeof app === "string") parts.push(`application ${app.slice(0, 8)}`);
  return parts.join(" › ") || "—";
}

function Row({ item, names, testId }: { item: AuditItem; names: Names; testId: string }) {
  const outcome = OUTCOME[item.outcome] ?? { tone: "neutral" as Tone, label: item.outcome ?? "—" };
  const denied = item.outcome === "denied";
  const detail = (item.detail ?? {}) as Record<string, unknown>;
  const reason = typeof detail.reason === "string" ? detail.reason : null;
  const request = typeof detail.requestId === "string" ? detail.requestId : null;
  const who = item.actorName || actorLabel(item.actor, names);
  return (
    <tr data-testid={testId} data-outcome={item.outcome} className={denied ? "bg-error/5" : undefined}>
      <td className={`align-top max-sm:w-24 sm:whitespace-nowrap ${denied ? "border-l-4 border-error" : ""}`}>{fmtTime(item.at)}</td>
      <td className="align-top">
        <span className="font-medium break-all">{who}</span>
        <span className="block text-xs text-base-content/60">{ACTOR_TYPE[item.actorType] ?? item.actorType ?? ""}</span>
      </td>
      <td className="align-top">
        {describe(item.action)}
        <span className="block break-all font-mono text-xs text-base-content/60">{item.action}</span>
        <span className="block break-all font-mono text-xs md:hidden">{item.target}</span>
        {reason && <span className="mt-1 block text-xs" data-testid="audit-reason">Reason: {reason}</span>}
        <details className="mt-1 text-xs">
          <summary className="cursor-pointer text-base-content/60">Details</summary>
          {request && <p className="mt-1 font-mono">Request {request}</p>}
          <pre tabIndex={0} aria-label={`Details of event ${item.id}`} className="mt-1 max-w-[16rem] overflow-x-auto rounded border border-base-300 bg-base-200 p-2 font-mono sm:max-w-md">{JSON.stringify(detail, null, 2)}</pre>
        </details>
      </td>
      <td className="break-all align-top font-mono text-xs max-md:hidden">{item.target}</td>
      <td className="align-top"><Badge tone={outcome.tone} testId="audit-outcome">{outcome.label}</Badge></td>
      <td className="align-top max-lg:hidden">{scopeText(item.scope, names)}</td>
    </tr>
  );
}

type Variant = {
  /** Where the view lives (`audit` or `identity/audit`); filters are kept after it in the address. */
  path: string;
  title: string;
  intro: string;
  /** The list operation: both take the same filters. */
  op: "audit" | "identity_audit";
  /** Test ids: `<id>` (section), `<id>-row`, `<id>-empty`, `<id>-error`, `<id>-loading`. */
  testId: string;
};

const stamp = () => new Date().toISOString().replace(/[-:]/g, "").replace(/\.\d+Z$/, "Z");

/** Hands text to the browser as a file to save. */
function save(text: string, name: string, type: string) {
  const url = URL.createObjectURL(new Blob([text], { type }));
  const a = document.createElement("a");
  a.href = url;
  a.download = name;
  document.body.append(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

/**
 * The audit trail: newest first, filtered by actor, target, action, outcome and time (kept in the
 * address, so a filtered view can be shared or bookmarked), paged, and exported with the same
 * filters as CSV or JSON lines. The server decides what the caller may see and records each export.
 */
export function AuditExplorer({ me, onAuth, variant }: { me: WhoAmI | null; onAuth: (m: string) => void; variant: Variant }) {
  const [filters, setFilters] = useState<AuditFilters>(() => filtersFromHash(window.location.hash));
  const [items, setItems] = useState<AuditItem[]>([]);
  const [status, setStatus] = useState<"loading" | "ok" | "error">("loading");
  const [error, setError] = useState<string | null>(null);
  // The cursor of the next, older page; null on the last one.
  const [next, setNext] = useState<string | null>(null);
  const more = next !== null;
  const [loadingMore, setLoadingMore] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const [exporting, setExporting] = useState(false);
  const [exported, setExported] = useState<string | null>(null);
  const [exportError, setExportError] = useState<string | null>(null);
  // On a phone the filters fold away so the events come first; they start open when set.
  const [showFilters, setShowFilters] = useState(() => KEYS.some((k) => filtersFromHash(window.location.hash)[k] !== ""));
  const menu = useRef<HTMLDetailsElement>(null);
  const id = useId();
  const dir = useDirectory(me, onAuth, { orgs: true, projects: true, users: true, services: true });

  // Typing is debounced; choices apply at once.
  const actor = useDebounced(filters.actor.trim(), 300);
  const target = useDebounced(filters.target.trim(), 300);
  const action = useDebounced(filters.action.trim(), 300);
  const applied: AuditFilters = { actor, target, action, outcome: filters.outcome, from: filters.from, to: filters.to };
  const query = { actor: actor || undefined, target: target || undefined, action: action || undefined, outcome: filters.outcome || undefined, from: filters.from || undefined, to: filters.to || undefined };
  const key = JSON.stringify(query);

  // The address follows the filters (without adding history entries), and the filters follow
  // the address when it changes from outside (back, forward, a pasted link).
  useEffect(() => {
    const next = hashFor(variant.path, applied);
    if (window.location.hash !== next) window.history.replaceState(null, "", next);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, variant.path]);
  useEffect(() => {
    const onHash = () => {
      if (window.location.hash.replace(/^#\/?/, "").replace(/\?.*$/, "") === variant.path) setFilters(filtersFromHash(window.location.hash));
    };
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, [variant.path]);

  const fetchPage = useCallback(
    (after?: string) => client.request(variant.op, { query: { ...JSON.parse(key), after, limit: PAGE } }),
    [key, variant.op],
  );
  const fail = useCallback((e: unknown, set: (m: string) => void) => {
    if (e instanceof ApiRequestError && e.unauthenticated) onAuth(e.message);
    set(messageOf(e));
  }, [onAuth]);

  useEffect(() => {
    let current = true;
    setStatus("loading");
    setError(null);
    fetchPage().then(
      (r) => { if (!current) return; setItems(r.items); setNext(r.next ?? null); setStatus("ok"); },
      (e: unknown) => { if (!current) return; fail(e, setError); setStatus("error"); },
    );
    return () => { current = false; };
  }, [fetchPage, fail, attempt]);

  const older = () => {
    if (!next) return;
    setLoadingMore(true);
    setError(null);
    fetchPage(next).then(
      (r) => { setItems((prev) => [...prev, ...r.items]); setNext(r.next ?? null); },
      (e: unknown) => fail(e, setError),
    ).finally(() => setLoadingMore(false));
  };

  const exportAs = (format: "csv" | "jsonl") => {
    if (menu.current) menu.current.open = false;
    setExporting(true);
    setExported(null);
    setExportError(null);
    client.request("audit_export", { query: { ...JSON.parse(key), format } }).then(
      (text) => {
        const lines = text.split("\n").filter((l) => l.trim() !== "").length - (format === "csv" ? 1 : 0);
        save(text, `apelora-audit-${stamp()}.${format}`, format === "csv" ? "text/csv" : "application/x-ndjson");
        setExported(`Exported ${lines} ${lines === 1 ? "event" : "events"} as ${format === "csv" ? "CSV" : "JSON lines"}. Exports are audited too.`);
      },
      (e: unknown) => fail(e, setExportError),
    ).finally(() => setExporting(false));
  };

  const set = <K extends keyof AuditFilters>(k: K, v: AuditFilters[K]) => setFilters((f) => ({ ...f, [k]: v }));
  const filtered = KEYS.some((k) => filters[k] !== "");
  const active = KEYS.filter((k) => filters[k] !== "").length;
  const field = (k: "actor" | "target" | "action", label: string, placeholder: string) => (
    <div className="flex w-full flex-col gap-1 text-sm sm:w-48">
      <label htmlFor={`${id}-${k}`}>{label}</label>
      <input id={`${id}-${k}`} type="search" className="input input-sm w-full" value={filters[k]} onChange={(e) => set(k, e.target.value)} maxLength={200} placeholder={placeholder} autoComplete="off" spellCheck={false} data-testid={`audit-${k}`} />
    </div>
  );
  const time = (k: "from" | "to", label: string) => (
    <div className="flex w-full flex-col gap-1 text-sm sm:w-52">
      <label htmlFor={`${id}-${k}`}>{label}</label>
      <input id={`${id}-${k}`} type="datetime-local" className="input input-sm w-full" value={toLocalInput(filters[k])} onChange={(e) => set(k, fromLocalInput(e.target.value))} data-testid={`audit-${k}`} />
    </div>
  );

  return (
    <section className="flex flex-col gap-3" aria-label={variant.title} data-testid={variant.testId}>
      <div className="flex flex-wrap items-center justify-between gap-3">
        <Heading level={1}>{variant.title}{status === "ok" && <span className="font-normal text-base-content/60"> ({items.length}{more ? "+" : ""})</span>}</Heading>
        <div className="flex flex-wrap items-center justify-end gap-2">
          <button type="button" className="btn btn-sm btn-ghost" onClick={() => setAttempt((n) => n + 1)}>Refresh</button>
          <details className="dropdown dropdown-end" ref={menu}>
            <summary className={`btn btn-sm btn-outline ${exporting ? "btn-disabled" : ""}`} aria-disabled={exporting} data-testid="audit-export">{exporting ? "Exporting…" : "Export"}</summary>
            <ul className="menu dropdown-content z-10 mt-1 w-56 rounded-box border border-base-300 bg-base-100 p-1 text-sm">
              <li><button type="button" onClick={() => exportAs("csv")} data-testid="audit-export-csv">CSV, for spreadsheets</button></li>
              <li><button type="button" onClick={() => exportAs("jsonl")} data-testid="audit-export-jsonl">JSON lines, for tools</button></li>
            </ul>
          </details>
        </div>
      </div>
      <p className="text-sm text-base-content/70">{variant.intro} Secrets are never recorded.</p>
      <div role="status" className="text-sm empty:hidden" data-testid="audit-export-done">{exported}</div>
      {exportError && <ErrorPanel message={exportError} testId="audit-export-error" />}

      <button type="button" className="btn btn-sm btn-ghost self-start sm:hidden" aria-expanded={showFilters} aria-controls={`${id}-filters`} onClick={() => setShowFilters((v) => !v)} data-testid="audit-filters-toggle">
        {showFilters ? "Hide filters" : "Filters"}{active > 0 && ` (${active} set)`}
      </button>
      <div id={`${id}-filters`} className={`${showFilters ? "flex" : "hidden"} flex-wrap items-end gap-3 rounded-box border border-base-300 p-3 sm:flex`} role="group" aria-label="Filters">
        {field("actor", "Who", "Name or user:…")}
        {field("target", "Exact target", "local-account:ada")}
        {field("action", "Action starts with", "role. or auth.")}
        <div className="flex w-full flex-col gap-1 text-sm sm:w-40">
          <label htmlFor={`${id}-outcome`}>Outcome</label>
          <select id={`${id}-outcome`} className="select select-sm w-full" value={filters.outcome} onChange={(e) => set("outcome", e.target.value as Outcome)} data-testid="audit-outcome-filter">
            <option value="">All outcomes</option>
            <option value="success">Success</option>
            <option value="denied">Denied</option>
            <option value="failed">Failed</option>
          </select>
        </div>
        {time("from", "From")}
        {time("to", "Until")}
        {filtered && <button type="button" className="btn btn-sm btn-ghost self-end" onClick={() => setFilters(NO_FILTERS)} data-testid="audit-clear">Clear filters</button>}
      </div>

      {status === "loading" && <p role="status" className="text-sm text-base-content/60" data-testid={`${variant.testId}-loading`}>Loading…</p>}
      {status === "error" && <ErrorPanel message={error ?? "Could not load the audit log."} testId={`${variant.testId}-error`} onRetry={() => setAttempt((n) => n + 1)} />}
      {status === "ok" && (
        <>
          {error && <ErrorPanel message={error} testId={`${variant.testId}-more-error`} />}
          {items.length === 0 ? (
            <p className="text-sm text-base-content/70" data-testid={`${variant.testId}-empty`}>{filtered ? "No events match these filters." : "Nothing recorded yet."}</p>
          ) : (
            <TableFrame>
              <thead><tr><th>When</th><th>Who</th><th>What</th><th className="max-md:hidden">Target</th><th>Outcome</th><th className="max-lg:hidden">Scope</th></tr></thead>
              <tbody>{items.map((i) => <Row key={i.id} item={i} names={dir.names} testId={`${variant.testId}-row`} />)}</tbody>
            </TableFrame>
          )}
          {more ? (
            <button type="button" className="btn btn-sm btn-outline self-start" onClick={older} disabled={loadingMore} data-testid="audit-older">{loadingMore ? "Loading…" : "Older events"}</button>
          ) : items.length > 0 ? <p className="text-xs text-base-content/60" data-testid="audit-end">No older events.</p> : null}
        </>
      )}
    </section>
  );
}

export function AuditView({ me = null, onAuth }: { me?: WhoAmI | null; onAuth: (m: string) => void }) {
  return (
    <AuditExplorer
      me={me}
      onAuth={onAuth}
      variant={{ path: "audit", title: "Audit log", intro: "Who did what, on what, and the outcome, newest first.", op: "audit", testId: "audit" }}
    />
  );
}

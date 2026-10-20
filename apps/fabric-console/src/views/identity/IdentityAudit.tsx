import { useCallback, useEffect, useId, useState } from "react";

import { ApiRequestError, type AuditItem, type WhoAmI } from "@apelora/api-client";

import { Badge, ErrorPanel, Heading, TableFrame, type Tone } from "../../components/ui";
import { client, messageOf } from "../../lib/api";
import { actorLabel, fmtTime, useDebounced, useDirectory, type Names } from "../../lib/identity";

const PAGE = 50;
type Outcome = "" | "success" | "denied" | "failed";
const OUTCOME: Record<string, { tone: Tone; label: string }> = {
  success: { tone: "ok", label: "Success" },
  denied: { tone: "error", label: "Denied" },
  failed: { tone: "warn", label: "Failed" },
};
const ACTOR_TYPE: Record<string, string> = { user: "Person", service: "Automation", token: "API token", development: "Development", cli: "Command line", system: "System" };

/** "role.assign" → "Role assign". */
const describe = (action: string) => {
  const s = action.replace(/[._-]/g, " ");
  return s.charAt(0).toUpperCase() + s.slice(1);
};

function scopeText(scope: AuditItem["scope"], names: Names): string {
  if (!scope) return "—";
  const parts: string[] = [];
  const org = scope.organization, project = scope.project, app = scope.application;
  if (typeof org === "string") parts.push(names.org(org));
  if (typeof project === "string") parts.push(names.project(project));
  if (typeof app === "string") parts.push(`application ${app.slice(0, 8)}`);
  return parts.join(" › ") || "—";
}

function Row({ item, names }: { item: AuditItem; names: Names }) {
  const outcome = OUTCOME[item.outcome] ?? { tone: "neutral" as Tone, label: item.outcome };
  const denied = item.outcome === "denied";
  const reason = typeof item.detail.reason === "string" ? item.detail.reason : null;
  return (
    <tr data-testid="identity-audit-row" data-outcome={item.outcome} className={denied ? "bg-error/5" : undefined}>
      <td className={`whitespace-nowrap align-top ${denied ? "border-l-4 border-error" : ""}`}>{fmtTime(item.at)}</td>
      <td className="align-top">
        <span className="font-medium">{actorLabel(item.actor, names)}</span>
        <span className="block text-xs text-base-content/60">{ACTOR_TYPE[item.actorType] ?? item.actorType}</span>
      </td>
      <td className="align-top">
        {describe(item.action)}
        <span className="block break-all font-mono text-xs text-base-content/60">{item.action}</span>
        <span className="block break-all font-mono text-xs md:hidden">{item.target}</span>
        {reason && <span className="mt-1 block text-xs" data-testid="audit-reason">Reason: {reason}</span>}
        <details className="mt-1 text-xs">
          <summary className="cursor-pointer text-base-content/60">Details</summary>
          <pre tabIndex={0} aria-label={`Details of event ${item.id}`} className="mt-1 max-w-xs overflow-x-auto rounded border border-base-300 bg-base-200 p-2 font-mono sm:max-w-md">{JSON.stringify(item.detail, null, 2)}</pre>
        </details>
      </td>
      <td className="break-all align-top font-mono text-xs max-md:hidden">{item.target}</td>
      <td className="align-top"><Badge tone={outcome.tone} testId="audit-outcome">{outcome.label}</Badge></td>
      <td className="align-top max-lg:hidden">{scopeText(item.scope, names)}</td>
    </tr>
  );
}

export function IdentityAuditView({ me, onAuth }: { me: WhoAmI | null; onAuth: (m: string) => void }) {
  const [action, setAction] = useState("");
  const [outcome, setOutcome] = useState<Outcome>("");
  const [items, setItems] = useState<AuditItem[]>([]);
  const [status, setStatus] = useState<"loading" | "ok" | "error">("loading");
  const [error, setError] = useState<string | null>(null);
  const [more, setMore] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const actionId = useId();
  const outcomeId = useId();
  const prefix = useDebounced(action.trim(), 300);
  const dir = useDirectory(me, onAuth, { orgs: true, projects: true, users: true, services: true });

  const fetchPage = useCallback((before?: number) => client.request("identity_audit", { query: { action: prefix || undefined, outcome: outcome || undefined, before, limit: PAGE } }), [prefix, outcome]);
  const fail = useCallback((e: unknown) => {
    if (e instanceof ApiRequestError && e.unauthenticated) onAuth(e.message);
    setError(messageOf(e));
  }, [onAuth]);

  useEffect(() => {
    let current = true;
    setStatus("loading");
    fetchPage().then(
      (r) => { if (!current) return; setItems(r.items); setMore(r.items.length === PAGE); setStatus("ok"); },
      (e: unknown) => { if (!current) return; fail(e); setStatus("error"); },
    );
    return () => { current = false; };
  }, [fetchPage, fail, attempt]);

  const older = () => {
    const last = items[items.length - 1];
    if (!last) return;
    setLoadingMore(true);
    setError(null);
    fetchPage(last.id).then(
      (r) => { setItems((prev) => [...prev, ...r.items]); setMore(r.items.length === PAGE); },
      (e: unknown) => fail(e),
    ).finally(() => setLoadingMore(false));
  };
  const filtered = !!prefix || !!outcome;

  return (
    <section className="flex flex-col gap-3" aria-label="Audit history" data-testid="identity-audit">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <Heading level={1}>Audit history{status === "ok" && <span className="font-normal text-base-content/60"> ({items.length}{more ? "+" : ""})</span>}</Heading>
        <button type="button" className="btn btn-sm btn-ghost" onClick={() => setAttempt((n) => n + 1)}>Refresh</button>
      </div>
      <p className="text-sm text-base-content/70">Every change to people, roles, groups, policies and approvals, and every refusal, newest first. You see the events of the scopes you may audit. Secrets are never recorded.</p>

      <div className="flex flex-wrap items-end gap-3 rounded-box border border-base-300 p-3" role="group" aria-label="Filters">
        <div className="flex flex-col gap-1 text-sm">
          <label htmlFor={actionId}>Action starts with</label>
          <input id={actionId} type="search" className="input input-sm w-52" value={action} onChange={(e) => setAction(e.target.value)} maxLength={64} placeholder="role. or emergency_access." autoComplete="off" spellCheck={false} data-testid="audit-action" />
        </div>
        <div className="flex flex-col gap-1 text-sm">
          <label htmlFor={outcomeId}>Outcome</label>
          <select id={outcomeId} className="select select-sm w-40" value={outcome} onChange={(e) => setOutcome(e.target.value as Outcome)} data-testid="audit-outcome-filter">
            <option value="">All outcomes</option>
            <option value="success">Success</option>
            <option value="denied">Denied</option>
            <option value="failed">Failed</option>
          </select>
        </div>
        {filtered && <button type="button" className="btn btn-sm btn-ghost" onClick={() => { setAction(""); setOutcome(""); }}>Clear filters</button>}
      </div>

      {status === "loading" && <p role="status" className="text-sm text-base-content/60" data-testid="identity-audit-loading">Loading…</p>}
      {status === "error" && <ErrorPanel message={error ?? "Could not load the audit history."} testId="identity-audit-error" onRetry={() => setAttempt((n) => n + 1)} />}
      {status === "ok" && (
        <>
          {error && <ErrorPanel message={error} testId="identity-audit-more-error" />}
          {items.length === 0 ? (
            <p className="text-sm text-base-content/70" data-testid="identity-audit-empty">{filtered ? "No events match these filters." : "Nothing recorded yet."}</p>
          ) : (
            <TableFrame>
              <thead><tr><th>When</th><th>Who</th><th>What</th><th className="max-md:hidden">Target</th><th>Outcome</th><th className="max-lg:hidden">Scope</th></tr></thead>
              <tbody>{items.map((i) => <Row key={i.id} item={i} names={dir.names} />)}</tbody>
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

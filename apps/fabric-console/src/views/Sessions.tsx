// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useEffect, useState } from "react";

import { ApiRequestError, type AuditItem, type SessionInfo, type SessionList, type WhoAmI } from "@apelora/api-client";
import { KIND_LABEL, remaining, shortDuration } from "@apelora/terminal";

import { Badge, ConfirmDialog, ErrorPanel, Heading, Notice, Section, SectionHeader, TableFrame, type Tone } from "../components/ui";
import { client, useLoad } from "../lib/api";
import { actorLabel, describeError, fmtTime, holds, useDirectory, useRun, type Names } from "../lib/identity";

const POLL_MS = 10_000;
const PAGE = 50;

/** Reloads `reload` every `ms` while the page is visible. */
function usePolled(reload: () => void, ms: number) {
  useEffect(() => {
    const id = setInterval(() => { if (document.visibilityState === "visible") reload(); }, ms);
    return () => clearInterval(id);
  }, [reload, ms]);
}

/** The clock the "time left" column counts against; not announced (it is not in a live region). */
function useNow(ms: number) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), ms);
    return () => clearInterval(id);
  }, [ms]);
  return now;
}

function ActiveSessions({ onAuth }: { onAuth: (m: string) => void }) {
  const [list, reload] = useLoad<SessionList>(() => client.get("sessions"), onAuth);
  usePolled(reload, POLL_MS);
  const now = useNow(5_000);
  const [ending, setEnding] = useState<SessionInfo | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const { busy, error, run } = useRun(onAuth);

  const end = (s: SessionInfo) => {
    setEnding(null);
    setNotice(null);
    void run(async () => {
      await client.request("terminate_session", { params: { id: s.id } });
      setNotice(`${s.mine ? "Your" : `${s.person}'s`} session on ${s.instance} was ended.`);
      reload();
    });
  };

  return (
    <Section title="Open now" state={list} testId="sessions-active" onRetry={reload}>
      {(l) => (
        <>
          <SectionHeader title="Open now" count={l.items.length}>
            <button type="button" className="btn btn-sm btn-ghost" onClick={reload}>Refresh</button>
          </SectionHeader>
          {notice && <Notice tone="ok" testId="sessions-notice">{notice}</Notice>}
          {error && <ErrorPanel message={error} testId="sessions-error" />}
          {l.items.length === 0 ? (
            <p className="text-sm text-base-content/70" data-testid="sessions-empty">No sessions are open. Open one with Terminal… on Instances or Applications.</p>
          ) : (
            <TableFrame>
              <thead><tr><th>Person</th><th>Instance</th><th className="max-md:hidden">Started</th><th className="max-sm:hidden">Last typed</th><th>Time left</th><th><span className="sr-only">Actions</span></th></tr></thead>
              <tbody>
                {l.items.map((s) => {
                  const left = remaining(s, now);
                  return (
                    <tr key={s.id} data-testid="session-row">
                      <td className="align-top font-medium">{s.mine ? "You" : s.person}</td>
                      <td className="align-top break-words">
                        <span className="font-mono text-xs">{s.instance}</span>
                        <span className="block text-xs text-base-content/60">{KIND_LABEL[s.kind]} · {s.application}</span>
                      </td>
                      <td className="align-top whitespace-nowrap max-md:hidden">{fmtTime(s.startedAt)}</td>
                      <td className="align-top whitespace-nowrap max-sm:hidden">{fmtTime(s.lastActivityAt)}</td>
                      <td className="align-top" data-testid="session-left">
                        {shortDuration(left.secs)}
                        <span className="block text-xs text-base-content/60">{left.by === "idle" ? "unless they type" : "time limit"}</span>
                      </td>
                      <td className="align-top text-right">
                        <button type="button" className="btn btn-xs btn-ghost text-error" disabled={busy} aria-label={s.mine ? `End session (yours) on ${s.instance}` : `End session of ${s.person} on ${s.instance}`} onClick={() => setEnding(s)} data-testid="session-terminate">End session</button>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </TableFrame>
          )}
          <ConfirmDialog
            open={!!ending}
            title={ending ? `End ${ending.mine ? "your" : `${ending.person}'s`} session on ${ending.instance}?` : ""}
            body={<p>The terminal closes at once and a command still running in it stops. {ending?.mine ? "You are" : `${ending?.person} is`} told why.</p>}
            action="End session"
            busy={busy}
            onCancel={() => setEnding(null)}
            onConfirm={() => { if (ending) end(ending); }}
          />
        </>
      )}
    </Section>
  );
}

const WHAT: Record<string, string> = {
  "session.request": "Asked for a session",
  "session.open": "Opened a session",
  "session.close": "Session ended",
  "session.denied": "Session refused",
  "session.terminate": "Ended a session",
};
const OUTCOME: Record<string, { tone: Tone; label: string }> = {
  success: { tone: "ok", label: "Success" },
  denied: { tone: "error", label: "Denied" },
  failure: { tone: "warn", label: "Failed" },
  failed: { tone: "warn", label: "Failed" },
};

/** The facts an event carries, in words: why it ended or was refused, how long, under which approval. */
function facts(item: AuditItem): string[] {
  const d = item.detail as Record<string, unknown>;
  const out: string[] = [];
  if (typeof d.kind === "string" && d.kind !== "terminal") out.push(d.kind === "powershell" ? "PowerShell" : d.kind);
  if (typeof d.reason === "string") out.push(`reason: ${d.reason}`);
  if (typeof d.seconds === "number") out.push(`lasted ${shortDuration(d.seconds)}`);
  if (typeof d.approval === "string") out.push(`approval ${d.approval.slice(0, 8)}`);
  if (typeof d.source === "string") out.push(`from ${d.source}`);
  return out;
}

function HistoryRow({ item, names }: { item: AuditItem; names: Names }) {
  const outcome = OUTCOME[item.outcome] ?? { tone: "neutral" as Tone, label: item.outcome };
  const f = facts(item);
  return (
    <tr data-testid="session-history-row" data-action={item.action}>
      <td className="whitespace-nowrap align-top max-sm:hidden">{fmtTime(item.at)}</td>
      <td className="align-top">{actorLabel(item.actor, names)}</td>
      <td className="align-top">
        {WHAT[item.action] ?? item.action}
        <span className="block text-xs text-base-content/70 sm:hidden">{fmtTime(item.at)}</span>
        {f.length > 0 && <span className="block text-xs text-base-content/60">{f.join(" · ")}</span>}
        <span className="block break-words font-mono text-xs sm:hidden">{item.target.replace(/^instance:/, "")}</span>
      </td>
      <td className="break-all align-top font-mono text-xs max-sm:hidden">{item.target.replace(/^instance:/, "")}</td>
      <td className="align-top"><Badge tone={outcome.tone}>{outcome.label}</Badge></td>
    </tr>
  );
}

/** Past sessions, from the audit trail (read-only). Who, which instance, when, why it ended; never what was typed. */
function SessionHistory({ me, onAuth }: { me: WhoAmI; onAuth: (m: string) => void }) {
  const [items, setItems] = useState<AuditItem[]>([]);
  const [status, setStatus] = useState<"loading" | "ok" | "error">("loading");
  const [error, setError] = useState<string | null>(null);
  const [more, setMore] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const dir = useDirectory(me, onAuth, { users: true, services: true });
  const fetchPage = useCallback((before?: number) => client.request("identity_audit", { query: { action: "session.", before, limit: PAGE } }), []);
  const fail = useCallback((e: unknown) => {
    if (e instanceof ApiRequestError && e.unauthenticated) onAuth(e.message);
    setError(describeError(e));
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

  return (
    <section className="flex flex-col gap-2" aria-label="History" data-testid="sessions-history">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <Heading>History from the audit trail</Heading>
        <button type="button" className="btn btn-sm btn-ghost" onClick={() => setAttempt((n) => n + 1)}>Refresh</button>
      </div>
      {status === "loading" && <p role="status" className="text-sm text-base-content/60">Loading…</p>}
      {status === "error" && <ErrorPanel message={error ?? "Could not load the history."} testId="sessions-history-error" onRetry={() => setAttempt((n) => n + 1)} />}
      {status === "ok" && (
        <>
          {error && <ErrorPanel message={error} />}
          {items.length === 0 ? <p className="text-sm text-base-content/70" data-testid="sessions-history-empty">No sessions recorded yet.</p> : (
            <TableFrame>
              <thead><tr><th className="max-sm:hidden">When</th><th>Who</th><th>What</th><th className="max-sm:hidden">Instance</th><th>Outcome</th></tr></thead>
              <tbody>{items.map((i) => <HistoryRow key={i.id} item={i} names={dir.names} />)}</tbody>
            </TableFrame>
          )}
          {more && <button type="button" className="btn btn-sm btn-outline self-start" onClick={older} disabled={loadingMore} data-testid="sessions-history-older">{loadingMore ? "Loading…" : "Older events"}</button>}
        </>
      )}
    </section>
  );
}

/**
 * Terminal and PowerShell sessions into instances: those open now (yours, and those on
 * applications you operate), with "End session", and their history from the audit trail.
 */
export function SessionsView({ me, onAuth }: { me: WhoAmI; onAuth: (m: string) => void }) {
  return (
    <div className="flex flex-col gap-6">
      <div className="flex flex-col gap-2">
        <Heading level={1}>Sessions</Heading>
        <p className="text-sm text-base-content/70">Terminals open now: yours, and those on applications you operate. What people type or see is never recorded.</p>
      </div>
      <ActiveSessions onAuth={onAuth} />
      {holds(me, "audit.read")
        ? <SessionHistory me={me} onAuth={onAuth} />
        : <p className="text-sm text-base-content/70" data-testid="sessions-history-off">Past sessions are in the audit trail, which needs the audit.read permission.</p>}
    </div>
  );
}

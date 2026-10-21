// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useEffect, useRef, useState, type KeyboardEvent } from "react";

import type {
  AlertChannel, AlertChannelList, AlertHistory, AlertItem, AlertList, AlertRuleItem, AlertRuleList, AlertSeverity, AlertSilence,
} from "@apelora/api-client";

import { Badge, ConfirmDialog, ErrorPanel, FormDialog, Heading, Notice, Section, SectionHeader, TableFrame, sentence, type Tone } from "../components/ui";
import { client, useLoad } from "../lib/api";
import { fmtTime, useRun } from "../lib/identity";

const POLL_MS = 20_000;
const PAGE = 50;

export const SEVERITY: Record<AlertSeverity, { tone: Tone; label: string }> = {
  critical: { tone: "error", label: "Critical" },
  warning: { tone: "warn", label: "Warning" },
  info: { tone: "info", label: "Info" },
};

/** Seconds in words: "at once", "45 s", "5 min", "2 h", "1 day". */
export function duration(secs: number): string {
  if (secs <= 0) return "at once";
  if (secs < 60) return `${secs} s`;
  if (secs < 3600) return `${Math.round(secs / 60)} min`;
  if (secs < 86_400) return `${Math.round(secs / 360) / 10} h`;
  const d = Math.round(secs / 8640) / 10;
  return `${d} ${d === 1 ? "day" : "days"}`;
}

/**
 * Focuses `ref` once its dialog is visible. The modal is `visibility: hidden` for a moment while it
 * opens, so `autoFocus` alone misses it: try again on the next frames.
 */
function useFocusWhenShown(ref: { current: HTMLElement | null }) {
  useEffect(() => {
    let frames = 30;
    let id = 0;
    const focus = () => {
      const el = ref.current;
      el?.focus();
      if (el && document.activeElement !== el && frames-- > 0) id = requestAnimationFrame(focus);
    };
    focus();
    return () => cancelAnimationFrame(id);
  }, [ref]);
}

/** Reloads `reload` every `ms` while the page is visible. */
function usePolled(reload: () => void, ms: number) {
  useEffect(() => {
    const id = setInterval(() => { if (document.visibilityState === "visible") reload(); }, ms);
    return () => clearInterval(id);
  }, [reload, ms]);
}

const who = (by: string, name?: string | null) => name || by;

function SeverityBadge({ severity }: { severity: AlertSeverity }) {
  const s = SEVERITY[severity];
  return <Badge tone={s.tone}>{s.label}</Badge>;
}

function What({ a }: { a: AlertItem }) {
  return (
    <>
      <span className="mb-1 block sm:hidden"><SeverityBadge severity={a.severity} /></span>
      <span className="font-medium">{a.title}</span>
      <span className="block break-words font-mono text-xs text-base-content/70">{a.subject}</span>
      <span className="block break-words text-xs text-base-content/70">{sentence(a.summary)}</span>
    </>
  );
}

// ---- acknowledge and silence ------------------------------------------------------------------

function AcknowledgeDialog({ alert, onClose, onDone, onAuth }: { alert: AlertItem; onClose: () => void; onDone: (msg: string) => void; onAuth: (m: string) => void }) {
  const [note, setNote] = useState("");
  const first = useRef<HTMLTextAreaElement>(null);
  useFocusWhenShown(first);
  const { busy, error, run } = useRun(onAuth);
  const submit = () => void run(async () => {
    await client.request("acknowledge_alert", { params: { id: String(alert.id) }, body: { note: note.trim() || null } });
    onDone(`You acknowledged “${alert.title}” on ${alert.subject}. It stays open until its condition clears.`);
  });
  return (
    <FormDialog open title={`Acknowledge “${alert.title}”`} onClose={onClose} testId="ack-dialog">
      <form className="flex flex-col gap-3" onSubmit={(e) => { e.preventDefault(); submit(); }}>
        <p className="text-sm text-base-content/80">Tells others you are on it. Notifications continue; silence it to pause them.</p>
        <label className="flex flex-col gap-1 text-sm">
          <span>Note (optional)</span>
          <textarea className="textarea textarea-sm w-full" maxLength={500} rows={3} value={note} onChange={(e) => setNote(e.target.value)} ref={first} data-testid="ack-note" />
        </label>
        {error && <ErrorPanel message={error} testId="ack-error" />}
        <div className="modal-action mt-0">
          <button type="button" className="btn btn-sm btn-ghost" onClick={onClose}>Cancel</button>
          <button type="submit" className="btn btn-sm btn-primary" disabled={busy} data-testid="ack-submit">Acknowledge</button>
        </div>
      </form>
    </FormDialog>
  );
}

const SILENCE_FOR = [
  { minutes: 30, label: "30 minutes" },
  { minutes: 60, label: "1 hour" },
  { minutes: 240, label: "4 hours" },
  { minutes: 1440, label: "1 day" },
  { minutes: 10080, label: "1 week" },
];

function SilenceDialog({ alert, onClose, onDone, onAuth }: { alert: AlertItem; onClose: () => void; onDone: (msg: string) => void; onAuth: (m: string) => void }) {
  const [scope, setScope] = useState<"subject" | "rule">("subject");
  const [minutes, setMinutes] = useState(60);
  const [note, setNote] = useState("");
  const first = useRef<HTMLSelectElement>(null);
  useFocusWhenShown(first);
  const { busy, error, run } = useRun(onAuth);
  const label = SILENCE_FOR.find((s) => s.minutes === minutes)?.label ?? `${minutes} minutes`;
  const submit = () => void run(async () => {
    await client.request("create_alert_silence", { body: { rule: alert.rule, subject: scope === "subject" ? alert.subject : null, minutes, note: note.trim() || null } });
    onDone(`Notifications for “${alert.title}”${scope === "subject" ? ` on ${alert.subject}` : ""} are paused for ${label}.`);
  });
  return (
    <FormDialog open title={`Silence “${alert.title}”`} onClose={onClose} testId="silence-dialog">
      <form className="flex flex-col gap-3" onSubmit={(e) => { e.preventDefault(); submit(); }}>
        <p className="text-sm text-base-content/80">Pauses notifications. The alert still shows here.</p>
        <fieldset className="flex flex-col gap-1 text-sm">
          <legend className="mb-1">Silence</legend>
          <label className="flex items-center gap-2"><input type="radio" className="radio radio-sm" name="silence-scope" checked={scope === "subject"} onChange={() => setScope("subject")} /> Only <span className="font-mono text-xs">{alert.subject}</span></label>
          <label className="flex items-center gap-2"><input type="radio" className="radio radio-sm" name="silence-scope" checked={scope === "rule"} onChange={() => setScope("rule")} data-testid="silence-whole-rule" /> Every alert of this rule</label>
        </fieldset>
        <label className="flex flex-col gap-1 text-sm">
          <span>For</span>
          <select className="select select-sm w-full" value={minutes} onChange={(e) => setMinutes(Number(e.target.value))} ref={first} data-testid="silence-minutes">
            {SILENCE_FOR.map((s) => <option key={s.minutes} value={s.minutes}>{s.label}</option>)}
          </select>
        </label>
        <label className="flex flex-col gap-1 text-sm">
          <span>Reason (optional)</span>
          <input className="input input-sm w-full" maxLength={500} value={note} onChange={(e) => setNote(e.target.value)} data-testid="silence-note" />
        </label>
        {error && <ErrorPanel message={error} testId="silence-error" />}
        <div className="modal-action mt-0">
          <button type="button" className="btn btn-sm btn-ghost" onClick={onClose}>Cancel</button>
          <button type="submit" className="btn btn-sm btn-primary" disabled={busy} data-testid="silence-submit">Silence for {label}</button>
        </div>
      </form>
    </FormDialog>
  );
}

// ---- the alerts tab ---------------------------------------------------------------------------

function AlertRows({ alerts, canRespond, reason, onAck, onSilence, testId }: { alerts: AlertItem[]; canRespond: boolean; reason?: string | null; onAck?: (a: AlertItem) => void; onSilence: (a: AlertItem) => void; testId: string }) {
  return (
    <TableFrame>
      <thead><tr><th className="max-sm:hidden">Severity</th><th>What</th><th className="max-sm:hidden">Since</th><th><span className="sr-only">Actions</span></th></tr></thead>
      <tbody>
        {alerts.map((a) => (
          <tr key={a.id} data-testid={testId} data-rule={a.rule}>
            <td className="align-top max-sm:hidden"><SeverityBadge severity={a.severity} /></td>
            <td className="align-top">
              <What a={a} />
              <span className="block text-xs text-base-content/70 sm:hidden">since {fmtTime(a.firedAt ?? a.startedAt)}</span>
              {a.acknowledged && <span className="block text-xs text-base-content/70" data-testid="alert-acked">Acknowledged by {who(a.acknowledged.by, a.acknowledged.byName)} · {fmtTime(a.acknowledged.at)}{a.acknowledged.note ? ` · “${a.acknowledged.note}”` : ""}</span>}
              {a.silencedBy && <span className="block text-xs text-base-content/70" data-testid="alert-silenced">Silenced until {fmtTime(a.silencedBy.until)}</span>}
              {a.clearingSince && <span className="block text-xs text-base-content/70">Clear since {fmtTime(a.clearingSince)}; resolves if it stays clear</span>}
            </td>
            <td className="whitespace-nowrap align-top max-sm:hidden">{fmtTime(a.firedAt ?? a.startedAt)}</td>
            <td className="align-top">
              {canRespond ? (
                <div className="flex flex-wrap justify-end gap-1">
                  {onAck && <button type="button" className="btn btn-xs btn-outline whitespace-nowrap" onClick={() => onAck(a)} aria-label={`Acknowledge ${a.title} on ${a.subject}`} data-testid="alert-ack">Acknowledge</button>}
                  {!a.silencedBy && <button type="button" className="btn btn-xs btn-ghost whitespace-nowrap" onClick={() => onSilence(a)} aria-label={`Silence ${a.title} on ${a.subject}`} data-testid="alert-silence">Silence…</button>}
                </div>
              ) : <span className="text-xs text-base-content/60" title={reason ?? undefined}>View only</span>}
            </td>
          </tr>
        ))}
      </tbody>
    </TableFrame>
  );
}

function Silences({ silences, canRespond, onAuth, onChanged }: { silences: AlertSilence[]; canRespond: boolean; onAuth: (m: string) => void; onChanged: (msg: string) => void }) {
  const [ending, setEnding] = useState<AlertSilence | null>(null);
  const { busy, error, run } = useRun(onAuth);
  const end = (s: AlertSilence) => { setEnding(null); void run(async () => { await client.request("end_alert_silence", { params: { id: s.id } }); onChanged(`The silence on “${s.ruleTitle}” ended. Notifications are sent again.`); }); };
  if (silences.length === 0) return null;
  return (
    <section className="flex flex-col gap-2" aria-label="Silences" data-testid="alert-silences">
      <Heading>Silences in force <span className="font-normal text-base-content/60">({silences.length})</span></Heading>
      {error && <ErrorPanel message={error} />}
      <TableFrame>
        <thead><tr><th>Rule</th><th className="max-sm:hidden">By</th><th>Until</th><th><span className="sr-only">Actions</span></th></tr></thead>
        <tbody>
          {silences.map((s) => (
            <tr key={s.id} data-testid="silence-row">
              <td className="align-top">
                {s.ruleTitle}
                <span className="block font-mono text-xs text-base-content/70">{s.subject ?? "every subject"}</span>
                {s.note && <span className="block text-xs text-base-content/70">“{s.note}”</span>}
              </td>
              <td className="align-top max-sm:hidden">{who(s.createdBy, s.createdByName)}</td>
              <td className="whitespace-nowrap align-top">{fmtTime(s.endsAt)}</td>
              <td className="align-top text-right">{canRespond && <button type="button" className="btn btn-xs btn-ghost whitespace-nowrap" disabled={busy} onClick={() => setEnding(s)} data-testid="silence-end">End silence</button>}</td>
            </tr>
          ))}
        </tbody>
      </TableFrame>
      <ConfirmDialog open={!!ending} danger={false} title={`End the silence on “${ending?.ruleTitle ?? ""}”?`} body={<p>Notifications for these alerts are sent again from the next change.</p>} action="End silence" busy={busy} onCancel={() => setEnding(null)} onConfirm={() => { if (ending) end(ending); }} />
    </section>
  );
}

function History({ onAuth }: { onAuth: (m: string) => void }) {
  const [items, setItems] = useState<AlertItem[]>([]);
  const [status, setStatus] = useState<"loading" | "ok" | "error">("loading");
  const [error, setError] = useState<string | null>(null);
  const [more, setMore] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const { run } = useRun(onAuth);
  const page = useCallback((before?: number) => client.request("alert_history", { query: { before, limit: PAGE } }), []);
  useEffect(() => {
    let current = true;
    setStatus("loading");
    page().then(
      (r: AlertHistory) => { if (!current) return; setItems(r.items); setMore(r.items.length === PAGE); setStatus("ok"); },
      (e: unknown) => { if (!current) return; setError(e instanceof Error ? e.message : String(e)); setStatus("error"); },
    );
    return () => { current = false; };
  }, [page, attempt]);
  const older = () => {
    const last = items[items.length - 1];
    if (!last) return;
    void run(async () => { const r = await page(last.id); setItems((p) => [...p, ...r.items]); setMore(r.items.length === PAGE); });
  };
  return (
    <section className="flex flex-col gap-2" aria-label="History" data-testid="alert-history">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <Heading>Resolved in the last 90 days</Heading>
        <button type="button" className="btn btn-sm btn-ghost" onClick={() => setAttempt((n) => n + 1)}>Refresh</button>
      </div>
      {status === "loading" && <p role="status" className="text-sm text-base-content/60">Loading…</p>}
      {status === "error" && <ErrorPanel message={error ?? "Could not load the history."} onRetry={() => setAttempt((n) => n + 1)} />}
      {status === "ok" && (items.length === 0 ? <p className="text-sm text-base-content/70" data-testid="alert-history-empty">No alert has resolved yet.</p> : (
        <TableFrame>
          <thead><tr><th className="max-sm:hidden">Severity</th><th>What</th><th className="max-sm:hidden">Fired</th><th className="max-sm:hidden">Resolved</th></tr></thead>
          <tbody>
            {items.map((a) => (
              <tr key={a.id} data-testid="alert-history-row">
                <td className="align-top max-sm:hidden"><SeverityBadge severity={a.severity} /></td>
                <td className="align-top">
                  <What a={a} />
                  <span className="block text-xs text-base-content/70 sm:hidden">resolved {fmtTime(a.resolvedAt)}</span>
                  {a.acknowledged && <span className="block text-xs text-base-content/70">Acknowledged by {who(a.acknowledged.by, a.acknowledged.byName)}</span>}
                </td>
                <td className="whitespace-nowrap align-top max-sm:hidden">{fmtTime(a.firedAt)}</td>
                <td className="whitespace-nowrap align-top max-sm:hidden">{fmtTime(a.resolvedAt)}</td>
              </tr>
            ))}
          </tbody>
        </TableFrame>
      ))}
      {status === "ok" && more && <button type="button" className="btn btn-sm btn-outline self-start" onClick={older} data-testid="alert-history-older">Older alerts</button>}
    </section>
  );
}

function AlertsPanel({ onAuth }: { onAuth: (m: string) => void }) {
  const [list, reload] = useLoad<AlertList>(() => client.get("alerts"), onAuth);
  usePolled(reload, POLL_MS);
  const [acking, setAcking] = useState<AlertItem | null>(null);
  const [silencing, setSilencing] = useState<AlertItem | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const done = (msg: string) => { setAcking(null); setSilencing(null); setNotice(msg); reload(); };
  return (
    <div className="flex flex-col gap-6">
      <Section title="Needs attention" state={list} testId="alerts-open" onRetry={reload}>
        {(l) => {
          const open = l.alerts.filter((a) => !a.acknowledged && !a.silencedBy);
          const handled = l.alerts.filter((a) => a.acknowledged || a.silencedBy);
          const canRespond = l.actions.respond.allowed;
          return (
            <div className="flex flex-col gap-6">
              <div className="flex flex-col gap-2">
                <SectionHeader title="Needs attention" count={open.length}>
                  <button type="button" className="btn btn-sm btn-ghost" onClick={reload}>Refresh</button>
                </SectionHeader>
                {l.evaluator.stale
                  ? <Notice tone="warn" testId="alerts-stale" title="Alerts may be out of date">{l.evaluator.message}</Notice>
                  : <p className="text-xs text-base-content/60" data-testid="alerts-evaluated">Last checked {fmtTime(l.evaluator.lastPassAt)} · every {duration(l.evaluator.everySecs ?? 30)}</p>}
                {notice && <Notice tone="ok" testId="alerts-notice">{notice}</Notice>}
                {open.length === 0
                  ? <p className="text-sm text-base-content/70" data-testid="alerts-none">Nothing needs attention.{handled.length > 0 ? " Acknowledged and silenced alerts are below." : ""}</p>
                  : <AlertRows alerts={open} canRespond={canRespond} reason={l.actions.respond.reason} onAck={setAcking} onSilence={setSilencing} testId="alert-row" />}
              </div>
              {handled.length > 0 && (
                <section className="flex flex-col gap-2" aria-label="Acknowledged or silenced" data-testid="alerts-handled">
                  <Heading>Acknowledged or silenced <span className="font-normal text-base-content/60">({handled.length})</span></Heading>
                  <AlertRows alerts={handled} canRespond={canRespond} reason={l.actions.respond.reason} onSilence={setSilencing} testId="alert-handled-row" />
                </section>
              )}
              <Silences silences={l.silences} canRespond={canRespond} onAuth={onAuth} onChanged={done} />
            </div>
          );
        }}
      </Section>
      <History onAuth={onAuth} />
      {acking && <AcknowledgeDialog alert={acking} onClose={() => setAcking(null)} onDone={done} onAuth={onAuth} />}
      {silencing && <SilenceDialog alert={silencing} onClose={() => setSilencing(null)} onDone={done} onAuth={onAuth} />}
    </div>
  );
}

// ---- rules ------------------------------------------------------------------------------------

const SOURCE: Record<string, { tone: Tone; label: string }> = {
  ok: { tone: "ok", label: "Watching" },
  "not-configured": { tone: "neutral", label: "Not set up" },
  unavailable: { tone: "warn", label: "Source unavailable" },
  disabled: { tone: "neutral", label: "Off" },
  unknown: { tone: "neutral", label: "Not checked yet" },
};

const sameParams = (r: AlertRuleItem) => JSON.stringify(r.params) === JSON.stringify(r.defaults);
const condition = (r: AlertRuleItem, threshold: number) => (r.thresholdEditable ? `${r.comparison === "at-most" ? "≤" : "≥"} ${threshold} ${r.unit}` : "While it lasts");

function RuleDialog({ rule, onClose, onDone, onAuth }: { rule: AlertRuleItem; onClose: () => void; onDone: (msg: string) => void; onAuth: (m: string) => void }) {
  const [enabled, setEnabled] = useState(rule.params.enabled);
  const [threshold, setThreshold] = useState(String(rule.params.threshold));
  const [forSecs, setForSecs] = useState(String(rule.params.forSecs));
  const [severity, setSeverity] = useState<AlertSeverity>(rule.params.severity);
  const first = useRef<HTMLInputElement>(null);
  useFocusWhenShown(first);
  const { busy, error, run } = useRun(onAuth);
  const reset = () => { setEnabled(rule.defaults.enabled); setThreshold(String(rule.defaults.threshold)); setForSecs(String(rule.defaults.forSecs)); setSeverity(rule.defaults.severity); };
  const submit = () => void run(async () => {
    await client.request("update_alert_rule", { params: { rule: rule.id }, body: { enabled, threshold: Number(threshold), forSecs: Number(forSecs), severity } });
    onDone(`“${rule.title}” saved. The next check uses it.`);
  });
  return (
    <FormDialog open title={`Edit “${rule.title}”`} onClose={onClose} testId="rule-dialog">
      <form className="flex flex-col gap-3" onSubmit={(e) => { e.preventDefault(); submit(); }}>
        <p className="text-sm text-base-content/80">{rule.description}</p>
        <label className="flex items-center gap-2 text-sm"><input type="checkbox" className="toggle toggle-sm" checked={enabled} onChange={(e) => setEnabled(e.target.checked)} ref={first} data-testid="rule-enabled" /> Check this rule</label>
        {rule.thresholdEditable && (
          <label className="flex flex-col gap-1 text-sm">
            <span>Fire when the value is {rule.comparison === "at-most" ? "at most" : "at least"} ({rule.unit}; {rule.thresholdMin}–{rule.thresholdMax})</span>
            <input type="number" className="input input-sm w-full" min={rule.thresholdMin} max={rule.thresholdMax} step="any" required value={threshold} onChange={(e) => setThreshold(e.target.value)} data-testid="rule-threshold" />
          </label>
        )}
        <label className="flex flex-col gap-1 text-sm">
          <span>Wait before firing and before resolving (seconds, 0–86400)</span>
          <input type="number" className="input input-sm w-full" min={0} max={86_400} step={1} required value={forSecs} onChange={(e) => setForSecs(e.target.value)} data-testid="rule-for" />
        </label>
        <label className="flex flex-col gap-1 text-sm">
          <span>Severity</span>
          <select className="select select-sm w-full" value={severity} onChange={(e) => setSeverity(e.target.value as AlertSeverity)} data-testid="rule-severity">
            {(["critical", "warning", "info"] as const).map((s) => <option key={s} value={s}>{SEVERITY[s].label}</option>)}
          </select>
        </label>
        {error && <ErrorPanel message={error} testId="rule-error" />}
        <div className="modal-action mt-0 flex-wrap">
          <button type="button" className="btn btn-sm btn-ghost mr-auto" onClick={reset} data-testid="rule-reset">Use defaults</button>
          <button type="button" className="btn btn-sm btn-ghost" onClick={onClose}>Cancel</button>
          <button type="submit" className="btn btn-sm btn-primary" disabled={busy} data-testid="rule-save">Save rule</button>
        </div>
      </form>
    </FormDialog>
  );
}

function RulesPanel({ onAuth }: { onAuth: (m: string) => void }) {
  const [list, reload] = useLoad<AlertRuleList>(() => client.get("alert_rules"), onAuth);
  const [editing, setEditing] = useState<AlertRuleItem | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  return (
    <Section title="Rules" state={list} testId="alert-rules" onRetry={reload}>
      {(l) => (
        <>
          <p className="text-sm text-base-content/70">Built-in checks on Apelora's own data. Muted values are defaults.</p>
          {notice && <Notice tone="ok" testId="rules-notice">{notice}</Notice>}
          <TableFrame>
            <thead><tr><th>Rule</th><th>Fires when</th><th className="max-md:hidden">Waits</th><th className="max-sm:hidden">Severity</th><th>State</th><th><span className="sr-only">Actions</span></th></tr></thead>
            <tbody>
              {l.rules.map((r) => {
                const src = SOURCE[r.sourceState] ?? SOURCE.unknown!;
                const muted = sameParams(r) ? "text-base-content/60" : "";
                return (
                  <tr key={r.id} data-testid="rule-row" data-rule={r.id}>
                    <td className="align-top">
                      <span className="font-medium">{r.title}</span>
                      <span className="block text-xs text-base-content/70">{r.source}{r.firing > 0 ? ` · ${r.firing} firing` : ""}</span>
                    </td>
                    <td className={`align-top ${r.params.threshold !== r.defaults.threshold ? "" : "text-base-content/60"}`} data-testid="rule-condition">{condition(r, r.params.threshold)}</td>
                    <td className={`whitespace-nowrap align-top max-md:hidden ${r.params.forSecs !== r.defaults.forSecs ? "" : "text-base-content/60"}`}>{duration(r.params.forSecs)}</td>
                    <td className="align-top max-sm:hidden"><SeverityBadge severity={r.params.severity} /></td>
                    <td className="align-top"><Badge tone={r.params.enabled ? src.tone : "neutral"} testId="rule-state">{r.params.enabled ? src.label : "Off"}</Badge></td>
                    <td className={`align-top text-right ${muted}`}>
                      {l.actions.configure.allowed && <button type="button" className="btn btn-xs btn-ghost" onClick={() => { setNotice(null); setEditing(r); }} aria-label={`Edit ${r.title}`} data-testid="rule-edit">Edit</button>}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </TableFrame>
          {!l.actions.configure.allowed && <p className="text-xs text-base-content/60" data-testid="rules-readonly">{l.actions.configure.reason}</p>}
          {editing && <RuleDialog rule={editing} onClose={() => setEditing(null)} onDone={(m) => { setEditing(null); setNotice(m); reload(); }} onAuth={onAuth} />}
        </>
      )}
    </Section>
  );
}

// ---- channels ---------------------------------------------------------------------------------

function ChannelDialog({ channel, prefix, onClose, onDone, onDelete, onAuth }: { channel: AlertChannel | null; prefix: string; onClose: () => void; onDone: (msg: string) => void; onDelete: (c: AlertChannel) => void; onAuth: (m: string) => void }) {
  const [name, setName] = useState(channel?.name ?? "");
  const [url, setUrl] = useState(channel?.url ?? "");
  const [secretRef, setSecretRef] = useState(channel?.secretRef ?? prefix);
  const [minSeverity, setMinSeverity] = useState<AlertSeverity>(channel?.minSeverity ?? "warning");
  const [enabled, setEnabled] = useState(channel?.enabled ?? true);
  const first = useRef<HTMLInputElement>(null);
  useFocusWhenShown(first);
  const { busy, error, run } = useRun(onAuth);
  const submit = () => void run(async () => {
    const body = { name: name.trim(), url: url.trim(), secretRef: secretRef.trim(), minSeverity, enabled };
    if (channel) await client.request("update_alert_channel", { params: { id: channel.id }, body });
    else await client.request("create_alert_channel", { body });
    onDone(channel ? `Webhook “${body.name}” saved.` : `Webhook “${body.name}” added. Send a test to check it.`);
  });
  return (
    <FormDialog open title={channel ? `Edit webhook “${channel.name}”` : "Add a webhook"} onClose={onClose} testId="channel-dialog">
      <form className="flex flex-col gap-3" onSubmit={(e) => { e.preventDefault(); submit(); }}>
        <label className="flex flex-col gap-1 text-sm">
          <span>Name</span>
          <input className="input input-sm w-full" maxLength={80} required value={name} onChange={(e) => setName(e.target.value)} ref={first} data-testid="channel-name" />
        </label>
        <label className="flex flex-col gap-1 text-sm">
          <span>URL (https only)</span>
          <input type="url" className="input input-sm w-full font-mono" placeholder="https://" required value={url} onChange={(e) => setUrl(e.target.value)} data-testid="channel-url" />
        </label>
        <label className="flex flex-col gap-1 text-sm">
          <span>Secret reference</span>
          <input className="input input-sm w-full font-mono" required spellCheck={false} autoComplete="off" value={secretRef} onChange={(e) => setSecretRef(e.target.value.toUpperCase())} aria-describedby="secret-ref-help" data-testid="channel-secret" />
          <span id="secret-ref-help" className="text-xs text-base-content/70">The name of an environment variable on every worker that holds the signing secret. Never paste the secret here.</span>
        </label>
        <label className="flex flex-col gap-1 text-sm">
          <span>Send alerts of</span>
          <select className="select select-sm w-full" value={minSeverity} onChange={(e) => setMinSeverity(e.target.value as AlertSeverity)} data-testid="channel-severity">
            <option value="critical">Critical only</option>
            <option value="warning">Warning and critical</option>
            <option value="info">Every severity</option>
          </select>
        </label>
        <label className="flex items-center gap-2 text-sm"><input type="checkbox" className="toggle toggle-sm" checked={enabled} onChange={(e) => setEnabled(e.target.checked)} /> Send notifications</label>
        {error && <ErrorPanel message={error} testId="channel-error" />}
        <div className="modal-action mt-0 flex-wrap">
          {channel && <button type="button" className="btn btn-sm btn-ghost mr-auto text-error" onClick={() => onDelete(channel)} data-testid="channel-delete">Delete…</button>}
          <button type="button" className="btn btn-sm btn-ghost" onClick={onClose}>Cancel</button>
          <button type="submit" className="btn btn-sm btn-primary" disabled={busy} data-testid="channel-save">{channel ? "Save webhook" : "Add webhook"}</button>
        </div>
      </form>
    </FormDialog>
  );
}

function deliveryState(c: AlertChannel): { tone: Tone; label: string; detail?: string } {
  if (!c.enabled) return { tone: "neutral", label: "Off" };
  if (c.delivery.failing > 0) return { tone: "error", label: "Failing", detail: c.delivery.lastError ?? undefined };
  if (c.delivery.pending > 0) return { tone: "info", label: `${c.delivery.pending} waiting` };
  if (c.delivery.lastDeliveredAt) return { tone: "ok", label: "Delivered", detail: `last ${fmtTime(c.delivery.lastDeliveredAt)}` };
  return { tone: "neutral", label: "Nothing sent yet" };
}

function ChannelsPanel({ onAuth }: { onAuth: (m: string) => void }) {
  const [list, reload] = useLoad<AlertChannelList>(() => client.get("alert_channels"), onAuth);
  const [editing, setEditing] = useState<AlertChannel | "new" | null>(null);
  const [deleting, setDeleting] = useState<AlertChannel | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const { busy, error, run } = useRun(onAuth);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => () => { if (timer.current) clearTimeout(timer.current); }, []);
  const test = (c: AlertChannel) => { setNotice(null); void run(async () => {
    const r = await client.request("test_alert_channel", { params: { id: c.id } });
    setNotice(`Test sent to the queue as message ${r.messageId}. Its delivery state appears here within seconds.`);
    reload();
    timer.current = setTimeout(reload, 4000);
  }); };
  const remove = (c: AlertChannel) => { setDeleting(null); void run(async () => { await client.request("delete_alert_channel", { params: { id: c.id } }); setNotice(`Webhook “${c.name}” deleted.`); reload(); }); };
  return (
    <Section title="Channels" state={list} testId="alert-channels" onRetry={reload}>
      {(l) => {
        const can = l.actions.configure.allowed;
        return (
          <>
            <SectionHeader title="Webhooks" count={l.channels.length}>
              {can && <button type="button" className="btn btn-sm btn-primary" onClick={() => { setNotice(null); setEditing("new"); }} data-testid="channel-add">Add webhook</button>}
            </SectionHeader>
            <p className="text-sm text-base-content/70">Each webhook receives alerts when they fire and resolve, signed with your secret.</p>
            {notice && <Notice tone="ok" testId="channels-notice">{notice}</Notice>}
            {error && <ErrorPanel message={error} testId="channels-error" />}
            {l.channels.length === 0 ? (
              <p className="text-sm text-base-content/70" data-testid="channels-empty">{can ? "No webhook yet. Add one to be told about alerts outside the Console." : "No webhook is set up."}</p>
            ) : (
              <TableFrame>
                <thead><tr><th>Webhook</th><th className="max-md:hidden">Secret reference</th><th className="max-sm:hidden">Sends</th><th>Delivery</th><th><span className="sr-only">Actions</span></th></tr></thead>
                <tbody>
                  {l.channels.map((c) => {
                    const d = deliveryState(c);
                    return (
                      <tr key={c.id} data-testid="channel-row">
                        <td className="align-top"><span className="font-medium">{c.name}</span><span className="block break-all font-mono text-xs text-base-content/70">{c.host}</span></td>
                        <td className="break-all align-top font-mono text-xs max-md:hidden">{c.secretRef}</td>
                        <td className="align-top max-sm:hidden">{c.minSeverity === "critical" ? "Critical only" : c.minSeverity === "warning" ? "Warning and up" : "Every severity"}</td>
                        <td className="align-top" data-testid="channel-delivery">
                          <Badge tone={d.tone}>{d.label}</Badge>
                          {d.detail && <span className="block break-words text-xs text-base-content/70">{sentence(d.detail)}</span>}
                        </td>
                        <td className="align-top">
                          {can && (
                            <div className="flex flex-wrap justify-end gap-1">
                              <button type="button" className="btn btn-xs btn-outline" disabled={busy || !c.enabled} onClick={() => test(c)} aria-label={`Send a test to ${c.name}`} data-testid="channel-test">Send test</button>
                              <button type="button" className="btn btn-xs btn-ghost" onClick={() => { setNotice(null); setEditing(c); }} aria-label={`Edit ${c.name}`} data-testid="channel-edit">Edit</button>
                            </div>
                          )}
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </TableFrame>
            )}
            {!can && <p className="text-xs text-base-content/60" data-testid="channels-readonly">{l.actions.configure.reason}</p>}
            {editing && <ChannelDialog channel={editing === "new" ? null : editing} prefix={l.secretPrefix} onClose={() => setEditing(null)} onDelete={(c) => { setEditing(null); setDeleting(c); }} onDone={(m) => { setEditing(null); setNotice(m); reload(); }} onAuth={onAuth} />}
            <ConfirmDialog open={!!deleting} title={`Delete webhook “${deleting?.name ?? ""}”?`} body={<p>It stops receiving alerts at once, including notifications still waiting to be delivered.</p>} action="Delete webhook" busy={busy} onCancel={() => setDeleting(null)} onConfirm={() => { if (deleting) remove(deleting); }} />
          </>
        );
      }}
    </Section>
  );
}

// ---- the view ---------------------------------------------------------------------------------

const TABS = [
  { id: "alerts", label: "Alerts" },
  { id: "rules", label: "Rules" },
  { id: "channels", label: "Channels" },
] as const;
type Tab = (typeof TABS)[number]["id"];

/** Alerts (`#/alerts`): what needs attention, acknowledged and silenced alerts, history; the rules; webhook channels. */
export function AlertsView({ onAuth }: { onAuth: (m: string) => void }) {
  const [tab, setTab] = useState<Tab>("alerts");
  const refs = useRef<Record<string, HTMLButtonElement | null>>({});
  const onKey = (e: KeyboardEvent<HTMLButtonElement>, i: number) => {
    const next = e.key === "ArrowRight" ? (i + 1) % TABS.length : e.key === "ArrowLeft" ? (i - 1 + TABS.length) % TABS.length : e.key === "Home" ? 0 : e.key === "End" ? TABS.length - 1 : -1;
    if (next < 0) return;
    e.preventDefault();
    const t = TABS[next]!;
    setTab(t.id);
    refs.current[t.id]?.focus();
  };
  return (
    <div className="flex flex-col gap-4">
      <h1 className="text-lg font-semibold">Alerts</h1>
      <div role="tablist" aria-label="Alerts" className="tabs tabs-border" data-testid="alert-tabs">
        {TABS.map((t, i) => (
          <button key={t.id} ref={(el) => { refs.current[t.id] = el; }} type="button" role="tab" id={`tab-${t.id}`} aria-selected={tab === t.id} aria-controls={`panel-${t.id}`} tabIndex={tab === t.id ? 0 : -1} className={`tab text-base-content/80 ${tab === t.id ? "tab-active" : ""}`} onClick={() => setTab(t.id)} onKeyDown={(e) => onKey(e, i)} data-testid={`tab-${t.id}`}>{t.label}</button>
        ))}
      </div>
      <div role="tabpanel" id={`panel-${tab}`} aria-labelledby={`tab-${tab}`} className="flex flex-col gap-4">
        {tab === "alerts" && <AlertsPanel onAuth={onAuth} />}
        {tab === "rules" && <RulesPanel onAuth={onAuth} />}
        {tab === "channels" && <ChannelsPanel onAuth={onAuth} />}
      </div>
    </div>
  );
}

/** The Overview's "Alerts" row: how many firing alerts nobody has acknowledged or silenced. */
export function AlertsAtAGlance({ onAuth }: { onAuth: (m: string) => void }) {
  const [list] = useLoad<AlertList>(() => client.get("alerts"), onAuth);
  if (list.kind !== "ok") return null;
  const c = list.data.counts;
  const n = c.needsAttention;
  const tone = c.critical > 0 && n > 0 ? "status-error" : n > 0 ? "status-warning" : "status-success";
  const value = n === 0 ? "Nothing needs attention" : `${n > 99 ? "99+" : n} ${n === 1 ? "alert needs" : "alerts need"} attention${c.critical > 0 ? ` · ${c.critical} critical` : ""}`;
  return (
    <section aria-label="Alerts" className="rounded-box border border-base-300" data-testid="overview-alerts">
      <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-1 px-4 py-3">
        <span className="text-sm font-medium">Alerts</span>
        <span className="flex items-center gap-2 text-sm text-base-content/70">
          <span className={`status ${tone}`} aria-hidden="true" />
          <span className="tabular-nums">{value}</span>
          <a href="#/alerts" className="link link-hover ml-2" data-testid="overview-alerts-link">Open alerts</a>
        </span>
      </div>
      {list.data.evaluator.stale && <p className="border-t border-base-300 px-4 py-2 text-xs text-base-content/70">{list.data.evaluator.message}</p>}
    </section>
  );
}

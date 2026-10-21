import { useState } from "react";

import type { RecoveryAction, RecoveryItem, RecoveryMember, RecoveryReport } from "@apelora/api-client";

import { Badge, ConfirmDialog, ErrorPanel, TableFrame, sentence, type Tone } from "../../components/ui";
import { client, messageOf, type Load } from "../../lib/api";
import { fmtTime } from "../../lib/identity";

/** What happens to one instance, in words, with its colour. */
const ACTION: Record<RecoveryAction, { label: (target?: string | null) => string; tone: Tone }> = {
  restart: { label: (t) => (t ? `Restarts on ${t}` : "Restarts elsewhere"), tone: "info" },
  "after-fencing": { label: (t) => (t ? `Restarts on ${t} once fenced` : "Restarts once fenced"), tone: "warn" },
  substitute: { label: () => "Gets a substitute elsewhere", tone: "info" },
  wait: { label: () => "Waits for its server", tone: "neutral" },
};

/** A stopped sentence from the server's clause. */
const stop = (s: string) => `${sentence(s)}${/[.!?]$/.test(s) ? "" : "."}`;

function Items({ items }: { items: RecoveryItem[] }) {
  if (items.length === 0) return <p className="text-sm text-base-content/70">Incus lists no instances on this server.</p>;
  return (
    <TableFrame>
      <thead>
        <tr><th>Instance</th><th>What happens</th><th className="max-sm:hidden">Why</th></tr>
      </thead>
      <tbody>
        {items.map((i) => {
          const a = ACTION[i.action];
          return (
            <tr key={`${i.project}/${i.instance}`} data-testid="recovery-item" data-action={i.action}>
              <td className="break-words">
                <span className="font-medium">{i.instance}</span>
                {i.application && <span className="block text-xs text-base-content/60">{i.application}, module {i.module}</span>}
              </td>
              <td>
                <Badge tone={a.tone}>{a.label(i.target)}</Badge>
                <span className="mt-1 block text-xs text-base-content/70 sm:hidden">{stop(i.reason)}</span>
              </td>
              <td className="max-sm:hidden text-sm">{stop(i.reason)}</td>
            </tr>
          );
        })}
      </tbody>
    </TableFrame>
  );
}

function Failed({ m, canConfirm, onConfirm }: { m: RecoveryMember; canConfirm: boolean; onConfirm: (member: string) => void }) {
  const fenced = m.fencing.condition === "fenced";
  const evidence = m.evidence ?? m.fencing.evidence;
  return (
    <section className="flex flex-col gap-3 rounded-box border border-base-300 p-4" aria-labelledby={`failed-${m.member}`} data-testid="failed-server" data-condition={m.fencing.condition}>
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 id={`failed-${m.member}`} className="font-semibold">{m.member} is offline</h3>
        <Badge tone={fenced ? "info" : "warn"} testId="fencing-state">{fenced ? "Fenced" : "Waiting for fencing"}</Badge>
      </div>
      <dl className="grid grid-cols-1 gap-x-6 gap-y-2 text-sm sm:grid-cols-3">
        <div className="flex flex-col gap-0.5"><dt className="text-base-content/70">Noticed</dt><dd>{fmtTime(m.detectedAt)}</dd></div>
        <div className="flex flex-col gap-0.5"><dt className="text-base-content/70">Last heartbeat</dt><dd>{fmtTime(m.lastHeartbeat)}</dd></div>
        <div className="flex flex-col gap-0.5"><dt className="text-base-content/70">Fenced</dt><dd>{m.fencedAt ? fmtTime(m.fencedAt) : "Not yet"}</dd></div>
      </dl>
      <p className="text-sm" data-testid="fencing-reason">{fenced && evidence ? stop(evidence.summary) : stop(m.fencing.reason)}</p>
      {!fenced && m.fencing.nextStep && <p className="text-sm text-base-content/70" data-testid="fencing-next-step">Next step: {stop(m.fencing.nextStep)}</p>}
      <Items items={m.items} />
      {!fenced && canConfirm && (
        <div className="flex flex-wrap justify-end">
          <button type="button" className="btn btn-sm btn-ghost text-error" aria-label={`Confirm ${m.member} is powered off`} onClick={() => onConfirm(m.member)} data-testid="confirm-powered-off">
            Confirm powered off…
          </button>
        </div>
      )}
    </section>
  );
}

/**
 * Servers → Members: failed servers (HA slice B). Whether each is fenced, and per instance
 * whether it restarts on another server or waits, as the server decided. Shown only while a
 * server is offline.
 */
export function FailedServers({ report, anyOffline, canConfirm, onChanged }: { report: Load<RecoveryReport>; anyOffline: boolean; canConfirm: boolean; onChanged: () => void }) {
  const [toConfirm, setToConfirm] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  if (report.kind === "error") return anyOffline && <ErrorPanel message={`Failed servers could not be read: ${report.message}`} testId="recovery-error" />;
  if (report.kind !== "ok" || report.data.items.length === 0) return null;
  const confirm = async (member: string) => {
    setError(null);
    try {
      await client.request("confirm_fenced", { params: { member }, body: { confirm: member, poweredOff: true } });
      setNotice(`${member} is recorded as powered off. Its modules on shared storage restart on other servers at the next supervision pass.`);
      onChanged();
    } catch (e) {
      setError(messageOf(e));
    }
  };
  return (
    <div className="flex flex-col gap-3" data-testid="failed-servers">
      <h2 className="font-semibold">Failed servers</h2>
      <p className="text-sm text-base-content/70">A module on shared storage restarts on another server once the failed one is fenced. Everything else waits for its server.</p>
      {notice && <p role="status" className="flex items-start gap-2 text-sm" data-testid="recovery-notice"><span className="status status-info mt-1.5 shrink-0" aria-hidden="true" />{notice}</p>}
      {error && <ErrorPanel message={error} testId="recovery-confirm-error" />}
      {report.data.items.map((m) => <Failed key={m.member} m={m} canConfirm={canConfirm} onConfirm={setToConfirm} />)}
      <ConfirmDialog
        open={!!toConfirm}
        title={`Confirm ${toConfirm ?? ""} is powered off?`}
        body={
          <div className="flex flex-col gap-2">
            <p>Its modules on shared storage will start on other servers. If the server is in fact still running, two copies would write the same disks and damage their data.</p>
            <p>Confirm only after checking that it is switched off, or cut off from the shared storage.</p>
          </div>
        }
        action="Confirm powered off"
        typeToConfirm={toConfirm ?? undefined}
        onCancel={() => setToConfirm(null)}
        onConfirm={() => { const m = toConfirm; setToConfirm(null); if (m) void confirm(m); }}
      />
    </div>
  );
}

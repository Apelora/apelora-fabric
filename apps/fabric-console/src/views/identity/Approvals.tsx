import { useId, useState } from "react";

import type { ApprovalDto, PermissionDto, PolicyDto, WhoAmI } from "@apelora/api-client";

import { Badge, ErrorPanel, FormDialog, Notice, Section, SectionHeader, TableFrame, type Tone } from "../../components/ui";
import { client, useLoad } from "../../lib/api";
import { actorLabel, actorRef, fmtTime, holdsPlatform, useDirectory, useRun } from "../../lib/identity";

const STATE: Record<string, { tone: Tone; label: string }> = {
  pending: { tone: "warn", label: "Pending" },
  approved: { tone: "ok", label: "Approved" },
  rejected: { tone: "error", label: "Rejected" },
  cancelled: { tone: "neutral", label: "Cancelled" },
  consumed: { tone: "neutral", label: "Used" },
};
const TARGET = /^[A-Za-z0-9:._/-]{1,200}$/;

const stringsOf = (v: unknown): string[] => (Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : []);

/** The header the requester sends with the action that needs this approval. */
function HeaderHint({ id }: { id: string }) {
  const [copied, setCopied] = useState(false);
  const line = `Apelora-Approval: ${id}`;
  return (
    <div className="flex flex-wrap items-center gap-2" data-testid="approval-hint">
      <code className="rounded bg-base-200 px-1.5 py-0.5 text-xs">{line}</code>
      <button type="button" className="btn btn-xs btn-outline" aria-label={`Copy the header for approval ${id}`} onClick={() => void navigator.clipboard.writeText(line).then(() => setCopied(true), () => setCopied(false))}>{copied ? "Copied" : "Copy header"}</button>
    </div>
  );
}

function RequestForm({ actions, onAuth, onClose, onRequested }: { actions: string[]; onAuth: (m: string) => void; onClose: () => void; onRequested: (a: ApprovalDto) => void }) {
  const [action, setAction] = useState("");
  const [target, setTarget] = useState("");
  const [reason, setReason] = useState("");
  const { busy, error, run } = useRun(onAuth);
  const actionId = useId();
  const targetId = useId();
  const reasonId = useId();
  const t = target.trim();
  const targetBad = !!t && !TARGET.test(t);
  const ok = !!action && !!t && !targetBad && reason.length <= 300 && !busy;
  const submit = () => void run(async () => onRequested(await client.request("request_approval", { body: { action, target: t, reason: reason.trim() } })));
  return (
    <form className="flex flex-col gap-3" onSubmit={(e) => { e.preventDefault(); if (ok) submit(); }}>
      <p className="text-sm text-base-content/70">Ask another person with approval rights to approve one risky action on one target. You must be allowed to do the action yourself.</p>
      <div className="flex flex-col gap-1 text-sm">
        <label htmlFor={actionId}>Action</label>
        <select id={actionId} className="select select-sm w-full" value={action} onChange={(e) => setAction(e.target.value)} data-testid="approval-action">
          <option value="">Choose an action…</option>
          {actions.map((a) => <option key={a} value={a}>{a}</option>)}
        </select>
        {actions.length === 0 && <span className="text-xs text-base-content/60">No action currently needs approval.</span>}
      </div>
      <div className="flex flex-col gap-1 text-sm">
        <label htmlFor={targetId}>Target</label>
        <input id={targetId} className="input input-sm w-full font-mono" value={target} onChange={(e) => setTarget(e.target.value)} placeholder="machine:rack1-07" autoComplete="off" spellCheck={false} aria-invalid={targetBad} data-testid="approval-target" />
        <span className={`text-xs ${targetBad ? "text-error" : "text-base-content/60"}`}>What it will be done to, such as machine:rack1-07 or evacuate:node-3 (letters, digits and : . _ - /).</span>
      </div>
      <div className="flex flex-col gap-1 text-sm">
        <label htmlFor={reasonId}>Reason (optional)</label>
        <textarea id={reasonId} className="textarea textarea-sm w-full" rows={2} value={reason} onChange={(e) => setReason(e.target.value)} maxLength={300} data-testid="approval-reason" />
      </div>
      {error && <ErrorPanel message={error} testId="approval-request-error" />}
      <div className="modal-action mt-0">
        <button type="button" className="btn btn-sm btn-ghost" onClick={onClose}>Cancel</button>
        <button type="submit" className="btn btn-sm btn-primary" disabled={!ok} data-testid="approval-submit">{busy ? "Requesting…" : "Request approval"}</button>
      </div>
    </form>
  );
}

export function ApprovalsPanel({ me, catalogue, policies, onAuth }: { me: WhoAmI | null; catalogue: PermissionDto[]; policies: PolicyDto[]; onAuth: (m: string) => void }) {
  const [list, reload] = useLoad<ApprovalDto[]>(() => client.get("approvals").then((r) => r.items), onAuth);
  const dir = useDirectory(me, onAuth, { users: true });
  const [requesting, setRequesting] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const { busy, error, run } = useRun(onAuth);
  const canRequest = holdsPlatform(me, "approval.request");
  const seesAll = holdsPlatform(me, "approval.grant");
  // Only a signed-in person decides; an API token that holds approval.grant still sees the list.
  const canDecide = seesAll && me?.kind === "user";
  const mine = me ? actorRef(me) : "";
  const actions = [...new Set([
    ...catalogue.filter((p) => p.needsApproval).map((p) => p.id),
    ...policies.filter((p) => p.kind === "approval" && p.enabled).flatMap((p) => stringsOf(p.parameters.actions)),
  ])].sort();

  const act = (a: ApprovalDto, op: "approve_request" | "reject_request" | "cancel_request", done: string) => {
    setNotice(null);
    void run(async () => {
      await client.request(op, { params: { id: a.id } });
      setNotice(done);
      reload();
    });
  };

  return (
    <Section title="Approvals" state={list} testId="approvals" onRetry={reload}>
      {(items) => (
        <>
          <SectionHeader title="Approvals" count={items.length}>
            <button type="button" className="btn btn-sm btn-ghost" onClick={reload}>Refresh</button>
            {canRequest && <button type="button" className="btn btn-sm btn-primary" onClick={() => setRequesting(true)} data-testid="approval-open">Request approval</button>}
          </SectionHeader>
          <p className="text-sm text-base-content/70">Some actions are carried out only after a second person approves them. An approval is good once, for 30 minutes, for exactly the action and target requested. You see your own requests{seesAll ? " and every open one" : ""}.</p>
          {notice && <Notice tone="ok" testId="approvals-notice">{notice}</Notice>}
          {error && <ErrorPanel message={error} testId="approvals-error" />}
          {items.length === 0 ? <p className="text-sm text-base-content/70" data-testid="approvals-empty">No approval requests.</p> : (
            <TableFrame>
              <thead><tr><th>Request</th><th className="max-sm:hidden">Requested by</th><th>State</th><th className="max-md:hidden">Expires</th><th><span className="sr-only">Actions</span></th></tr></thead>
              <tbody>
                {items.map((a) => {
                  const isMine = a.requester === mine;
                  const s = STATE[a.state] ?? { tone: "neutral" as Tone, label: a.state };
                  return (
                    <tr key={a.id} data-testid="approval-row" data-state={a.state}>
                      <td className="align-top">
                        <span className="font-mono text-xs">{a.action}</span> on <span className="font-mono text-xs">{a.target}</span>
                        {a.reason && <span className="block text-xs text-base-content/60">{a.reason}</span>}
                        {isMine && a.state === "approved" && (
                          <div className="mt-2 flex flex-col gap-1 text-xs text-base-content/70">
                            Send this header with the request that needs the approval:
                            <HeaderHint id={a.id} />
                          </div>
                        )}
                      </td>
                      <td className="align-top max-sm:hidden">{isMine ? "You" : actorLabel(a.requester, dir.names)}{a.approver && <span className="block text-xs text-base-content/60">decided by {actorLabel(a.approver, dir.names)}</span>}</td>
                      <td className="align-top"><Badge tone={s.tone}>{s.label}</Badge></td>
                      <td className="align-top whitespace-nowrap max-md:hidden">{fmtTime(a.expiresAt)}</td>
                      <td className="whitespace-nowrap text-right align-top">
                        {a.state === "pending" && canDecide && (
                          <>
                            <button type="button" className="btn btn-xs btn-ghost" disabled={busy} aria-label={`Approve ${a.action} on ${a.target}`} onClick={() => act(a, "approve_request", `Approved ${a.action} on ${a.target}. The requester has 30 minutes to use it once.`)}>Approve</button>
                            <button type="button" className="btn btn-xs btn-ghost text-error" disabled={busy} aria-label={`Reject ${a.action} on ${a.target}`} onClick={() => act(a, "reject_request", `Rejected ${a.action} on ${a.target}.`)}>Reject</button>
                          </>
                        )}
                        {a.state === "pending" && isMine && canRequest && (
                          <button type="button" className="btn btn-xs btn-ghost" disabled={busy} aria-label={`Cancel the request for ${a.action} on ${a.target}`} onClick={() => act(a, "cancel_request", `Your request for ${a.action} on ${a.target} was cancelled.`)}>Cancel request</button>
                        )}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </TableFrame>
          )}
          <FormDialog open={requesting} title="Request approval" onClose={() => setRequesting(false)} testId="approval-dialog">
            {requesting && <RequestForm actions={actions} onAuth={onAuth} onClose={() => setRequesting(false)} onRequested={(a) => { setRequesting(false); setNotice(`Requested ${a.action} on ${a.target}. Someone else with approval rights has to approve it.`); reload(); }} />}
          </FormDialog>
        </>
      )}
    </Section>
  );
}

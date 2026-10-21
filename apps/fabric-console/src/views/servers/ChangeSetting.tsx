// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useEffect, useId, useRef, useState } from "react";

import { ApiRequestError, type ApprovalDto, type ClusterSetting, type SettingsChangeReport } from "@apelora/api-client";

import { Badge, ErrorPanel, Notice, type Tone } from "../../components/ui";
import { client } from "../../lib/api";
import { actorRef, describeError, fmtTime } from "../../lib/identity";

const ACTION = "cluster.sensitive_change";
const POLL_MS = 5_000;
const STATE: Record<string, { tone: Tone; label: string }> = {
  pending: { tone: "warn", label: "Waiting for approval" },
  approved: { tone: "ok", label: "Approved" },
  rejected: { tone: "error", label: "Rejected" },
  cancelled: { tone: "neutral", label: "Cancelled" },
  consumed: { tone: "neutral", label: "Used" },
};
const usable = (a: ApprovalDto | null) => !!a && (a.state === "pending" || a.state === "approved") && Date.parse(a.expiresAt) > Date.now();

type Step = "edit" | "review" | "done";

/**
 * Change one cluster setting: pick the value, review what changes and what to watch for (a dry
 * run: nothing is changed), have another person approve exactly this change, then apply it. The
 * control plane checks everything again; this dialog only shows its answers. Legacy
 * administrator tokens are not subject to approvals, so they apply straight away.
 */
export function ChangeSetting({ setting, needsApproval, onClose, onChanged, onAuth }: {
  setting: ClusterSetting;
  needsApproval: boolean;
  onClose: () => void;
  onChanged: () => void;
  onAuth: (m: string) => void;
}) {
  const [step, setStep] = useState<Step>("edit");
  const [value, setValue] = useState(setting.value ?? "");
  const [report, setReport] = useState<SettingsChangeReport | null>(null);
  const [request, setRequest] = useState<ApprovalDto | null>(null);
  const [reason, setReason] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const dialog = useRef<HTMLDialogElement>(null);
  const self = useRef<Promise<string> | null>(null);
  const ids = { value: useId(), reason: useId(), title: useId() };

  useEffect(() => { dialog.current?.showModal(); }, []);
  const fail = useCallback((e: unknown) => {
    if (e instanceof ApiRequestError && e.unauthenticated) onAuth(e.message);
    setError(describeError(e));
  }, [onAuth]);

  const body = (dryRun: boolean) => ({ changes: { [setting.key]: value.trim() === "" ? null : value.trim() }, dryRun });

  const review = async () => {
    setBusy(true);
    setError(null);
    try {
      setReport(await client.request("change_cluster_settings", { body: body(true) }));
      setStep("review");
    } catch (e) {
      fail(e);
    } finally {
      setBusy(false);
    }
  };

  // The caller's newest request for exactly this change (people who may approve also see others').
  const target = report?.approvalTarget;
  const check = useCallback(async () => {
    if (!target) return;
    try {
      self.current ??= client.get("whoami").then(actorRef);
      const [{ items }, me] = await Promise.all([client.get("approvals"), self.current]);
      const mine = items.filter((a) => a.action === ACTION && a.target === target && a.requester === me).sort((a, b) => b.createdAt.localeCompare(a.createdAt));
      setRequest(mine[0] ?? null);
    } catch (e) {
      self.current = null;
      fail(e);
    }
  }, [target, fail]);
  useEffect(() => { if (step === "review" && needsApproval) void check(); }, [step, needsApproval, check]);
  const pending = request?.state === "pending";
  useEffect(() => {
    if (!pending) return;
    const id = setInterval(() => { if (document.visibilityState === "visible") void check(); }, POLL_MS);
    return () => clearInterval(id);
  }, [pending, check]);

  const ask = async () => {
    if (!target) return;
    setBusy(true);
    setError(null);
    try {
      setRequest(await client.request("request_approval", { body: { action: ACTION, target, reason: reason.trim() } }));
    } catch (e) {
      fail(e);
    } finally {
      setBusy(false);
    }
  };

  const approved = usable(request) && request?.state === "approved" ? request : null;
  const apply = async () => {
    setBusy(true);
    setError(null);
    try {
      setReport(await client.request("change_cluster_settings", { body: body(false), approvalId: approved?.id }));
      setStep("done");
      onChanged();
    } catch (e) {
      fail(e);
    } finally {
      setBusy(false);
    }
  };

  const change = report?.changes[0];
  const state = request ? STATE[request.state] ?? { tone: "neutral" as Tone, label: request.state } : null;
  const unset = value.trim() === "";
  return (
    <dialog ref={dialog} className="modal" aria-labelledby={ids.title} data-testid="change-setting" onCancel={(e) => { e.preventDefault(); onClose(); }}>
      <div className="modal-box flex max-w-lg flex-col gap-3">
        <h2 id={ids.title} className="text-base font-semibold">{step === "done" ? "Setting changed" : `Change “${setting.label}”`}</h2>
        <code className="text-xs text-base-content/60">{setting.key}</code>

        {step === "edit" && (
          <>
            <p className="text-sm text-base-content/70">{setting.meaning}</p>
            <div className="flex flex-col gap-1 text-sm">
              <label htmlFor={ids.value}>New value</label>
              <input id={ids.value} className="input input-sm w-full" value={value} onChange={(e) => setValue(e.target.value)} autoComplete="off" spellCheck={false} data-testid="setting-value" />
              <span className="text-xs text-base-content/60">
                Now: {setting.usesDefault ? `Incus's default${setting.default != null ? ` (${setting.default})` : ""}` : setting.value}.
                {" "}Leave it empty to go back to Incus's default{setting.default != null ? ` (${setting.default})` : ""}.
              </span>
            </div>
          </>
        )}

        {step === "review" && change && (
          <div className="flex flex-col gap-3 text-sm" data-testid="setting-review">
            <p>
              <span className="text-base-content/70">{change.from ?? "default"}</span>
              <span aria-hidden="true"> → </span><span className="sr-only"> becomes </span>
              <strong>{change.to ?? "default"}</strong>
            </p>
            {change.warnings.map((w) => <Notice key={w} tone="warn" testId="setting-warning">{w}</Notice>)}
            {needsApproval ? (
              <fieldset className="flex flex-col gap-2 rounded-box border border-warning/50 p-3" data-testid="setting-approval">
                <legend className="px-1 text-sm font-medium">Another person approves first</legend>
                {request && state && (
                  <div className="flex flex-col gap-1" data-testid="approval-state" data-state={request.state}>
                    <span className="flex flex-wrap items-center gap-2"><Badge tone={state.tone}>{state.label}</Badge><span className="text-base-content/70">requested {fmtTime(request.createdAt)}</span></span>
                    {approved && <span>Approved. Apply the change before {fmtTime(approved.expiresAt)}.</span>}
                    {request.state === "rejected" && <span>It was rejected. You can ask again with more detail.</span>}
                  </div>
                )}
                {!approved && !pending && (
                  <div className="flex flex-col gap-2 sm:flex-row sm:items-end">
                    <div className="flex flex-1 flex-col gap-1">
                      <label htmlFor={ids.reason}>Why is this needed?</label>
                      <input id={ids.reason} className="input input-sm w-full" value={reason} maxLength={300} onChange={(e) => setReason(e.target.value)} autoComplete="off" data-testid="approval-reason" />
                    </div>
                    <button type="button" className="btn btn-sm btn-outline" disabled={busy || reason.trim().length < 3} onClick={() => void ask()} data-testid="approval-request">{busy ? "Requesting…" : "Request approval"}</button>
                  </div>
                )}
                {pending && <button type="button" className="btn btn-xs btn-ghost self-start" onClick={() => void check()} data-testid="approval-check">Check now</button>}
              </fieldset>
            ) : (
              <Notice tone="info">This credential does not need an approval. The change is audited.</Notice>
            )}
          </div>
        )}

        {step === "done" && change && (
          <p role="status" className="text-sm" data-testid="setting-done">{change.label} is now <strong>{change.to ?? "Incus's default"}</strong>. The change is in the audit trail.</p>
        )}

        {error && <ErrorPanel message={error} testId="setting-error" />}

        <div className="modal-action mt-0">
          {step === "done" ? (
            <button type="button" className="btn btn-sm btn-primary" onClick={onClose}>Close</button>
          ) : (
            <>
              <button type="button" className="btn btn-sm btn-ghost" onClick={onClose}>Cancel</button>
              {step === "edit" && <button type="button" className="btn btn-sm btn-primary" disabled={busy || (unset && setting.usesDefault)} onClick={() => void review()} data-testid="setting-review-btn">{busy ? "Checking…" : "Review change"}</button>}
              {step === "review" && <button type="button" className="btn btn-sm btn-primary" disabled={busy || (needsApproval && !approved)} onClick={() => void apply()} data-testid="setting-apply">{busy ? "Applying…" : "Apply change"}</button>}
            </>
          )}
        </div>
      </div>
    </dialog>
  );
}

import { useEffect, useState } from "react";

import { useLifecycle } from "../../features/applications/lifecycle";
import { useAppToasts } from "../../features/applications/toast";
import type { AppView, OperationKind } from "../../features/applications/model";
import { IconPlay, IconRestart, IconSpinner, IconStop } from "../icons";

const BUTTONS: { kind: OperationKind; label: string; icon: typeof IconPlay }[] = [
  { kind: "start", label: "Start", icon: IconPlay },
  { kind: "restart", label: "Restart", icon: IconRestart },
  { kind: "stop", label: "Stop", icon: IconStop },
];

/**
 * Start, Restart and Stop for one application. Which are available comes from the control plane
 * (it knows the permissions and the observed state); while an operation is submitted or running
 * every button is disabled and the clicked one shows progress. Nothing is "done" here until the
 * control plane says so.
 */
export function LifecycleActions({ app, size = "xs" }: { app: AppView; size?: "xs" | "sm" }) {
  const tracked = useLifecycle((s) => s.tracked[app.name]);
  const ask = useLifecycle((s) => s.ask);
  const run = useLifecycle((s) => s.run);
  const toast = useAppToasts((s) => s.push);
  const busy = !!tracked;
  const busyReason = tracked ? `${tracked.kind === "stop" ? "Stopping" : tracked.kind === "start" ? "Starting" : "Restarting"}${tracked.total ? ` (${tracked.done}/${tracked.total} steps)` : "…"}` : null;

  const click = async (kind: OperationKind) => {
    if (busy) return;
    if (kind === "start") {
      const r = await run(app.name, "start", false);
      if (!r.ok) toast("error", r.message);
    } else ask(app, kind);
  };

  return (
    <div className="join" role="group" aria-label={`Lifecycle of ${app.name}`} data-testid="lifecycle" data-busy={busy}>
      {BUTTONS.map(({ kind, label, icon: Icon }) => {
        const a = app.actions[kind];
        const disabled = busy || !a.allowed;
        const reason = busyReason ?? (a.allowed ? `${label} ${app.name}` : (a.reason ?? "Not available"));
        const showSpinner = tracked?.kind === kind;
        return (
          <span key={kind} className="join-item" title={reason}>
            <button
              type="button"
              className={`btn btn-${size} btn-outline join-item rounded-none first:rounded-l-md last:rounded-r-md`}
              disabled={disabled}
              aria-label={`${label} ${app.name}`}
              data-testid={`action-${kind}`}
              data-allowed={a.allowed}
              onClick={() => void click(kind)}
            >
              {showSpinner ? <span className="animate-spin"><IconSpinner size={13} /></span> : <Icon size={13} />}
              <span>{label}</span>
            </button>
          </span>
        );
      })}
      {busyReason && <span className="sr-only" role="status" data-testid="lifecycle-progress">{busyReason}</span>}
    </div>
  );
}

/** The asking dialogs for Stop and Restart: what happens, then (if the control plane says clients lose service) a stronger warning. */
export function LifecycleDialogs() {
  const asking = useLifecycle((s) => s.asking);
  const close = useLifecycle((s) => s.close);
  const run = useLifecycle((s) => s.run);
  const push = useAppToasts((s) => s.push);
  const [warning, setWarning] = useState<string | null>(null);
  const [sending, setSending] = useState(false);
  useEffect(() => { setWarning(null); setSending(false); }, [asking]);
  useEffect(() => {
    if (!asking) return;
    const on = (e: KeyboardEvent) => { if (e.key === "Escape") close(); };
    window.addEventListener("keydown", on);
    return () => window.removeEventListener("keydown", on);
  }, [asking, close]);
  if (!asking) return null;
  const { app, kind } = asking;
  const verb = kind === "stop" ? "Stop" : "Restart";
  const go = async (confirm: boolean) => {
    if (sending) return;
    setSending(true);
    const r = await run(app.name, kind, confirm);
    if (r.ok) { close(); return; }
    setSending(false);
    if (r.code === "confirmation_required") setWarning(r.message);
    else { push("error", r.message); close(); }
  };
  return (
    <dialog className="modal modal-open" aria-labelledby="lc-title" data-testid="lifecycle-dialog">
      <div className="modal-box max-w-md">
        <h3 id="lc-title" className="text-base font-semibold">{verb} {app.name}?</h3>
        <p className="mt-2 text-sm text-base-content/80">
          {kind === "stop"
            ? "The control plane stops this application's instances in dependency order: what depends on others stops first. Each instance gets a graceful shutdown. Definitions and data are kept; only this application's own instances are touched."
            : "Everything that is running stops in dependency order and starts again providers first. Clients are interrupted while it restarts."}
        </p>
        {warning && <p role="alert" className="mt-3 rounded-box border border-warning/50 bg-warning/10 px-3 py-2 text-sm" data-testid="lifecycle-warning">{warning}</p>}
        <p className="mt-2 text-xs text-base-content/60">It is only reported as {kind === "stop" ? "stopped" : "restarted"} after Incus confirms every instance.</p>
        <div className="modal-action">
          <button type="button" className="btn btn-sm" onClick={close}>Cancel</button>
          {warning
            ? <button type="button" className="btn btn-sm btn-error" disabled={sending} onClick={() => void go(true)} data-testid="lifecycle-confirm-disruption">{verb} and interrupt service</button>
            : <button type="button" className="btn btn-sm btn-primary" disabled={sending} onClick={() => void go(false)} data-testid="lifecycle-confirm">{verb}</button>}
        </div>
      </div>
      <div className="modal-backdrop" onClick={close} />
    </dialog>
  );
}

import { useEffect, useState } from "react";

import { useLifecycle } from "../../features/applications/lifecycle";
import { useAppToasts } from "../../features/applications/toast";
import type { AppView, OperationKind } from "../../features/applications/model";
import { IconMove, IconPlay, IconPower, IconRestart, IconSpinner, IconStop } from "../icons";
import { MigrateDialog } from "./MigrateDialog";

/** What a person can ask for; an update comes from deploying a changed version. */
const BUTTONS: { kind: Exclude<OperationKind, "update">; label: string; icon: typeof IconPlay; danger?: boolean }[] = [
  { kind: "start", label: "Start", icon: IconPlay },
  { kind: "restart", label: "Restart", icon: IconRestart },
  { kind: "shutdown", label: "Shut down", icon: IconPower },
  { kind: "stop", label: "Stop", icon: IconStop, danger: true },
  { kind: "migrate", label: "Migrate…", icon: IconMove },
];

const PROGRESS: Record<OperationKind, string> = { start: "Starting", restart: "Restarting", shutdown: "Shutting down", stop: "Stopping", migrate: "Migrating", update: "Updating" };

/**
 * Start, Restart, Shut down, Stop and Migrate for one application. Which are available comes from
 * the control plane (it knows the permissions and the observed state); while an operation is
 * submitted or running every button is disabled and the clicked one shows progress. Nothing is
 * "done" here until the control plane says so. Shut down asks each guest to stop; Stop cuts the
 * power at once; Migrate asks which server first.
 */
export function LifecycleActions({ app, size = "xs" }: { app: AppView; size?: "xs" | "sm" }) {
  const tracked = useLifecycle((s) => s.tracked[app.name]);
  const ask = useLifecycle((s) => s.ask);
  const run = useLifecycle((s) => s.run);
  const toast = useAppToasts((s) => s.push);
  const [migrating, setMigrating] = useState(false);
  const busy = !!tracked;
  const busyReason = tracked ? `${PROGRESS[tracked.kind]}${tracked.total ? ` (${tracked.done}/${tracked.total} steps)` : "…"}` : null;

  const click = async (kind: OperationKind) => {
    if (busy) return;
    if (kind === "start") {
      const r = await run(app.name, "start", false);
      if (!r.ok) toast("error", r.message);
    } else if (kind === "migrate") setMigrating(true);
    else ask(app, kind);
  };

  return (
    <div className="flex flex-wrap items-center gap-1" role="group" aria-label={`Lifecycle of ${app.name}`} data-testid="lifecycle" data-busy={busy}>
      {BUTTONS.map(({ kind, label, icon: Icon, danger }) => {
        // A control plane older than ADR 0015 has no shutdown or migrate.
        const a = app.actions[kind] ?? { allowed: false, reason: "This control plane does not offer it yet" };
        const disabled = busy || !a.allowed;
        const reason = busyReason ?? (a.allowed ? `${label.replace("…", "")} ${app.name}` : (a.reason ?? "Not available"));
        const showSpinner = tracked?.kind === kind;
        return (
          <span key={kind} title={reason}>
            <button
              type="button"
              className={`btn btn-${size} btn-outline ${danger ? "text-error" : ""}`}
              disabled={disabled}
              aria-label={`${label.replace("…", "")} ${app.name}`}
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
      {migrating && <MigrateDialog app={app} onClose={() => setMigrating(false)} />}
    </div>
  );
}

const TITLE: Partial<Record<OperationKind, string>> = { shutdown: "Shut down", stop: "Stop", restart: "Restart" };
const PAST: Partial<Record<OperationKind, string>> = { shutdown: "shut down", stop: "stopped", restart: "restarted" };

/** The asking dialogs for Shut down, Stop and Restart: what happens, then (if the control plane says clients lose service) a stronger warning. */
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
  const verb = TITLE[kind] ?? "Stop";
  const go = async (confirm: boolean) => {
    if (sending) return;
    setSending(true);
    const r = await run(app.name, kind, confirm);
    if (r.ok) { close(); return; }
    setSending(false);
    if (r.code === "confirmation_required") setWarning(r.message);
    else { push("error", r.message); close(); }
  };
  const body: Record<string, string> = {
    shutdown: "The control plane asks this application's instances to shut down, in dependency order: what depends on a module stops first. Each instance gets time to finish what it is doing. Definitions and data are kept; only this application's own instances are touched.",
    stop: "Every instance is powered off at once, in dependency order, without waiting for it to shut down. Anything an instance had only in memory is lost; data on disks and volumes is kept. Use Shut down first unless an instance does not respond.",
    restart: "Everything that is running stops in dependency order and starts again providers first. Clients are interrupted while it restarts.",
  };
  return (
    <dialog className="modal modal-open" aria-labelledby="lc-title" data-testid="lifecycle-dialog">
      <div className="modal-box max-w-md">
        <h3 id="lc-title" className="text-base font-semibold">{kind === "stop" ? `Stop ${app.name} now?` : `${verb} ${app.name}?`}</h3>
        <p className="mt-2 text-sm text-base-content/80">{body[kind]}</p>
        {warning && <p role="alert" className="mt-3 rounded-box border border-warning/50 bg-warning/10 px-3 py-2 text-sm" data-testid="lifecycle-warning">{warning}</p>}
        <p className="mt-2 text-xs text-base-content/60">It is only reported as {PAST[kind]} after the cluster confirms every instance.</p>
        <div className="modal-action">
          <button type="button" className="btn btn-sm" onClick={close}>Cancel</button>
          {warning
            ? <button type="button" className="btn btn-sm btn-error" disabled={sending} onClick={() => void go(true)} data-testid="lifecycle-confirm-disruption">{verb} and interrupt service</button>
            : <button type="button" className={`btn btn-sm ${kind === "stop" ? "btn-error" : "btn-primary"}`} disabled={sending} onClick={() => void go(false)} data-testid="lifecycle-confirm">{verb}</button>}
        </div>
      </div>
      <div className="modal-backdrop" onClick={close} />
    </dialog>
  );
}

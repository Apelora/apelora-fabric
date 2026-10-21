// SPDX-License-Identifier: AGPL-3.0-only
import { Suspense, lazy, useId } from "react";

/** The instance a session is opened into: an Apelora instance of one application. */
export interface TerminalTarget {
  application: string;
  instance: string;
}

// The terminal emulator is large and only needed once someone opens a session.
const TerminalSession = lazy(() => import("./TerminalSession").then((m) => ({ default: m.TerminalSession })));

/** Shown while the terminal code loads, so the button always answers at once. */
function Opening({ onClose }: { onClose: () => void }) {
  const titleId = useId();
  return (
    <div className="modal modal-open" role="dialog" aria-modal="true" aria-labelledby={titleId} data-testid="session-loading">
      <div className="modal-box flex max-w-md flex-col gap-3">
        <h2 id={titleId} className="font-semibold">Opening the terminal</h2>
        <p role="status" className="flex items-center gap-2 text-sm text-base-content/70"><span className="loading loading-spinner loading-sm" aria-hidden="true" />Loading…</p>
        <div className="modal-action mt-0">
          <button type="button" className="btn btn-sm btn-ghost" onClick={onClose}>Cancel</button>
        </div>
      </div>
    </div>
  );
}

/**
 * A terminal (or PowerShell) session into one instance, brokered by the control plane: the same
 * sessions the Studio opens. Mount it anywhere an instance is shown (lists, a detail page) and
 * pass the instance as `target`; `null` renders nothing. The control plane decides everything
 * (permission, approval in production, ticket, origin, limits); the dialog shows its answers.
 */
export function TerminalDialog({ target, onClose, onAuth }: { target: TerminalTarget | null; onClose: () => void; onAuth: (message: string) => void }) {
  if (!target) return null;
  return (
    <Suspense fallback={<Opening onClose={onClose} />}>
      <TerminalSession key={`${target.application}/${target.instance}`} target={target} onClose={onClose} onAuth={onAuth} />
    </Suspense>
  );
}

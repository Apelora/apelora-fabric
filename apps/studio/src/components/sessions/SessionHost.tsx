import { Suspense, lazy, useEffect } from "react";

import { useBackend } from "../../features/applications/backend";
import { useSessions } from "../../features/sessions/sessions";

// The terminal library is large and only needed once someone opens a session.
const SessionDialog = lazy(() => import("./SessionDialog").then((m) => ({ default: m.SessionDialog })));

/** Shown while the terminal code loads, so opening a session always answers at once. */
function Opening() {
  return (
    <div className="modal modal-open" role="dialog" aria-modal="true" aria-labelledby="session-loading-title" data-testid="session-loading">
      <div className="modal-box flex max-w-md flex-col gap-3">
        <h2 id="session-loading-title" className="text-base font-semibold">Opening the terminal</h2>
        <p role="status" className="flex items-center gap-2 text-sm text-base-content/70"><span className="loading loading-spinner loading-sm" aria-hidden="true" />Loading…</p>
        <div className="modal-action mt-0">
          <button type="button" className="btn btn-sm btn-ghost" onClick={() => useSessions.getState().close()}>Cancel</button>
        </div>
      </div>
    </div>
  );
}

/** Follows what the signed-in person may open on the application and hosts the session dialog. */
export function SessionHost({ app }: { app: string }) {
  const kind = useBackend((s) => s.backend.kind);
  const open = useSessions((s) => s.target !== null);
  useEffect(() => {
    useSessions.getState().watch(kind === "api" ? app : null);
    return () => useSessions.getState().watch(null);
  }, [app, kind]);
  return open ? <Suspense fallback={<Opening />}><SessionDialog /></Suspense> : null;
}

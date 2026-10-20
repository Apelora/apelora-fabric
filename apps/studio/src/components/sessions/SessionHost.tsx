import { Suspense, lazy, useEffect } from "react";

import { useBackend } from "../../features/applications/backend";
import { useSessions } from "../../features/sessions/sessions";

// The terminal library is large and only needed once someone opens a session.
const SessionDialog = lazy(() => import("./SessionDialog").then((m) => ({ default: m.SessionDialog })));

/** Follows what the signed-in person may open on the application and hosts the session dialog. */
export function SessionHost({ app }: { app: string }) {
  const kind = useBackend((s) => s.backend.kind);
  const open = useSessions((s) => s.target !== null);
  useEffect(() => {
    useSessions.getState().watch(kind === "api" ? app : null);
    return () => useSessions.getState().watch(null);
  }, [app, kind]);
  return open ? <Suspense fallback={null}><SessionDialog /></Suspense> : null;
}

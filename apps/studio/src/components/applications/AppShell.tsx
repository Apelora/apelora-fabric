import { useEffect, useState } from "react";

import { useBackend } from "../../features/applications/backend";
import { useAppToasts } from "../../features/applications/toast";
import { THEMES, useTheme } from "../../features/theme";
import { onLinkClick, paths, type Route } from "../../features/router";
import { IconX } from "../icons";
import { ConnectDialog } from "./ConnectDialog";
import { LifecycleDialogs } from "./LifecycleActions";

function Toasts() {
  const { toasts, dismiss } = useAppToasts();
  const tone = { info: "alert-info", success: "alert-success", warning: "alert-warning", error: "alert-error" } as const;
  return (
    <div className="toast toast-bottom toast-center z-50" role="status" aria-live="polite" data-testid="toasts">
      {toasts.map((t) => (
        <div key={t.id} className={`alert alert-soft ${tone[t.kind]} max-w-md py-2 text-xs`} data-testid="toast">
          <span>{t.text}</span>
          <button className="btn btn-ghost btn-xs btn-square" aria-label="Dismiss" onClick={() => dismiss(t.id)}><IconX size={12} /></button>
        </div>
      ))}
    </div>
  );
}

/** The frame around the Applications pages: product header, where the data lives, theme. */
export function AppShell({ route, children, bare = false }: { route: Route; children: React.ReactNode; bare?: boolean }) {
  const [theme, setTheme] = useTheme();
  const { backend, status, reason, who, detect } = useBackend();
  const [connect, setConnect] = useState(false);
  useEffect(() => { if (status === "detecting") void detect(); }, [status, detect]);
  const link = (href: string, label: string, active: boolean) => (
    <a href={href} onClick={onLinkClick} className={`rounded-md px-2.5 py-1 text-xs font-medium ${active ? "bg-base-200 text-base-content" : "text-base-content/70 hover:bg-base-200"}`} aria-current={active ? "page" : undefined} data-testid={`nav-${label.toLowerCase().replace(/\s+/g, "-")}`}>{label}</a>
  );
  // The Designer is the editor with its own header; it only needs the dialogs and the sign-in check.
  if (bare) return <div className="h-full bg-base-100 text-base-content" data-testid="apps-shell">{children}<LifecycleDialogs /></div>;
  return (
    <div className="flex min-h-full flex-col bg-base-100 text-base-content" data-testid="apps-shell">
      <header className="flex min-h-12 shrink-0 flex-wrap items-center gap-x-4 gap-y-1 border-b border-base-300 bg-base-100 px-4 py-1">
        <div className="flex items-center gap-2 whitespace-nowrap">
          <img src="/logo.svg" alt="" className="size-6 rounded" />
          <span className="text-sm font-semibold">Apelora Studio</span>
        </div>
        <nav className="flex items-center gap-1" aria-label="Studio sections">
          {link(paths.applications(), "Applications", route.name !== "library")}
          {link(paths.library(), "Assembly library", false)}
        </nav>
        <div className="ml-auto flex flex-wrap items-center gap-2">
          <button type="button" className="badge badge-ghost badge-sm gap-1.5" onClick={() => setConnect(true)} title={reason ?? (who ? `Signed in as ${who}` : "")} data-testid="source-chip" data-source={backend.kind}>
            <span className={`status ${backend.kind === "api" ? "status-success" : "status-warning"}`} aria-hidden="true" />
            {backend.kind === "api" ? `Control plane · ${who ?? ""}` : "This browser · not connected"}
          </button>
          <select className="select select-sm w-24" aria-label="Theme" value={theme} onChange={(e) => setTheme(e.target.value)}>{THEMES.map((t) => <option key={t} value={t}>{t}</option>)}</select>
        </div>
      </header>
      <div className="min-h-0 flex-1">{children}</div>
      {connect && <ConnectDialog onClose={() => setConnect(false)} />}
      <LifecycleDialogs />
      <Toasts />
    </div>
  );
}

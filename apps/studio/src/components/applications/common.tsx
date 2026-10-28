import { RUNTIME_LABEL, DESIGN_LABEL, type AppView, type DeploymentStatus, type DesignStatus } from "../../features/applications/model";
import { iconFor } from "../../features/applications/icons";

/** The application's icon in the editor's own style: its logo on a tile tinted like the category. */
export function AppIcon({ icon, size = 32 }: { icon: string; size?: number }) {
  const c = iconFor(icon);
  const inner = Math.round(size * 0.62);
  return (
    <span className="inline-flex shrink-0 items-center justify-center rounded-lg border" data-testid="app-icon" data-icon={icon} style={{ width: size, height: size, borderColor: `color-mix(in oklab, ${c.tint} 35%, var(--color-base-300))`, background: `color-mix(in oklab, ${c.tint} 12%, var(--color-base-100))` }} aria-hidden="true">
      {c.url ? <img src={c.url} alt="" width={inner} height={inner} className="apl-logo" style={{ width: inner, height: inner }} draggable={false} /> : <span className="rounded bg-base-300" style={{ width: inner, height: inner }} />}
    </span>
  );
}

const RUNTIME_TONE: Record<DeploymentStatus, string> = {
  running: "badge-success",
  degraded: "badge-warning",
  failed: "badge-error",
  starting: "badge-info",
  stopping: "badge-info",
  migrating: "badge-info",
  updating: "badge-info",
  stopped: "badge-neutral",
  "not-deployed": "badge-ghost",
  unknown: "badge-ghost",
};
const DOT: Record<DeploymentStatus, string> = { running: "status-success", degraded: "status-warning", failed: "status-error", starting: "status-info animate-pulse", stopping: "status-info animate-pulse", migrating: "status-info animate-pulse", updating: "status-info animate-pulse", stopped: "status-neutral", "not-deployed": "status-neutral", unknown: "status-neutral" };

/** Runtime status, always with its words so colour is never the only signal. */
export function RuntimeBadge({ app, className = "" }: { app: AppView; className?: string }) {
  const s = app.deployment.status;
  const when = new Date(app.deployment.observedAt).toLocaleString();
  const note = app.deployment.stale ? "Could not be observed; the last answer is not shown as healthy." : `Observed ${when}${app.deployment.source === "mock" ? " (mock data, not a real cluster)" : ""}`;
  return (
    <span className={`badge badge-soft badge-sm gap-1.5 whitespace-nowrap ${RUNTIME_TONE[s]} ${s === "unknown" ? "border-dashed" : ""} ${className}`} data-testid="runtime-status" data-status={s} title={note}>
      <span className={`status ${DOT[s]}`} aria-hidden="true" />
      {RUNTIME_LABEL[s]}
    </span>
  );
}

const DESIGN_TONE: Record<DesignStatus, string> = { draft: "badge-info", validated: "badge-success", published: "badge-neutral" };

export function DesignBadge({ app, className = "" }: { app: AppView; className?: string }) {
  const d = app.design;
  return (
    <span className={`badge badge-soft badge-sm whitespace-nowrap ${DESIGN_TONE[d.status]} ${className}`} data-testid="design-status" data-status={d.status} title={d.hasUnpublishedChanges ? "The draft has changes no published version contains." : undefined}>
      {DESIGN_LABEL[d.status]}{d.hasUnpublishedChanges ? " · changed" : ""}
    </span>
  );
}

const rtf = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });
export function timeAgo(iso: string | null | undefined, now = Date.now()): string {
  if (!iso) return "—";
  const t = new Date(iso).getTime();
  if (!Number.isFinite(t)) return "—";
  const s = Math.round((t - now) / 1000);
  const a = Math.abs(s);
  if (a < 45) return "just now";
  if (a < 3600) return rtf.format(Math.round(s / 60), "minute");
  if (a < 86400) return rtf.format(Math.round(s / 3600), "hour");
  if (a < 86400 * 30) return rtf.format(Math.round(s / 86400), "day");
  return new Date(t).toLocaleDateString();
}

export function Empty({ title, children, action }: { title: string; children?: React.ReactNode; action?: React.ReactNode }) {
  return (
    <div className="flex flex-col items-center gap-2 rounded-box border border-dashed border-base-300 px-6 py-12 text-center" data-testid="empty-state">
      <h3 className="text-sm font-semibold">{title}</h3>
      {children && <p className="max-w-md text-xs text-base-content/70">{children}</p>}
      {action}
    </div>
  );
}

export function ErrorState({ message, onRetry }: { message: string; onRetry: () => void }) {
  return (
    <div role="alert" className="flex flex-col items-start gap-2 rounded-box border border-error/40 bg-error/5 px-4 py-3 text-sm" data-testid="error-state">
      <p className="font-medium text-error">Could not load</p>
      <p className="text-xs text-base-content/80">{message}</p>
      <button type="button" className="btn btn-xs" onClick={onRetry}>Retry</button>
    </div>
  );
}

export function Skeleton({ rows = 3 }: { rows?: number }) {
  return (
    <div className="flex flex-col gap-2" aria-busy="true" aria-label="Loading" data-testid="loading">
      {Array.from({ length: rows }, (_, i) => <div key={i} className="h-14 animate-pulse rounded-box bg-base-200" />)}
    </div>
  );
}

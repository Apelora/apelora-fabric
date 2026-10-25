import { useEffect, useId, useRef, useState, type ReactNode } from "react";

import type { Source } from "@apelora/api-client";

import { needsDatabase, type Load } from "../lib/api";

/** The API's messages are clauses; shown on their own they start with a capital. */
export const sentence = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);

export function StatusRow({ label, tone, value }: { label: string; tone: "ok" | "off" | "pending"; value: string }) {
  const dot = tone === "ok" ? "status-success" : tone === "off" ? "status-error" : "status-neutral";
  return (
    <div className="flex items-center justify-between gap-4 px-4 py-3">
      <span className="text-sm font-medium">{label}</span>
      <span className="flex items-center gap-2 text-sm text-base-content/70">
        <span className={`status ${dot}`} aria-hidden="true" />
        {value}
      </span>
    </div>
  );
}

/** Says where the data came from: a real Incus cluster, or labelled mock data. */
export function SourceNote({ source }: { source: Source }) {
  const time = new Date(source.observedAt).toLocaleTimeString();
  return source.provider === "mock" ? (
    <span className="badge badge-sm badge-ghost" data-testid="source-note">Mock data · not a real cluster</span>
  ) : (
    <span className="text-xs text-base-content/60" data-testid="source-note">From Incus · observed {time}</span>
  );
}

export type Tone = "ok" | "warn" | "error" | "info" | "neutral";
const DOT: Record<Tone, string> = { ok: "status-success", warn: "status-warning", error: "status-error", info: "status-info", neutral: "status-neutral" };

/** A short state in words with a coloured dot: the colour never carries the meaning alone. */
export function Badge({ tone = "neutral", children, title, testId }: { tone?: Tone; children: ReactNode; title?: string; testId?: string }) {
  return (
    <span className="badge badge-sm badge-ghost gap-1.5 whitespace-nowrap" title={title} data-testid={testId}>
      <span className={`status ${DOT[tone]}`} aria-hidden="true" />
      {children}
    </span>
  );
}

const NOTICE_BORDER: Record<Tone, string> = { ok: "border-success/40", warn: "border-warning/50", error: "border-error/40", info: "border-base-300", neutral: "border-base-300" };

/** An inline message. Errors and warnings are announced at once; the rest politely. */
export function Notice({ tone = "info", children, testId, title }: { tone?: Tone; children: ReactNode; testId?: string; title?: string }) {
  return (
    <div role={tone === "error" || tone === "warn" ? "alert" : "status"} className={`flex items-start gap-2 rounded-box border px-4 py-3 text-sm ${NOTICE_BORDER[tone]}`} data-testid={testId}>
      <span className={`status mt-1.5 shrink-0 ${DOT[tone]}`} aria-hidden="true" />
      <div className="min-w-0 flex flex-col gap-1">
        {title && <strong className="font-semibold">{title}</strong>}
        <div>{children}</div>
      </div>
    </div>
  );
}

export function ErrorPanel({ message, testId, onRetry }: { message: string; testId?: string; onRetry?: () => void }) {
  return (
    <div role="alert" className="flex flex-wrap items-center justify-between gap-3 rounded-box border border-error/40 px-4 py-3 text-sm">
      <span className="text-error" data-testid={testId}>{message}</span>
      {onRetry && <button type="button" className="btn btn-sm btn-ghost" onClick={onRetry}>Try again</button>}
    </div>
  );
}

/** Heading for a section; a view with a single section uses it as the page title (`level` 1). */
export function Heading({ level = 2, children }: { level?: 1 | 2; children: ReactNode }) {
  return level === 1 ? <h1 className="text-lg font-semibold">{children}</h1> : <h2 className="font-semibold">{children}</h2>;
}

/** A titled block whose content appears once its data has loaded. */
export function Section<T>({ title, level, state, testId, onRetry, children }: { title: string; level?: 1 | 2; state: Load<T>; testId: string; onRetry?: () => void; children: (data: T) => ReactNode }) {
  return (
    <section className="flex flex-col gap-2" aria-label={title} data-testid={testId}>
      {state.kind === "loading" && (
        <>
          <Heading level={level}>{title}</Heading>
          <p role="status" className="text-sm text-base-content/60" data-testid={`${testId}-loading`}>Loading…</p>
        </>
      )}
      {state.kind === "error" && (
        <>
          <Heading level={level}>{title}</Heading>
          {needsDatabase(state) ? (
            <Notice title="Needs a database" testId={`${testId}-needs-database`}>
              This control plane runs without a database, for a quick look only. To use this page, start PostgreSQL and run the control plane with <code>DATABASE_URL</code> set (see the settings reference).
            </Notice>
          ) : (
            <ErrorPanel message={state.message} testId={`${testId}-error`} onRetry={onRetry} />
          )}
        </>
      )}
      {state.kind === "ok" && children(state.data)}
    </section>
  );
}

export function SectionHeader({ title, level, count, source, children }: { title: string; level?: 1 | 2; count?: number; source?: Source; children?: ReactNode }) {
  return (
    <div className="flex flex-wrap items-center justify-between gap-3">
      <Heading level={level}>
        {title}
        {count !== undefined && <span className="font-normal text-base-content/60"> ({count})</span>}
      </Heading>
      <div className="flex items-center gap-3">
        {source && <SourceNote source={source} />}
        {children}
      </div>
    </div>
  );
}

/** `fixed` gives columns the widths set on their headers, so several tables one above the other line up. */
export function TableFrame({ children, fixed }: { children: ReactNode; fixed?: boolean }) {
  return <div className="overflow-x-auto rounded-box border border-base-300"><table className={`table table-sm ${fixed ? "table-fixed" : ""}`}>{children}</table></div>;
}

/**
 * Confirms an action that cannot be undone. Cancel is the prominent, focused choice; the
 * action itself is quieter. With `typeToConfirm`, the user must type that text first.
 */
export function ConfirmDialog({ open, title, body, action, dismiss = "Cancel", typeToConfirm, busy, danger = true, onConfirm, onCancel }: {
  open: boolean; title: string; body: ReactNode; action: string; dismiss?: string; typeToConfirm?: string; busy?: boolean; danger?: boolean;
  onConfirm: () => void; onCancel: () => void;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const [typed, setTyped] = useState("");
  const inputId = useId();
  useEffect(() => {
    const d = ref.current;
    if (!d) return;
    if (open && !d.open) {
      setTyped("");
      d.showModal();
      // The next step: typing the name when asked to, otherwise the safe choice. The modal's
      // opening transition keeps it `visibility: hidden` for a moment, and a hidden button
      // cannot take focus: try again on the next frames until it does.
      const target = typeToConfirm ? inputRef.current : cancelRef.current;
      let frames = 30;
      const focus = () => {
        target?.focus();
        if (target && document.activeElement !== target && d.open && frames-- > 0) requestAnimationFrame(focus);
      };
      focus();
    }
    if (!open && d.open) d.close();
  }, [open, typeToConfirm]);
  const ready = !typeToConfirm || typed === typeToConfirm;
  return (
    <dialog ref={ref} className="modal" aria-labelledby={`${inputId}-title`} onCancel={(e) => { e.preventDefault(); onCancel(); }}>
      <div className="modal-box flex flex-col gap-3" data-testid="confirm-dialog">
        <h3 id={`${inputId}-title`} className="font-semibold">{title}</h3>
        <div className="text-sm text-base-content/80">{body}</div>
        {typeToConfirm && (
          <label htmlFor={inputId} className="flex flex-col gap-1 text-sm">
            <span>Type <strong className="font-mono">{typeToConfirm}</strong> to confirm</span>
            <input ref={inputRef} id={inputId} className="input input-sm w-full font-mono" autoComplete="off" spellCheck={false} value={typed} onChange={(e) => setTyped(e.target.value)} data-testid="confirm-input" />
          </label>
        )}
        <div className="modal-action mt-2">
          <button ref={cancelRef} type="button" className="btn btn-sm btn-outline" onClick={onCancel}>{dismiss}</button>
          <button type="button" className={`btn btn-sm btn-ghost ${danger ? "text-error" : ""}`} disabled={!ready || busy} onClick={onConfirm} data-testid="confirm-action">{action}</button>
        </div>
      </div>
    </dialog>
  );
}

/** A modal form with its own title; closing it (Escape or Cancel) calls `onClose`. */
export function FormDialog({ open, title, children, onClose, testId, wide }: { open: boolean; title: string; children: ReactNode; onClose: () => void; testId?: string; wide?: boolean }) {
  const ref = useRef<HTMLDialogElement>(null);
  const titleId = useId();
  useEffect(() => {
    const d = ref.current;
    if (!d) return;
    if (open && !d.open) d.showModal();
    if (!open && d.open) d.close();
  }, [open]);
  return (
    <dialog ref={ref} className="modal" aria-labelledby={titleId} onCancel={(e) => { e.preventDefault(); onClose(); }}>
      <div className={`modal-box flex flex-col gap-4 ${wide ? "max-w-3xl" : ""}`} data-testid={testId}>
        <h3 id={titleId} className="font-semibold">{title}</h3>
        {children}
      </div>
    </dialog>
  );
}

import { useEffect, useId, type ReactNode } from "react";

/** Shared shell for dialogs: titled, closes on Escape or backdrop click. */
export function Modal({ testId, title, hint, onClose, children, footer, size = "max-w-2xl" }: { testId: string; title: string; hint?: string; onClose: () => void; children: ReactNode; footer?: ReactNode; size?: string }) {
  const id = useId();
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") { e.stopPropagation(); onClose(); } };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose]);
  return (
    <dialog className="modal modal-open" data-testid={testId} aria-labelledby={id}>
      <div className={`modal-box flex max-h-[90vh] w-full ${size} flex-col p-0`}>
        <div className="border-b border-base-300 px-5 py-3">
          <h3 id={id} className="text-base font-semibold">{title}</h3>
          {hint && <p className="text-xs text-base-content/60">{hint}</p>}
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto p-5">{children}</div>
        <div className="flex items-center justify-end gap-2 border-t border-base-300 px-5 py-3">
          {footer}
          <button type="button" className="btn btn-sm" data-testid={`${testId}-close`} onClick={onClose}>Close</button>
        </div>
      </div>
      <div className="modal-backdrop" onClick={onClose} />
    </dialog>
  );
}

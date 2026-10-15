import { useStudio, useStudioApi } from "../../features/editor/context";
import { IconX } from "../icons";

const kindClass = { info: "alert-info", success: "alert-success", warning: "alert-warning", error: "alert-error" } as const;

export function Toasts() {
  const api = useStudioApi();
  const toasts = useStudio((s) => s.toasts);
  const importErrors = useStudio((s) => s.importErrors);
  return (
    <>
      <div className="toast toast-bottom toast-center z-50" role="status" aria-live="polite" data-testid="toasts">
        {toasts.map((t) => (
          <div key={t.id} className={`alert alert-soft ${kindClass[t.kind]} max-w-md py-2 text-xs`} data-testid="toast">
            <span>{t.text}</span>
            <button className="btn btn-ghost btn-xs btn-square" aria-label="Dismiss" onClick={() => api.getState().dismissToast(t.id)}><IconX size={12} /></button>
          </div>
        ))}
      </div>
      <dialog className={`modal ${importErrors ? "modal-open" : ""}`} aria-labelledby="import-title" data-testid="import-dialog">
        <div className="modal-box max-w-lg">
          <h3 id="import-title" className="text-base font-semibold">Import failed</h3>
          <p className="py-2 text-sm text-base-content/70">The file was not loaded and your current assembly is unchanged.</p>
          <ul className="max-h-48 list-disc space-y-1 overflow-y-auto pl-5 text-xs" data-testid="import-errors">{importErrors?.map((e, i) => <li key={i}>{e}</li>)}</ul>
          <div className="modal-action"><button className="btn btn-sm" onClick={() => api.getState().setImportErrors(null)}>Close</button></div>
        </div>
        <div className="modal-backdrop" onClick={() => api.getState().setImportErrors(null)} />
      </dialog>
    </>
  );
}

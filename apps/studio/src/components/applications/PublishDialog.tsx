import { useEffect, useState } from "react";

import { useBackend } from "../../features/applications/backend";
import { AppError, type VersionInfo } from "../../features/applications/model";
import { onLinkClick, paths } from "../../features/router";

/** Freezes the saved draft as an immutable, numbered version. Deployments reference versions, never drafts. */
export function PublishDialog({ name, onClose, onDone }: { name: string; onClose: () => void; onDone?: (v: VersionInfo) => void }) {
  const backend = useBackend((s) => s.backend);
  const [label, setLabel] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<VersionInfo | null>(null);
  useEffect(() => {
    const on = (e: KeyboardEvent) => { if (e.key === "Escape") onClose(); };
    window.addEventListener("keydown", on);
    return () => window.removeEventListener("keydown", on);
  }, [onClose]);
  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      const v = await backend.publish(name, label.trim());
      setDone(v);
      onDone?.(v);
    } catch (err) {
      setError(err instanceof AppError ? err.message : err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };
  return (
    <dialog className="modal modal-open" aria-labelledby="publish-title" data-testid="publish-dialog">
      <form className="modal-box max-w-md" onSubmit={(e) => void submit(e)}>
        <h3 id="publish-title" className="text-base font-semibold">Publish a version of {name}</h3>
        {done ? (
          <div className="mt-3 flex flex-col gap-2 text-sm" data-testid="publish-done">
            <p>Published <strong>version {done.revision}</strong>{done.label ? ` (${done.label})` : ""}. It can no longer change; deploy it from Deployments or Versions.</p>
            <div className="modal-action">
              <a className="btn btn-sm" href={paths.application(name, "versions")} onClick={(e) => { onLinkClick(e); onClose(); }}>See versions</a>
              <button type="button" className="btn btn-sm btn-primary" onClick={onClose} data-testid="publish-close">Close</button>
            </div>
          </div>
        ) : (
          <>
            <p className="mt-2 text-sm text-base-content/80">The saved draft becomes an immutable version. Deployments always name a version, so later edits to the draft never change what is running.</p>
            <label className="mt-3 flex flex-col gap-1 text-xs">Label (optional)
              <input className="input input-sm" value={label} maxLength={80} placeholder="e.g. before the database upgrade" onChange={(e) => setLabel(e.target.value)} data-testid="publish-label" autoFocus />
            </label>
            {error && <p role="alert" className="mt-3 rounded-box border border-error/40 bg-error/5 px-3 py-2 text-xs text-error" data-testid="publish-error">{error}</p>}
            <div className="modal-action">
              <button type="button" className="btn btn-sm" onClick={onClose}>Cancel</button>
              <button type="submit" className="btn btn-sm btn-primary" disabled={busy} data-testid="publish-confirm">{busy ? "Publishing…" : "Publish"}</button>
            </div>
          </>
        )}
      </form>
      <div className="modal-backdrop" onClick={onClose} />
    </dialog>
  );
}

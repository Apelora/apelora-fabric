import { useEffect, useState } from "react";

import { useBackend } from "../../features/applications/backend";

/** Connects the Studio to a control plane with a token kept only for this browser tab. */
export function ConnectDialog({ onClose }: { onClose: () => void }) {
  const { connect, backend, who, disconnect } = useBackend();
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    const on = (e: KeyboardEvent) => { if (e.key === "Escape") onClose(); };
    window.addEventListener("keydown", on);
    return () => window.removeEventListener("keydown", on);
  }, [onClose]);
  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try { await connect(token); onClose(); } catch (err) { setError(err instanceof Error ? err.message : String(err)); setBusy(false); }
  };
  return (
    <dialog className="modal modal-open" aria-labelledby="connect-title" data-testid="connect-dialog">
      <form className="modal-box flex max-w-md flex-col gap-3" onSubmit={(e) => void submit(e)}>
        <h3 id="connect-title" className="text-base font-semibold">{backend.kind === "api" ? "Control plane" : "Connect to a control plane"}</h3>
        {backend.kind === "api" ? (
          <>
            <p className="text-sm text-base-content/80">Signed in as <strong>{who}</strong>. Applications, drafts and status come from the control plane.</p>
            <div className="modal-action mt-0">
              <button type="button" className="btn btn-sm" onClick={() => { disconnect(); onClose(); }} data-testid="disconnect">Sign out</button>
              <button type="button" className="btn btn-sm btn-primary" onClick={onClose}>Close</button>
            </div>
          </>
        ) : (
          <>
            <p className="text-sm text-base-content/80">Without a control plane, applications are kept in this browser and cannot be started or stopped. Paste an access token (an API token or an identity-provider access token). It stays in this tab only.</p>
            <label className="flex flex-col gap-1 text-sm">
              <span>Access token</span>
              <input type="password" className="input input-sm w-full font-mono" autoComplete="off" spellCheck={false} value={token} onChange={(e) => setToken(e.target.value)} data-testid="connect-token" autoFocus />
            </label>
            {error && <p role="alert" className="text-sm text-error" data-testid="connect-error">{error}</p>}
            <div className="modal-action mt-0">
              <button type="button" className="btn btn-sm" onClick={onClose}>Cancel</button>
              <button type="submit" className="btn btn-sm btn-primary" disabled={!token.trim() || busy} data-testid="connect-submit">{busy ? "Connecting…" : "Connect"}</button>
            </div>
          </>
        )}
      </form>
      <div className="modal-backdrop" onClick={onClose} />
    </dialog>
  );
}

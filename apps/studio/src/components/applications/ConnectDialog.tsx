import { useEffect, useRef, useState } from "react";

import { startOidcSignIn, type SignInOptions } from "@apelora/api-client";

import { useBackend } from "../../features/applications/backend";
import { client, messageOf } from "../../features/deploy/api";

type Provider = SignInOptions["providers"][number];

/**
 * Connects the Studio to a control plane: with the organisation's identity provider when the
 * control plane offers one, or with an access token. The token is kept only for this browser tab.
 */
export function ConnectDialog({ onClose }: { onClose: () => void }) {
  const { connect, backend, who, disconnect } = useBackend();
  const [options, setOptions] = useState<SignInOptions | null>(null);
  const [showToken, setShowToken] = useState(false);
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [opening, setOpening] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const tokenInput = useRef<HTMLInputElement>(null);

  useEffect(() => {
    const on = (e: KeyboardEvent) => { if (e.key === "Escape") onClose(); };
    window.addEventListener("keydown", on);
    return () => window.removeEventListener("keydown", on);
  }, [onClose]);
  useEffect(() => {
    if (backend.kind === "api") return;
    // Without the options a token still works: the control plane decides.
    client.get("sign_in_options").then(setOptions, () => setOptions({ providers: [], apiTokens: true, directories: [] }));
  }, [backend.kind]);
  useEffect(() => { if (showToken) tokenInput.current?.focus(); }, [showToken]);

  const providers = options?.providers ?? [];
  const tokenOpen = options !== null && (providers.length === 0 || showToken);

  const startOidc = async (p: Provider) => {
    setOpening(p.issuer);
    setError(null);
    const here = `${window.location.pathname}${window.location.search}${window.location.hash}`;
    try { await startOidcSignIn(p, `${window.location.origin}/`, here); } catch (e) { setError(messageOf(e)); setOpening(null); }
  };
  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!token.trim()) { setError("Paste an access token first."); tokenInput.current?.focus(); return; }
    setBusy(true);
    setError(null);
    try { await connect(token); onClose(); } catch (err) { setError(messageOf(err)); setBusy(false); }
  };

  return (
    <dialog className="modal modal-open" aria-labelledby="connect-title" data-testid="connect-dialog">
      <form className="modal-box flex max-w-md flex-col gap-3" noValidate onSubmit={(e) => void submit(e)}>
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
            <p className="text-sm text-base-content/80">Without a control plane, applications are kept in this browser and cannot be started or stopped.</p>
            {!options && <div className="skeleton h-8 w-48" data-testid="connect-loading" />}
            {providers.length > 0 && (
              <div className="flex flex-wrap gap-2" role="group" aria-label="Sign in with your organisation">
                {providers.map((p, i) => (
                  <button
                    key={p.issuer} type="button" data-testid="connect-oidc"
                    className={`btn btn-sm ${i === 0 ? "btn-primary" : "btn-outline"}`}
                    disabled={opening !== null} aria-busy={opening === p.issuer || undefined}
                    onClick={() => void startOidc(p)}
                  >
                    {opening === p.issuer && <span className="loading loading-spinner loading-xs" aria-hidden="true" />}
                    {providers.length === 1 ? `Continue with ${p.label}` : p.label}
                  </button>
                ))}
              </div>
            )}
            {providers.length > 0 && options?.apiTokens && !showToken && (
              <button type="button" className="link link-hover self-start text-sm text-base-content/70" onClick={() => setShowToken(true)} data-testid="connect-use-token">Use an access token instead</button>
            )}
            {tokenOpen && (
              <label className="flex flex-col gap-1 text-sm">
                <span>Access token</span>
                <input ref={tokenInput} type="password" className="input input-sm w-full font-mono" autoComplete="off" spellCheck={false} value={token} onChange={(e) => setToken(e.target.value)} data-testid="connect-token" autoFocus={providers.length === 0} />
                <span className="text-xs text-base-content/60">An API token or an identity-provider access token. It stays in this tab only.</span>
              </label>
            )}
            {error && <p role="alert" className="text-sm text-error" data-testid="connect-error">{error}</p>}
            <div className="modal-action mt-0">
              <button type="button" className="btn btn-sm" onClick={onClose}>Cancel</button>
              {tokenOpen && (
                <button type="submit" className={`btn btn-sm ${providers.length === 0 ? "btn-primary" : "btn-outline"}`} disabled={busy} data-testid="connect-submit">{busy ? "Connecting…" : "Connect"}</button>
              )}
            </div>
          </>
        )}
      </form>
      <div className="modal-backdrop" onClick={onClose} />
    </dialog>
  );
}

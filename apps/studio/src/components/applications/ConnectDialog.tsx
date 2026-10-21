import { useEffect, useRef, useState } from "react";

import { startOidcSignIn, type SignInOptions } from "@apelora/api-client";

import { useBackend } from "../../features/applications/backend";
import { client, messageOf } from "../../features/deploy/api";

type Provider = SignInOptions["providers"][number];

/**
 * Connects the Studio to a control plane: with the organisation's identity provider when the
 * control plane offers one, with a username and password when local accounts are on, or with an
 * access token. The token is kept only for this browser tab.
 */
export function ConnectDialog({ onClose }: { onClose: () => void }) {
  const { connect, backend, who, disconnect } = useBackend();
  const [options, setOptions] = useState<SignInOptions | null>(null);
  const [showToken, setShowToken] = useState(false);
  const [token, setToken] = useState("");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [opening, setOpening] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const tokenInput = useRef<HTMLInputElement>(null);
  const userInput = useRef<HTMLInputElement>(null);
  const passwordInput = useRef<HTMLInputElement>(null);

  useEffect(() => {
    const on = (e: KeyboardEvent) => { if (e.key === "Escape") onClose(); };
    window.addEventListener("keydown", on);
    return () => window.removeEventListener("keydown", on);
  }, [onClose]);
  useEffect(() => {
    if (backend.kind === "api") return;
    // Without the options a token still works: the control plane decides.
    client.get("sign_in_options").then(setOptions, () => setOptions({ providers: [], apiTokens: true, localAccounts: false, directories: [] }));
  }, [backend.kind]);
  useEffect(() => { if (showToken) tokenInput.current?.focus(); }, [showToken]);

  const providers = options?.providers ?? [];
  const local = !!options?.localAccounts;
  const tokenFirst = providers.length === 0 && !local;
  const tokenOpen = options !== null && (tokenFirst || showToken);

  const startOidc = async (p: Provider) => {
    setOpening(p.issuer);
    setError(null);
    const here = `${window.location.pathname}${window.location.search}${window.location.hash}`;
    try { await startOidcSignIn(p, `${window.location.origin}/`, here); } catch (e) { setError(messageOf(e)); setOpening(null); }
  };
  const submitToken = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!token.trim()) { setError("Paste an access token first."); tokenInput.current?.focus(); return; }
    setBusy(true);
    setError(null);
    try { await connect(token); onClose(); } catch (err) { setError(messageOf(err)); setBusy(false); }
  };
  const submitPassword = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!username.trim()) { setError("Enter your username."); userInput.current?.focus(); return; }
    if (!password) { setError("Enter your password."); passwordInput.current?.focus(); return; }
    setBusy(true);
    setError(null);
    try {
      const r = await client.request("sign_in_with_password", { body: { username: username.trim(), password } });
      await connect(r.token);
      onClose();
    } catch (err) {
      setError(messageOf(err));
      setPassword("");
      setBusy(false);
      passwordInput.current?.focus();
    }
  };

  return (
    <dialog className="modal modal-open" aria-labelledby="connect-title" data-testid="connect-dialog">
      <div className="modal-box flex max-w-md flex-col gap-3">
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
            {local && (
              <form className="flex flex-col gap-2" noValidate onSubmit={(e) => void submitPassword(e)} data-testid="connect-password-form">
                {providers.length > 0 && <p className="text-xs text-base-content/60">Or with your Apelora account:</p>}
                <label className="flex flex-col gap-1 text-sm">
                  <span>Username</span>
                  <input ref={userInput} type="text" name="username" className="input input-sm w-full" autoComplete="username" autoCapitalize="none" spellCheck={false} value={username} onChange={(e) => setUsername(e.target.value)} data-testid="connect-username" autoFocus={providers.length === 0} />
                </label>
                <label className="flex flex-col gap-1 text-sm">
                  <span>Password</span>
                  <input ref={passwordInput} type="password" name="password" className="input input-sm w-full" autoComplete="current-password" value={password} onChange={(e) => setPassword(e.target.value)} data-testid="connect-password" />
                </label>
                <button type="submit" className={`btn btn-sm self-end ${providers.length === 0 ? "btn-primary" : "btn-outline"}`} disabled={busy} aria-busy={busy || undefined} data-testid="connect-password-submit">
                  {busy && <span className="loading loading-spinner loading-xs" aria-hidden="true" />}
                  Sign in
                </button>
              </form>
            )}
            {!tokenFirst && options?.apiTokens && !showToken && (
              <button type="button" className="link link-hover self-start text-sm text-base-content/70" onClick={() => setShowToken(true)} data-testid="connect-use-token">Use an access token instead</button>
            )}
            {tokenOpen && (
              <form id="connect-token-form" className="flex flex-col gap-1 text-sm" noValidate onSubmit={(e) => void submitToken(e)}>
                <label className="flex flex-col gap-1">
                  <span>Access token</span>
                  <input ref={tokenInput} type="password" className="input input-sm w-full font-mono" autoComplete="off" spellCheck={false} value={token} onChange={(e) => setToken(e.target.value)} data-testid="connect-token" autoFocus={tokenFirst} />
                </label>
                <span className="text-xs text-base-content/60">An API token or an identity-provider access token. It stays in this tab only.</span>
              </form>
            )}
            {error && <p role="alert" className="text-sm text-error" data-testid="connect-error">{error}</p>}
            <div className="modal-action mt-0">
              <button type="button" className="btn btn-sm" onClick={onClose}>Cancel</button>
              {tokenOpen && (
                <button type="submit" form="connect-token-form" className={`btn btn-sm ${tokenFirst ? "btn-primary" : "btn-outline"}`} disabled={busy} data-testid="connect-submit">{busy ? "Connecting…" : "Connect"}</button>
              )}
            </div>
          </>
        )}
      </div>
      <div className="modal-backdrop" onClick={onClose} />
    </dialog>
  );
}

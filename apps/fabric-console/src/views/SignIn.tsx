import { useEffect, useState } from "react";

import { startOidcSignIn, type Health, type SignInOptions } from "@apelora/api-client";

import { client, messageOf } from "../lib/api";

/** Why the sign-in page is shown, in words for the person (not the API's message). */
export type SignInReason = "required" | "rejected" | "ended" | { error: string };

const reasonText = (r: SignInReason): string | null =>
  r === "required" ? null
    : r === "rejected" ? "That token was not accepted. It may have expired or been revoked."
    : r === "ended" ? "Your session ended. Sign in again."
    : r.error;

/**
 * The sign-in page: the organisation's identity provider first when the control plane offers
 * it, an API token otherwise or as the alternative. Nothing is stored beyond this browser tab.
 */
export function SignInPage({ reason, health, onToken }: { reason: SignInReason; health: Health | null; onToken: (token: string) => void }) {
  const [options, setOptions] = useState<SignInOptions | null>(null);
  const [optionsError, setOptionsError] = useState<string | null>(null);
  const [showToken, setShowToken] = useState(false);
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    client.get("sign_in_options").then(setOptions, (e: unknown) => setOptionsError(messageOf(e)));
  }, []);

  const providers = options?.providers ?? [];
  const oidc = providers[0] ?? null;
  const tokens = options ? options.apiTokens : true;
  const tokenFirst = !oidc;
  const message = error ?? reasonText(reason);

  const startOidc = async (provider: SignInOptions["providers"][number]) => {
    setBusy(true);
    setError(null);
    try {
      await startOidcSignIn(provider, `${window.location.origin}${window.location.pathname}`, window.location.hash);
    } catch (e) {
      setError(messageOf(e));
      setBusy(false);
    }
  };

  return (
    <main className="flex min-h-full items-center justify-center bg-base-100 px-4 py-12" data-testid="sign-in-page">
      <div className="flex w-full max-w-sm flex-col gap-6">
        <div className="flex flex-col items-center gap-3 text-center">
          <img src="/logo.svg" alt="Apelora" className="size-12" />
          <h1 className="text-xl font-semibold text-balance">Sign in to the Fabric Console</h1>
          <p className="text-sm text-base-content/70">Manage servers, applications and who may change them.</p>
        </div>

        <div className="flex flex-col gap-4 rounded-box border border-base-300 p-6">
          {message && <p role="alert" className="text-sm text-error" data-testid="sign-in-error">{message}</p>}
          {optionsError && <p className="text-sm text-base-content/70">The sign-in options could not be read ({optionsError}); an API token still works.</p>}

          {providers.map((p, i) => (
            <button key={p.issuer} type="button" className={`btn w-full ${i === 0 ? "btn-primary" : "btn-outline"}`} disabled={busy} onClick={() => void startOidc(p)} data-testid="sign-in-oidc">
              {busy ? "Opening sign-in…" : `Sign in with ${p.label}`}
            </button>
          ))}

          {oidc && tokens && !showToken && (
            <button type="button" className="btn btn-ghost btn-sm self-center" onClick={() => setShowToken(true)} data-testid="sign-in-use-token">Use an API token instead</button>
          )}

          {tokens && (tokenFirst || showToken) && (
            <form className="flex flex-col gap-3" data-testid="sign-in" onSubmit={(e) => { e.preventDefault(); if (token.trim()) onToken(token.trim()); }}>
              <label className="flex flex-col gap-1 text-sm">
                <span>API token</span>
                <input
                  type="password" autoComplete="off" spellCheck={false} autoFocus={tokenFirst}
                  className={`input input-sm w-full font-mono ${reason === "rejected" && !error ? "input-error" : ""}`}
                  aria-invalid={reason === "rejected" || undefined}
                  aria-describedby="token-hint"
                  value={token} onChange={(e) => setToken(e.target.value)} data-testid="token-input"
                />
                <span id="token-hint" className="text-xs text-base-content/60">An administrator creates one for you. It is kept only in this browser tab.</span>
              </label>
              <button type="submit" className={`btn btn-sm w-full ${tokenFirst ? "btn-primary" : "btn-outline"}`} disabled={!token.trim()}>Sign in</button>
            </form>
          )}

          {options && !oidc && !tokens && (
            <p className="text-sm text-base-content/70" data-testid="sign-in-none">No way to sign in is turned on for this control plane. An administrator sets up the identity provider.</p>
          )}
        </div>

        <p className="text-center text-xs text-base-content/60" data-testid="sign-in-status">
          {health ? `Apelora ${health.version}, ${health.provider === "mock" ? "showing mock data" : health.incus.connected ? "connected to Incus" : "Incus not connected"}` : "Checking the control plane…"}
        </p>
      </div>
    </main>
  );
}

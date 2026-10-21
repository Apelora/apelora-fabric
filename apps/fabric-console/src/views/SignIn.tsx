import { useEffect, useRef, useState } from "react";

import { startOidcSignIn, type Health, type SignInOptions } from "@apelora/api-client";

import { client, messageOf } from "../lib/api";
import { SignInArt } from "./SignInArt";

/** Why the sign-in page is shown, in words for the person (not the API's message). */
export type SignInReason = "required" | "rejected" | "ended" | { error: string };

type Provider = SignInOptions["providers"][number];

const reasonText = (r: SignInReason): string | null =>
  r === "required" ? null
    : r === "rejected" ? "That token was not accepted. It may have expired or been revoked."
    : r === "ended" ? "Your session ended. Sign in again."
    : r.error;

/**
 * The sign-in page: the organisation's identity providers first, an API token as the
 * alternative (or the only way when no provider is set up). Nothing is stored beyond this tab.
 */
export function SignInPage({ reason, health, onToken }: { reason: SignInReason; health: Health | null; onToken: (token: string) => void }) {
  const [options, setOptions] = useState<SignInOptions | null>(null);
  const [optionsError, setOptionsError] = useState<string | null>(null);
  const [showToken, setShowToken] = useState(false);
  const [token, setToken] = useState("");
  const [tokenMissing, setTokenMissing] = useState(false);
  const [opening, setOpening] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const tokenInput = useRef<HTMLInputElement>(null);

  useEffect(() => {
    client.get("sign_in_options").then(setOptions, (e: unknown) => setOptionsError(messageOf(e)));
  }, []);

  const loading = !options && !optionsError;
  const providers = options?.providers ?? [];
  // Without the options an API token is still worth offering: the server decides.
  const tokens = options ? options.apiTokens : !!optionsError;
  const tokenFirst = providers.length === 0;
  const tokenOpen = tokens && (tokenFirst || showToken);
  const message = error ?? reasonText(reason);

  useEffect(() => { if (showToken) tokenInput.current?.focus(); }, [showToken]);

  const startOidc = async (provider: Provider) => {
    setOpening(provider.issuer);
    setError(null);
    try {
      await startOidcSignIn(provider, `${window.location.origin}${window.location.pathname}`, window.location.hash);
    } catch (e) {
      setError(messageOf(e));
      setOpening(null);
    }
  };

  const submitToken = (e: React.FormEvent) => {
    e.preventDefault();
    const t = token.trim();
    if (!t) { setTokenMissing(true); tokenInput.current?.focus(); return; }
    onToken(t);
  };

  return (
    <main className="grid min-h-full bg-base-100 xl:grid-cols-2" data-testid="sign-in-page">
      <aside className="hidden p-3 xl:flex" aria-hidden="true">
        <div className="relative flex-1 overflow-hidden rounded-2xl border border-base-300 bg-[linear-gradient(135deg,#ECFDF5_0%,#EEF2FF_45%,#FDF2F8_75%,#F0F9FF_100%)]">
          <div className="pointer-events-none absolute inset-0 bg-[radial-gradient(45%_40%_at_30%_30%,rgba(20,184,166,0.12),transparent_70%),radial-gradient(40%_35%_at_75%_70%,rgba(139,92,246,0.10),transparent_70%)]" />
          <div className="pointer-events-none absolute inset-0"><SignInArt /></div>
        </div>
      </aside>

      <section className="flex min-h-full flex-col items-center px-6 pt-12 pb-6">
        <div className="flex w-full max-w-sm flex-1 flex-col justify-center">
         <div className="flex flex-col gap-6 rounded-2xl border border-base-300 bg-base-100 p-8 shadow-sm">
          <div className="flex flex-col items-center gap-2 text-center">
            <img src="/logo.svg" alt="Apelora" className="mb-2 size-11" />
            <h1 className="text-xl font-semibold text-balance">Sign in to the Fabric Console</h1>
            <p className="text-sm text-base-content/70">Manage servers, applications and who may change them.</p>
          </div>

          <div className="flex flex-col items-center gap-4 text-center" aria-busy={loading}>
            {message && <p role="alert" className="text-sm text-error" data-testid="sign-in-error">{message}</p>}

            {loading && (
              <div className="flex flex-col gap-3" data-testid="sign-in-loading">
                <span className="sr-only">Checking how you can sign in…</span>
                <div className="skeleton mx-auto h-9 w-52" />
                <div className="skeleton mx-auto h-4 w-36" />
              </div>
            )}

            {providers.length === 1 && (
              <button
                type="button" data-testid="sign-in-oidc" className="btn btn-primary btn-sm h-9 w-full px-4 sm:w-auto"
                disabled={opening !== null} aria-busy={opening !== null || undefined} onClick={() => void startOidc(providers[0]!)}
              >
                {opening && <span className="loading loading-spinner loading-xs" aria-hidden="true" />}
                Continue with {providers[0]!.label}
              </button>
            )}

            {providers.length > 1 && (
              <div className="flex flex-col items-center gap-2" role="group" aria-labelledby="sign-in-with">
                <p id="sign-in-with" className="text-sm font-medium">Sign in with your organisation</p>
                <div className="flex flex-wrap justify-center gap-2">
                  {providers.map((p, i) => (
                    <button
                      key={p.issuer} type="button" data-testid="sign-in-oidc"
                      className={`btn btn-sm h-9 px-4 max-sm:w-full ${i === 0 ? "btn-primary" : "btn-outline"}`}
                      disabled={opening !== null} aria-busy={opening === p.issuer || undefined}
                      aria-label={`Sign in with ${p.label}`}
                      onClick={() => void startOidc(p)}
                    >
                      {opening === p.issuer && <span className="loading loading-spinner loading-xs" aria-hidden="true" />}
                      {p.label}
                    </button>
                  ))}
                </div>
              </div>
            )}

            {!tokenFirst && tokens && !showToken && (
              <button type="button" className="link link-hover text-sm text-base-content/70" onClick={() => setShowToken(true)} data-testid="sign-in-use-token">
                Use an API token instead
              </button>
            )}

            {!tokenFirst && tokenOpen && (
              <div className="flex w-full items-center gap-3 text-xs text-base-content/60" aria-hidden="true">
                <span className="h-px flex-1 bg-base-300" />or<span className="h-px flex-1 bg-base-300" />
              </div>
            )}

            {tokenOpen && (
              <form className="flex w-full flex-col gap-2 text-left" data-testid="sign-in" noValidate onSubmit={submitToken}>
                <label className="text-sm font-medium" htmlFor="token-input">API token</label>
                <input
                  id="token-input" ref={tokenInput}
                  type="password" autoComplete="off" spellCheck={false} autoFocus={tokenFirst}
                  placeholder="apl_…"
                  className={`input w-full font-mono ${(reason === "rejected" && !error) || tokenMissing ? "input-error" : ""}`}
                  aria-invalid={reason === "rejected" || tokenMissing || undefined}
                  aria-describedby="token-hint"
                  value={token} onChange={(e) => { setToken(e.target.value); setTokenMissing(false); }} data-testid="token-input"
                />
                <span id="token-hint" className={`text-xs ${tokenMissing ? "text-error" : "text-base-content/60"}`}>
                  {tokenMissing ? "Paste the API token an administrator gave you." : "An administrator creates it for you. It stays in this browser tab only."}
                </span>
                <button type="submit" className={`btn btn-sm mt-2 h-9 w-full px-5 sm:w-auto sm:self-center ${tokenFirst ? "btn-primary" : "btn-outline"}`}>Sign in</button>
              </form>
            )}

            {optionsError && <p className="text-xs text-base-content/60">The sign-in options could not be read ({optionsError}); an API token may still work.</p>}

            {options && providers.length === 0 && !tokens && (
              <div className="flex flex-col gap-2 text-sm" data-testid="sign-in-none">
                <p className="font-medium">Sign-in is not set up yet</p>
                <p className="text-base-content/70">API tokens are turned off and no identity provider is configured. An administrator sets one with the <span className="font-mono text-xs">APELORA_OIDC_*</span> settings of the control plane, or turns API tokens back on.</p>
              </div>
            )}
          </div>
         </div>
        </div>

        <p className="mt-8 flex items-center justify-center gap-2 text-xs text-base-content/60" data-testid="sign-in-status">
            <span className={`status ${health ? (health.provider === "mock" || health.incus.connected ? "status-success" : "status-warning") : ""}`} aria-hidden="true" />
            {health ? `Apelora ${health.version}, ${health.provider === "mock" ? "showing mock data" : health.incus.connected ? "connected to Incus" : "Incus not connected"}` : "Checking the control plane…"}
        </p>
      </section>
    </main>
  );
}

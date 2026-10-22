import { useEffect, useRef, useState, type ReactNode } from "react";

import { ApiRequestError, startOidcSignIn, type Health, type SignInOptions } from "@apelora/api-client";

import { client, messageOf } from "../lib/api";
import { SignInArt } from "./SignInArt";

/** Why the sign-in page is shown, in words for the person (not the API's message). */
export type SignInReason = "required" | "rejected" | "ended" | "signed-out" | { error: string };

type Provider = SignInOptions["providers"][number];

const reasonText = (r: SignInReason): string | null =>
  r === "required" ? null
    : r === "rejected" ? "That token was not accepted. It may have expired or been revoked."
    : r === "ended" ? "Your session ended. Sign in again."
    : r === "signed-out" ? "You signed out."
    : r.error;

/** The password rule, as the control plane states it. */
export const PASSWORD_RULE = "Use 12 to 128 characters, not your username. A few unrelated words work well.";

/**
 * The frame every signed-out page shares: a decorative illustration on the left half of wide
 * screens, a centred card on the right, and the control plane's version and connection below it.
 */
export function SignInFrame({ health, testId, children }: { health: Health | null; testId: string; children: ReactNode }) {
  return (
    <main className="grid min-h-full bg-base-100 xl:grid-cols-2" data-testid={testId}>
      <aside className="hidden p-3 xl:flex" aria-hidden="true">
        <div className="relative flex-1 overflow-hidden rounded-2xl border border-base-300 bg-[linear-gradient(135deg,#ECFDF5_0%,#EEF2FF_45%,#FDF2F8_75%,#F0F9FF_100%)]">
          <div className="pointer-events-none absolute inset-0 bg-[radial-gradient(45%_40%_at_30%_30%,rgba(20,184,166,0.12),transparent_70%),radial-gradient(40%_35%_at_75%_70%,rgba(139,92,246,0.10),transparent_70%)]" />
          <div className="pointer-events-none absolute inset-0"><SignInArt /></div>
        </div>
      </aside>

      <section className="flex min-h-full flex-col items-center px-4 pt-12 pb-6 sm:px-6">
        <div className="flex w-full max-w-sm flex-1 flex-col justify-center">
          <div className="flex flex-col gap-6 rounded-2xl border border-base-300 bg-base-100 p-6 shadow-sm sm:p-8">
            {children}
          </div>
        </div>

        <p className="mt-8 flex items-center justify-center gap-2 text-xs text-base-content/60" data-testid="sign-in-status">
          <span className={`status ${health ? (health.provider === "mock" || health.incus.connected ? "status-success" : "status-warning") : ""}`} aria-hidden="true" />
          {health ? `Apelora ${health.version}, ${health.provider === "mock" ? "showing mock data" : health.incus.connected ? "connected to Incus" : "Incus not connected"}` : "Checking the control plane…"}
        </p>
        {/* The AGPL's network clause: everyone who uses Apelora, signed in or not, can find its source. */}
        <p className="mt-2 text-center text-xs"><a className="link link-hover text-base-content/60" href="/licenses" data-testid="sign-in-licences">Licences and source code</a></p>
      </section>
    </main>
  );
}

/** The card's logo, title and one line under it. */
export function CardTitle({ title, children }: { title: string; children?: ReactNode }) {
  return (
    <div className="flex flex-col items-center gap-4 text-center">
      <img src="/logo.svg" alt="Apelora" className="size-11" />
      <div className="flex flex-col items-center gap-2">
        <h1 className="text-xl font-semibold text-balance">{title}</h1>
        {children && <p className="text-sm text-base-content/70">{children}</p>}
      </div>
    </div>
  );
}

/** "or" between two ways in. */
const Divider = () => (
  <div className="flex w-full items-center gap-3 text-xs text-base-content/60" aria-hidden="true">
    <span className="h-px flex-1 bg-base-300" />or<span className="h-px flex-1 bg-base-300" />
  </div>
);

/** One way to sign in with a username and password: a local account, or one directory. */
type PasswordAccount = { id: string; label: string; directory?: string };

/**
 * Username and password for a local account or a directory. The answer is the same whether the
 * account is unknown, the password wrong or the account locked: the server says which it is to nobody.
 */
function PasswordForm({ primary, accounts, onToken }: { primary: boolean; accounts: PasswordAccount[]; onToken: (token: string) => void }) {
  const [account, setAccount] = useState(accounts[0]?.id ?? "");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [missing, setMissing] = useState<{ username?: boolean; password?: boolean }>({});
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const userInput = useRef<HTMLInputElement>(null);
  const passwordInput = useRef<HTMLInputElement>(null);
  const chosen = accounts.find((a) => a.id === account) ?? accounts[0];

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    const name = username.trim();
    const gaps = { username: !name, password: !password };
    setMissing(gaps);
    if (gaps.username) { userInput.current?.focus(); return; }
    if (gaps.password) { passwordInput.current?.focus(); return; }
    setBusy(true);
    setError(null);
    try {
      const r = await client.request("sign_in_with_password", {
        body: { username: name, password, ...(chosen?.directory ? { directory: chosen.directory } : {}) },
      });
      onToken(r.token);
    } catch (err) {
      setError(err instanceof ApiRequestError && err.code === "network_error" ? "The control plane could not be reached. Check your connection and try again." : messageOf(err));
      setPassword("");
      setBusy(false);
      passwordInput.current?.focus();
    }
  };

  return (
    <form className="flex w-full flex-col gap-3 text-left" data-testid="password-sign-in" noValidate onSubmit={(e) => void submit(e)}>
      {accounts.length > 1 && (
        <div className="flex flex-col gap-1">
          <label className="text-sm font-medium" htmlFor="sign-in-account">Sign in with</label>
          <select
            id="sign-in-account" className="select w-full" value={chosen?.id ?? ""}
            onChange={(e) => { setAccount(e.target.value); setError(null); }} data-testid="sign-in-account"
          >
            {accounts.map((a) => <option key={a.id} value={a.id}>{a.label}</option>)}
          </select>
        </div>
      )}
      {accounts.length === 1 && chosen?.directory && (
        <p className="text-xs text-base-content/60">Username and password from {chosen.label}.</p>
      )}
      <div className="flex flex-col gap-1">
        <label className="text-sm font-medium" htmlFor="username-input">Username</label>
        <input
          id="username-input" ref={userInput} name="username" type="text"
          autoComplete="username" autoCapitalize="none" spellCheck={false} autoFocus={primary}
          className={`input w-full ${missing.username ? "input-error" : ""}`}
          aria-invalid={missing.username || undefined} aria-describedby={missing.username ? "username-error" : undefined}
          value={username} onChange={(e) => { setUsername(e.target.value); setMissing((m) => ({ ...m, username: false })); }}
          data-testid="username-input"
        />
        {missing.username && <span id="username-error" className="text-xs text-error">Enter your username.</span>}
      </div>
      <div className="flex flex-col gap-1">
        <label className="text-sm font-medium" htmlFor="password-input">Password</label>
        <input
          id="password-input" ref={passwordInput} name="password" type="password" autoComplete="current-password"
          className={`input w-full ${missing.password || error ? "input-error" : ""}`}
          aria-invalid={missing.password || !!error || undefined}
          aria-describedby={missing.password ? "password-error" : error ? "password-sign-in-error" : undefined}
          value={password} onChange={(e) => { setPassword(e.target.value); setMissing((m) => ({ ...m, password: false })); }}
          data-testid="password-input"
        />
        {missing.password && <span id="password-error" className="text-xs text-error">Enter your password.</span>}
      </div>
      {error && <p id="password-sign-in-error" role="alert" className="text-sm text-error" data-testid="password-sign-in-error">{error}</p>}
      <button type="submit" className={`btn btn-sm h-9 w-full ${primary ? "btn-primary" : "btn-outline"}`} disabled={busy} aria-busy={busy || undefined} data-testid="password-submit">
        {busy && <span className="loading loading-spinner loading-xs" aria-hidden="true" />}
        Sign in
      </button>
      {!chosen?.directory && <p className="text-center text-xs text-base-content/60">Forgot your password? Ask an administrator.</p>}
    </form>
  );
}

/**
 * The sign-in page: the organisation's identity providers first, then a username and password
 * for a local account or a directory, and an API token as the alternative (or the only way when
 * nothing else is set up). Nothing is stored beyond this tab.
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
  const local = !!options?.localAccounts;
  const accounts: PasswordAccount[] = [
    ...(local ? [{ id: "local", label: "Apelora account" }] : []),
    ...(options?.directories ?? []).map((d) => ({ id: d.id, label: d.name, directory: d.id })),
  ];
  // Without the options an API token is still worth offering: the server decides.
  const tokens = options ? options.apiTokens : !!optionsError;
  const tokenFirst = providers.length === 0 && accounts.length === 0;
  const tokenOpen = tokens && (tokenFirst || showToken);
  const message = error ?? reasonText(reason);
  const calm = !error && reason === "signed-out";

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
    <SignInFrame health={health} testId="sign-in-page">
      <CardTitle title="Sign in to the Fabric Console">Manage servers, applications and who may change them.</CardTitle>

      <div className="flex flex-col items-center gap-4 text-center" aria-busy={loading}>
        {message && (
          <p role={calm ? "status" : "alert"} className={`text-sm ${calm ? "text-base-content/70" : "text-error"}`} data-testid="sign-in-error">{message}</p>
        )}

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

        {accounts.length > 0 && providers.length > 0 && <Divider />}
        {accounts.length > 0 && <PasswordForm primary={providers.length === 0} accounts={accounts} onToken={onToken} />}

        {!tokenFirst && tokens && !showToken && (
          <button type="button" className="link link-hover -my-3 py-3 text-sm text-base-content/70" onClick={() => setShowToken(true)} data-testid="sign-in-use-token">
            Use an API token instead
          </button>
        )}

        {!tokenFirst && tokenOpen && <Divider />}

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

        {options && providers.length === 0 && !tokens && accounts.length === 0 && (
          <div className="flex flex-col gap-2 text-sm" data-testid="sign-in-none">
            <p className="font-medium">Sign-in is not set up yet</p>
            <p className="text-base-content/70">API tokens and local accounts are turned off and no identity provider is configured. An administrator sets one with the <span className="font-mono text-xs">APELORA_OIDC_*</span> settings of the control plane, or turns API tokens back on.</p>
          </div>
        )}
      </div>
    </SignInFrame>
  );
}

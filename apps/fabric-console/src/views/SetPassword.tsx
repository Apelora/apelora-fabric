import { useRef, useState } from "react";

import { ApiRequestError, type Health } from "@apelora/api-client";

import { client, messageOf } from "../lib/api";
import { CardTitle, PASSWORD_RULE, SignInFrame } from "./SignIn";

/** The setup token from `#/set-password?token=…`. It stays in the fragment, so no server log sees it. */
export const setupTokenFromHash = (hash: string): string | null => {
  const m = /^#\/?set-password(?:\?(.*))?$/.exec(hash);
  if (!m) return null;
  return new URLSearchParams(m[1] ?? "").get("token") ?? "";
};

const length = (s: string) => Array.from(s).length;

/**
 * Where a person with a new local account (or a reset one) chooses their password, from the
 * one-time link an administrator sent them. The link works once, for 24 hours.
 */
export function SetPasswordPage({ token, health, onDone }: { token: string; health: Health | null; onDone: () => void }) {
  const [password, setPassword] = useState("");
  const [again, setAgain] = useState("");
  const [fieldError, setFieldError] = useState<{ password?: string; again?: string }>({});
  const [linkError, setLinkError] = useState<string | null>(token ? null : "This link has no setup token. Open the full link you were sent, or ask an administrator for a new one.");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState(false);
  const first = useRef<HTMLInputElement>(null);
  const second = useRef<HTMLInputElement>(null);

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    setError(null);
    const n = length(password);
    if (n < 12 || n > 128) { setFieldError({ password: n < 12 ? `This has ${n} ${n === 1 ? "character" : "characters"}. Use at least 12.` : "Use at most 128 characters." }); first.current?.focus(); return; }
    if (password !== again) { setFieldError({ again: "The two passwords are different. Type the same password twice." }); second.current?.focus(); return; }
    setFieldError({});
    setBusy(true);
    try {
      await client.request("complete_password_setup", { body: { token, password } });
      setDone(true);
    } catch (err) {
      if (err instanceof ApiRequestError && err.code === "setup_invalid") setLinkError(err.message);
      else if (err instanceof ApiRequestError && err.code === "weak_password") { setFieldError({ password: err.message }); first.current?.focus(); }
      else setError(err instanceof ApiRequestError && err.code === "network_error" ? "The control plane could not be reached. Check your connection and try again." : messageOf(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <SignInFrame health={health} testId="set-password-page">
      {done ? (
        <>
          <CardTitle title="Your password is set">Sign in with your username and the password you just chose.</CardTitle>
          <div role="status" className="sr-only">Your password is set.</div>
          <button type="button" className="btn btn-primary btn-sm h-9 w-full" onClick={onDone} data-testid="set-password-sign-in">Go to sign-in</button>
        </>
      ) : linkError ? (
        <>
          <CardTitle title="This setup link does not work" />
          <p role="alert" className="text-center text-sm text-base-content/80" data-testid="set-password-link-error">{linkError}</p>
          <button type="button" className="btn btn-outline btn-sm h-9 w-full" onClick={onDone}>Go to sign-in</button>
        </>
      ) : (
        <>
          <CardTitle title="Choose your password">You sign in to Apelora with it and your username.</CardTitle>
          <form className="flex flex-col gap-4" noValidate onSubmit={(e) => void submit(e)} data-testid="set-password-form">
            <div className="flex flex-col gap-1">
              <label className="text-sm font-medium" htmlFor="new-password">New password</label>
              <input
                id="new-password" ref={first} type="password" autoComplete="new-password" autoFocus
                className={`input w-full ${fieldError.password ? "input-error" : ""}`}
                aria-invalid={!!fieldError.password || undefined} aria-describedby="new-password-help"
                value={password} onChange={(e) => { setPassword(e.target.value); setFieldError((f) => ({ ...f, password: undefined })); }}
                data-testid="new-password"
              />
              <span id="new-password-help" className={`text-xs ${fieldError.password ? "text-error" : "text-base-content/60"}`} data-testid="new-password-help">
                {fieldError.password ?? PASSWORD_RULE}
              </span>
            </div>
            <div className="flex flex-col gap-1">
              <label className="text-sm font-medium" htmlFor="new-password-again">Type it again</label>
              <input
                id="new-password-again" ref={second} type="password" autoComplete="new-password"
                className={`input w-full ${fieldError.again ? "input-error" : ""}`}
                aria-invalid={!!fieldError.again || undefined} aria-describedby={fieldError.again ? "new-password-again-error" : undefined}
                value={again} onChange={(e) => { setAgain(e.target.value); setFieldError((f) => ({ ...f, again: undefined })); }}
                data-testid="new-password-again"
              />
              {fieldError.again && <span id="new-password-again-error" className="text-xs text-error" data-testid="new-password-again-error">{fieldError.again}</span>}
            </div>
            {error && <p role="alert" className="text-sm text-error" data-testid="set-password-error">{error}</p>}
            <button type="submit" className="btn btn-primary btn-sm h-9 w-full" disabled={busy} aria-busy={busy || undefined} data-testid="set-password-submit">
              {busy && <span className="loading loading-spinner loading-xs" aria-hidden="true" />}
              Set password
            </button>
          </form>
        </>
      )}
    </SignInFrame>
  );
}

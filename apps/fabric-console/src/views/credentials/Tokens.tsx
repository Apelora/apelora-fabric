// SPDX-License-Identifier: AGPL-3.0-only
import { useState, type ReactNode } from "react";

import type { PersonalToken, PersonalTokenList, WhoAmI } from "@apelora/api-client";

import { SecretOnceDialog, type OneTimeSecret } from "../../components/SecretOnce";
import { Badge, ConfirmDialog, ErrorPanel, FormDialog, Heading, Notice, Section, SectionHeader, TableFrame, type Tone } from "../../components/ui";
import { client, useLoad } from "../../lib/api";
import { fmtDate, fmtTime, useRun } from "../../lib/identity";
import { credentialOf } from "./pages";

const STATE: Record<string, { tone: Tone; label: string }> = {
  active: { tone: "ok", label: "Active" },
  rotating: { tone: "warn", label: "Replaced" },
  expired: { tone: "neutral", label: "Expired" },
  revoked: { tone: "neutral", label: "Revoked" },
};

const LIVE = (t: PersonalToken) => t.state === "active" || t.state === "rotating";

/** When a token stops working, in words. */
function ends(t: PersonalToken): string {
  if (t.state === "revoked") return `Revoked ${fmtDate(t.revokedAt)}${t.revokedBy === "disabled" ? " (account disabled)" : ""}`;
  if (t.state === "expired") return `Expired ${fmtDate(t.expiresAt)}`;
  if (t.state === "rotating") return `Works until ${fmtTime(t.expiresAt)}`;
  return fmtDate(t.expiresAt);
}

/** Personal tokens as a table; `owner` adds the person column (administrators). */
export function TokenTable({ items, owner, actions }: { items: PersonalToken[]; owner?: boolean; actions: (t: PersonalToken) => ReactNode }) {
  return (
    <TableFrame>
      <thead>
        <tr>
          {owner && <th>Person</th>}
          <th>Name</th>
          <th>State</th>
          <th className="max-sm:hidden">Last used</th>
          <th className="max-md:hidden">Expires</th>
          <th><span className="sr-only">Actions</span></th>
        </tr>
      </thead>
      <tbody>
        {items.map((t) => {
          const s = STATE[t.state] ?? { tone: "neutral" as Tone, label: t.state };
          return (
            <tr key={t.id} data-testid="personal-token-row" data-state={t.state} className={LIVE(t) ? undefined : "text-base-content/60"}>
              {owner && <td className="align-top font-medium">{t.owner}</td>}
              <td className="align-top">
                <span className={owner ? undefined : "font-medium"}>{t.name}</span>
                {t.current && <span className="block text-xs text-base-content/60">Used for this page</span>}
                <span className="block text-xs text-base-content/60 sm:hidden">Last used {fmtTime(t.lastUsedAt)}</span>
              </td>
              <td className="align-top">
                <Badge tone={s.tone}>{s.label}</Badge>
                <span className="block text-xs text-base-content/60 md:hidden">{ends(t)}</span>
              </td>
              <td className="align-top whitespace-nowrap max-sm:hidden">{t.lastUsedAt ? fmtTime(t.lastUsedAt) : "Never"}</td>
              <td className="align-top max-md:hidden">{ends(t)}</td>
              <td className="align-top"><div className="flex flex-wrap justify-end gap-1">{actions(t)}</div></td>
            </tr>
          );
        })}
      </tbody>
    </TableFrame>
  );
}

const DAYS = [7, 30, 90, 365];

function CreateDialog({ open, onClose, onCreated }: { open: boolean; onClose: () => void; onCreated: (secret: OneTimeSecret) => void }) {
  const [name, setName] = useState("");
  const [days, setDays] = useState(30);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const submit = async () => {
    setBusy(true);
    setError(null);
    try {
      const r = await client.request("create_my_token", { body: { name: name.trim(), expiresInDays: days } });
      setName("");
      onClose();
      onCreated({ title: `Token “${r.info.name}” created`, what: "token", value: r.token, note: <>Copy it now and keep it in your secret store. It works until {fmtDate(r.info.expiresAt)}; Apelora cannot show it again.</> });
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <FormDialog open={open} title="Create a personal token" onClose={onClose} testId="personal-token-dialog">
      {open && (
        <form className="flex flex-col gap-3" onSubmit={(e) => { e.preventDefault(); void submit(); }}>
          <label className="flex flex-col gap-1 text-sm">
            <span>Name</span>
            <input className="input input-sm w-full" value={name} onChange={(e) => setName(e.target.value)} placeholder="Backup script on my laptop" maxLength={64} autoComplete="off" autoFocus data-testid="personal-token-name" />
            <span className="text-xs text-base-content/60">What will use it, so you recognise it later.</span>
          </label>
          <label className="flex flex-col gap-1 text-sm">
            <span>Expires after</span>
            <select className="select select-sm w-full sm:w-48" value={days} onChange={(e) => setDays(Number(e.target.value))} data-testid="personal-token-days">
              {DAYS.map((d) => <option key={d} value={d}>{d} days</option>)}
            </select>
          </label>
          <p className="text-xs text-base-content/60">It can do what you can, while you can. It cannot open terminals, create tokens or approve requests.</p>
          {error && <ErrorPanel message={error} testId="personal-token-error" />}
          <div className="modal-action mt-0">
            <button type="button" className="btn btn-sm btn-outline" onClick={onClose}>Cancel</button>
            <button type="submit" className="btn btn-sm btn-primary" disabled={busy || !name.trim()} data-testid="personal-token-submit">{busy ? "Creating…" : "Create token"}</button>
          </div>
        </form>
      )}
    </FormDialog>
  );
}

const OVERLAPS = [
  { minutes: 0, label: "Stop the old token now" },
  { minutes: 10, label: "Keep the old token for 10 minutes" },
  { minutes: 60, label: "Keep the old token for 1 hour" },
  { minutes: 1440, label: "Keep the old token for 1 day" },
];

function RotateDialog({ token, onClose, onRotated }: { token: PersonalToken | null; onClose: () => void; onRotated: (secret: OneTimeSecret) => void }) {
  const [overlap, setOverlap] = useState(60);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const submit = async (t: PersonalToken) => {
    setBusy(true);
    setError(null);
    try {
      const r = await client.request("rotate_my_token", { params: { id: t.id }, body: { overlapMinutes: overlap } });
      onClose();
      onRotated({
        title: `New token for “${r.info.name}”`,
        what: "token",
        value: r.token,
        note: <>Put it where the old one was used. {overlap === 0 ? "The old token has stopped working." : <>The old token works until {fmtTime(r.previous.expiresAt)}.</>} Apelora cannot show this one again.</>,
      });
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <FormDialog open={!!token} title={`Replace “${token?.name ?? ""}” with a new token`} onClose={onClose} testId="rotate-dialog">
      {token && (
        <form className="flex flex-col gap-3" onSubmit={(e) => { e.preventDefault(); void submit(token); }}>
          <p className="text-sm text-base-content/80">The new token keeps the name and lifetime. Choose how long the old one keeps working while your script switches over.</p>
          <label className="flex flex-col gap-1 text-sm">
            <span>Old token</span>
            <select className="select select-sm w-full" value={overlap} onChange={(e) => setOverlap(Number(e.target.value))} autoFocus data-testid="rotate-overlap">
              {OVERLAPS.map((o) => <option key={o.minutes} value={o.minutes}>{o.label}</option>)}
            </select>
          </label>
          {error && <ErrorPanel message={error} testId="rotate-error" />}
          <div className="modal-action mt-0">
            <button type="button" className="btn btn-sm btn-outline" onClick={onClose}>Cancel</button>
            <button type="submit" className="btn btn-sm btn-primary" disabled={busy} data-testid="rotate-submit">{busy ? "Replacing…" : "Replace token"}</button>
          </div>
        </form>
      )}
    </FormDialog>
  );
}

/** The signed-in person's own tokens (`#/account/tokens`). */
export function YourTokensView({ me, onAuth }: { me: WhoAmI; onAuth: (m: string) => void }) {
  const person = me.kind === "user";
  const [list, reload] = useLoad<PersonalTokenList | null>(() => (person ? client.get("my_tokens") : Promise.resolve(null)), onAuth);
  const [creating, setCreating] = useState(false);
  const [rotating, setRotating] = useState<PersonalToken | null>(null);
  const [revoking, setRevoking] = useState<PersonalToken | null>(null);
  const [secret, setSecret] = useState<OneTimeSecret | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const { busy, error, run } = useRun(onAuth);
  // A personal token cannot make another (it would outlive its own expiry).
  const viaToken = credentialOf(me) === "personal-token";
  const why = viaToken ? "Sign in to create or replace tokens: a personal token cannot make another." : undefined;
  const shown = (s: OneTimeSecret) => { setSecret(s); reload(); };
  const full = list.kind === "ok" && !!list.data && list.data.items.filter(LIVE).length >= list.data.limit;

  const revoke = (t: PersonalToken) => {
    setRevoking(null);
    setNotice(null);
    void run(async () => {
      await client.request("revoke_my_token", { params: { id: t.id } });
      setNotice(`“${t.name}” no longer works.`);
      reload();
    });
  };

  return (
    <div className="flex flex-col gap-6">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="flex flex-col gap-2">
          <Heading level={1}>Your tokens</Heading>
          <p className="max-w-2xl text-sm text-base-content/70">For your scripts. A token acts as you, with your access at the time.</p>
        </div>
        {person && (
          <button type="button" className="btn btn-sm btn-primary" disabled={!!why || full} title={why} onClick={() => setCreating(true)} data-testid="personal-token-open">Create token</button>
        )}
      </div>
      {!person ? (
        <Notice testId="personal-tokens-people-only">Personal tokens belong to people. You are signed in as {me.kind === "service" ? "an automation identity" : "an API token"}; sign in as yourself to make one.</Notice>
      ) : (
        <Section title="Tokens" state={list} testId="personal-tokens" onRetry={reload}>
          {(l) => {
            const items = l?.items ?? [];
            return (
              <>
                {why && <p className="text-sm text-base-content/70" data-testid="personal-token-why">{why}</p>}
                {full && <p className="text-sm text-base-content/70" data-testid="personal-token-full">You have {l?.limit} working tokens, the most allowed. Revoke one to create another.</p>}
                {notice && <Notice tone="ok" testId="personal-token-notice">{notice}</Notice>}
                {error && <ErrorPanel message={error} testId="personal-token-action-error" />}
                {items.length === 0 ? (
                  <p className="text-sm text-base-content/70" data-testid="personal-tokens-empty">You have no tokens yet. Create one for a script that works as you.</p>
                ) : (
                  <TokenTable items={items} actions={(t) => LIVE(t) && (
                    <>
                      {t.state === "active" && <button type="button" className="btn btn-xs btn-ghost" disabled={!!why || busy} title={why} aria-label={`Replace token ${t.name}`} onClick={() => setRotating(t)} data-testid="personal-token-rotate">Replace…</button>}
                      <button type="button" className="btn btn-xs btn-ghost text-error" disabled={busy} aria-label={`Revoke token ${t.name}`} onClick={() => setRevoking(t)} data-testid="personal-token-revoke">Revoke</button>
                    </>
                  )} />
                )}
              </>
            );
          }}
        </Section>
      )}
      {creating && <CreateDialog open={creating} onClose={() => setCreating(false)} onCreated={shown} />}
      {rotating && <RotateDialog token={rotating} onClose={() => setRotating(null)} onRotated={shown} />}
      <SecretOnceDialog secret={secret} onClose={() => setSecret(null)} />
      <ConfirmDialog
        open={!!revoking}
        title={`Revoke “${revoking?.name ?? ""}”?`}
        body={<p>Anything using it is refused from now on{revoking?.current ? ", including this page" : ""}. This cannot be undone.</p>}
        action="Revoke token"
        busy={busy}
        onCancel={() => setRevoking(null)}
        onConfirm={() => { if (revoking) revoke(revoking); }}
      />
    </div>
  );
}

/** Everyone's personal tokens (Identity & Access → Sign-ins & tokens; `token.manage`). */
export function PeopleTokensSection({ onAuth }: { onAuth: (m: string) => void }) {
  const [list, reload] = useLoad<PersonalTokenList>(() => client.get("personal_tokens"), onAuth);
  const [revoking, setRevoking] = useState<PersonalToken | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const { busy, error, run } = useRun(onAuth);
  const revoke = (t: PersonalToken) => {
    setRevoking(null);
    setNotice(null);
    void run(async () => {
      await client.request("revoke_personal_token", { params: { id: t.id } });
      setNotice(`${t.owner}’s token “${t.name}” no longer works.`);
      reload();
    });
  };
  return (
    <Section title="Personal tokens" state={list} testId="people-tokens" onRetry={reload}>
      {(l) => (
        <>
          <SectionHeader title="Personal tokens" count={l.items.filter(LIVE).length}>
            <button type="button" className="btn btn-sm btn-ghost" onClick={reload}>Refresh</button>
          </SectionHeader>
          <p className="text-sm text-base-content/70">Each acts as its person. Revoke one that may have leaked.</p>
          {notice && <Notice tone="ok" testId="people-tokens-notice">{notice}</Notice>}
          {error && <ErrorPanel message={error} testId="people-tokens-error" />}
          {l.items.length === 0 ? (
            <p className="text-sm text-base-content/70" data-testid="people-tokens-empty">Nobody has a personal token.</p>
          ) : (
            <TokenTable owner items={l.items} actions={(t) => LIVE(t) && (
              <button type="button" className="btn btn-xs btn-ghost text-error" disabled={busy} aria-label={`Revoke ${t.owner}’s token ${t.name}`} onClick={() => setRevoking(t)} data-testid="people-token-revoke">Revoke</button>
            )} />
          )}
          <ConfirmDialog
            open={!!revoking}
            title={`Revoke ${revoking?.owner ?? ""}’s token “${revoking?.name ?? ""}”?`}
            body={<p>Their scripts using it are refused from now on. They can make a new one while they may sign in.</p>}
            action="Revoke token"
            busy={busy}
            onCancel={() => setRevoking(null)}
            onConfirm={() => { if (revoking) revoke(revoking); }}
          />
        </>
      )}
    </Section>
  );
}

// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";

import type { ActiveSignIn, ActiveSignInList, WhoAmI } from "@apelora/api-client";

import { Badge, ConfirmDialog, ErrorPanel, Heading, Notice, Section, SectionHeader, TableFrame } from "../../components/ui";
import { client, useLoad } from "../../lib/api";
import { fmtTime, useRun } from "../../lib/identity";
import { credentialOf } from "./pages";

const METHOD: Record<string, string> = { password: "Username and password", directory: "Directory" };

function SignInTable({ items, person, onEnd, busy }: { items: ActiveSignIn[]; person?: boolean; onEnd: (s: ActiveSignIn) => void; busy: boolean }) {
  return (
    <TableFrame>
      <thead>
        <tr>
          {person && <th>Person</th>}
          <th>How</th>
          <th className="max-sm:hidden">Signed in</th>
          <th className="max-md:hidden">Last used</th>
          <th>Ends</th>
          <th><span className="sr-only">Actions</span></th>
        </tr>
      </thead>
      <tbody>
        {items.map((s) => (
          <tr key={s.id} data-testid="sign-in-row">
            {person && <td className="align-top font-medium">{s.person}</td>}
            <td className="align-top">
              {METHOD[s.method] ?? s.method}
              {s.current && <span className="block"><Badge tone="info">This browser</Badge></span>}
              <span className="block text-xs text-base-content/60 sm:hidden">Signed in {fmtTime(s.createdAt)}</span>
            </td>
            <td className="align-top whitespace-nowrap max-sm:hidden">{fmtTime(s.createdAt)}</td>
            <td className="align-top whitespace-nowrap max-md:hidden">{fmtTime(s.lastUsedAt)}</td>
            <td className="align-top">
              {fmtTime(s.endsAt)}
              <span className="block text-xs text-base-content/60">{s.endsAt === s.expiresAt ? "time limit" : "unless used before"}</span>
            </td>
            <td className="align-top text-right">
              {s.current && !person ? (
                <span className="text-xs text-base-content/60">Sign out from the menu</span>
              ) : (
                <button type="button" className="btn btn-xs btn-ghost text-error" disabled={busy} aria-label={`Sign out ${person ? `${s.person}, ` : ""}signed in ${fmtTime(s.createdAt)}`} onClick={() => onEnd(s)} data-testid="sign-in-end">Sign out</button>
              )}
            </td>
          </tr>
        ))}
      </tbody>
    </TableFrame>
  );
}

/** The signed-in person's own sign-ins (`#/account/sign-ins`). */
export function YourSignInsView({ me, onAuth }: { me: WhoAmI; onAuth: (m: string) => void }) {
  const person = me.kind === "user";
  const [list, reload] = useLoad<ActiveSignInList | null>(() => (person ? client.get("my_sign_ins") : Promise.resolve(null)), onAuth);
  const [ending, setEnding] = useState<ActiveSignIn | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const { busy, error, run } = useRun(onAuth);
  const credential = credentialOf(me);
  const end = (s: ActiveSignIn) => {
    setEnding(null);
    setNotice(null);
    void run(async () => {
      await client.request("end_my_sign_in", { params: { id: s.id } });
      setNotice("That browser is signed out.");
      reload();
    });
  };
  return (
    <div className="flex flex-col gap-6">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="flex flex-col gap-2">
          <Heading level={1}>Your sign-ins</Heading>
          <p className="max-w-2xl text-sm text-base-content/70">Browsers signed in with your password or directory account.</p>
        </div>
        {person && <button type="button" className="btn btn-sm btn-ghost" onClick={reload}>Refresh</button>}
      </div>
      {credential === "identity-provider" && (
        <Notice testId="sign-ins-provider" title="This browser is signed in through your identity provider">
          Apelora does not keep that sign-in, so it is not listed. It ends within an hour, when the provider’s token expires; sign out at the provider to end it sooner.
        </Notice>
      )}
      {credential === "personal-token" && <Notice testId="sign-ins-token">This page was opened with a personal token, which is not a sign-in. Manage it under Your tokens.</Notice>}
      {!person ? (
        <Notice testId="sign-ins-people-only">Sign-ins belong to people. You are using {me.kind === "service" ? "an automation identity" : "an API token"}.</Notice>
      ) : (
        <Section title="Signed in now" state={list} testId="my-sign-ins" onRetry={reload}>
          {(l) => {
            const items = l?.items ?? [];
            return (
              <>
                {notice && <Notice tone="ok" testId="sign-ins-notice">{notice}</Notice>}
                {error && <ErrorPanel message={error} testId="sign-ins-error" />}
                {items.length === 0 ? (
                  <p className="text-sm text-base-content/70" data-testid="my-sign-ins-empty">You are not signed in anywhere with a username and password or a directory.</p>
                ) : (
                  <SignInTable items={items} busy={busy} onEnd={setEnding} />
                )}
              </>
            );
          }}
        </Section>
      )}
      <ConfirmDialog
        open={!!ending}
        title="Sign out that browser?"
        body={<p>Whoever uses it has to sign in again. Your other sign-ins are not affected.</p>}
        action="Sign out"
        busy={busy}
        onCancel={() => setEnding(null)}
        onConfirm={() => { if (ending) end(ending); }}
      />
    </div>
  );
}

/** Everyone's sign-ins (Identity & Access → Sign-ins & tokens; `user.manage`). */
export function PeopleSignInsSection({ onAuth }: { onAuth: (m: string) => void }) {
  const [list, reload] = useLoad<ActiveSignInList>(() => client.get("sign_ins"), onAuth);
  const [ending, setEnding] = useState<ActiveSignIn | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const { busy, error, run } = useRun(onAuth);
  const end = (s: ActiveSignIn) => {
    setEnding(null);
    setNotice(null);
    void run(async () => {
      await client.request("end_sign_in", { params: { id: s.id } });
      setNotice(`${s.person} was signed out of that browser.`);
      reload();
    });
  };
  return (
    <Section title="Signed in now" state={list} testId="people-sign-ins" onRetry={reload}>
      {(l) => (
        <>
          <SectionHeader title="Signed in now" count={l.items.length}>
            <button type="button" className="btn btn-sm btn-ghost" onClick={reload}>Refresh</button>
          </SectionHeader>
          <p className="text-sm text-base-content/70">Identity provider sign-ins are not listed: the provider ends those.</p>
          {notice && <Notice tone="ok" testId="people-sign-ins-notice">{notice}</Notice>}
          {error && <ErrorPanel message={error} testId="people-sign-ins-error" />}
          {l.items.length === 0 ? (
            <p className="text-sm text-base-content/70" data-testid="people-sign-ins-empty">Nobody is signed in with a username and password or a directory.</p>
          ) : (
            <SignInTable person items={l.items} busy={busy} onEnd={setEnding} />
          )}
          <ConfirmDialog
            open={!!ending}
            title={`Sign ${ending?.person ?? ""} out of that browser?`}
            body={<p>They have to sign in again there. To keep them out, disable the person under Users.</p>}
            action="Sign out"
            busy={busy}
            onCancel={() => setEnding(null)}
            onConfirm={() => { if (ending) end(ending); }}
          />
        </>
      )}
    </Section>
  );
}

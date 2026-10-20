import { useId, useState } from "react";

import type { ServiceIdentityDto, ServiceKind } from "@apelora/api-client";

import { SecretOnceDialog, type OneTimeSecret } from "../../components/SecretOnce";
import { Badge, ConfirmDialog, ErrorPanel, FormDialog, Notice, Section, SectionHeader, TableFrame } from "../../components/ui";
import { client, useLoad } from "../../lib/api";
import { fmtDate, fmtTime, useRun } from "../../lib/identity";

const KINDS: { value: ServiceKind; label: string; what: string }[] = [
  { value: "workload", label: "Workload", what: "creates and changes instances in the tenants' Incus projects" },
  { value: "network", label: "Network", what: "manages networks, ACLs and load balancers" },
  { value: "storage", label: "Storage", what: "manages storage volumes" },
  { value: "provisioning", label: "Provisioning", what: "commissions, installs and powers physical machines and issues join tokens" },
  { value: "automation", label: "Automation", what: "a script or pipeline that calls the Apelora API with its own token and ordinary role assignments" },
];
const CREDENTIAL = /^[A-Za-z0-9._/-]{1,80}$/;

const NoSecrets = ({ testId }: { testId: string }) => (
  <Notice tone="info" testId={testId} title="Apelora never stores secrets">
    Identities that call other systems record only the <em>name</em> of a credential kept in the secret store. An automation identity gets an API token that is shown once, when you create it, and is stored only as a hash.
  </Notice>
);

function CreateForm({ onAuth, onClose, onCreated }: { onAuth: (m: string) => void; onClose: () => void; onCreated: (r: { identity: ServiceIdentityDto; token: string | null }) => void }) {
  const [name, setName] = useState("");
  const [kind, setKind] = useState<ServiceKind>("automation");
  const [credential, setCredential] = useState("");
  const [days, setDays] = useState(90);
  const [restrictions, setRestrictions] = useState("");
  const { busy, error, run } = useRun(onAuth);
  const ids = [useId(), useId(), useId(), useId(), useId()];
  const automation = kind === "automation";
  const credentialBad = !automation && !!credential && !CREDENTIAL.test(credential);
  let parsed: Record<string, unknown> | undefined;
  let restrictionError: string | null = null;
  if (restrictions.trim()) {
    try {
      const v: unknown = JSON.parse(restrictions);
      if (v && typeof v === "object" && !Array.isArray(v)) parsed = v as Record<string, unknown>;
      else restrictionError = "Restrictions must be a JSON object such as {\"projects\": [\"acme-prod\"]}.";
    } catch { restrictionError = "That is not valid JSON."; }
  }
  const daysOk = !automation || (Number.isInteger(days) && days >= 1 && days <= 365);
  const ok = !!name.trim() && !credentialBad && !restrictionError && daysOk && !busy;
  const submit = () => void run(async () => {
    const r = await client.request("create_service_identity", { body: { name: name.trim(), kind, credentialRef: !automation && credential.trim() ? credential.trim() : null, expiresInDays: automation ? days : null, restrictions: parsed ?? {} } });
    onCreated({ identity: r.identity, token: r.token ?? null });
  });
  return (
    <form className="flex flex-col gap-3" onSubmit={(e) => { e.preventDefault(); if (ok) submit(); }}>
      <NoSecrets testId="no-secrets-dialog" />
      <div className="flex flex-col gap-1 text-sm">
        <label htmlFor={ids[0]}>Name</label>
        <input id={ids[0]} className="input input-sm w-full" value={name} onChange={(e) => setName(e.target.value)} maxLength={80} autoComplete="off" data-testid="service-name" />
      </div>
      <div className="flex flex-col gap-1 text-sm">
        <label htmlFor={ids[1]}>Kind</label>
        <select id={ids[1]} className="select select-sm w-full" value={kind} onChange={(e) => setKind(e.target.value as ServiceKind)} data-testid="service-kind">
          {KINDS.map((k) => <option key={k.value} value={k.value}>{k.label}</option>)}
        </select>
        <span className="text-xs text-base-content/60" data-testid="service-kind-note">{KINDS.find((k) => k.value === kind)?.what}.</span>
      </div>
      {automation ? (
        <div className="flex flex-col gap-1 text-sm">
          <label htmlFor={ids[2]}>Token expires after (days, 1–365)</label>
          <input id={ids[2]} type="number" min={1} max={365} className="input input-sm w-32" value={days} onChange={(e) => setDays(Number(e.target.value))} aria-invalid={!daysOk} data-testid="service-days" />
          <span className="text-xs text-base-content/60">After you create it, the token is shown once. Give it roles on the Project memberships or Users pages.</span>
        </div>
      ) : (
        <div className="flex flex-col gap-1 text-sm">
          <label htmlFor={ids[3]}>Credential name in the secret store</label>
          <input id={ids[3]} className="input input-sm w-full font-mono" value={credential} onChange={(e) => setCredential(e.target.value)} placeholder="incus/deploy-client" autoComplete="off" spellCheck={false} aria-invalid={credentialBad} data-testid="service-credential" />
          <span className={`text-xs ${credentialBad ? "text-error" : "text-base-content/60"}`}>The name only (letters, digits and . _ - /), never the secret itself.</span>
        </div>
      )}
      <div className="flex flex-col gap-1 text-sm">
        <label htmlFor={ids[4]}>Restrictions (optional JSON)</label>
        <textarea id={ids[4]} className="textarea textarea-sm w-full font-mono text-xs" rows={2} value={restrictions} onChange={(e) => setRestrictions(e.target.value)} placeholder={'{"projects": ["acme-prod"]}'} spellCheck={false} aria-invalid={!!restrictionError} data-testid="service-restrictions" />
        {restrictionError && <span className="text-xs text-error" role="alert">{restrictionError}</span>}
      </div>
      {error && <ErrorPanel message={error} testId="service-error" />}
      <div className="modal-action mt-0">
        <button type="button" className="btn btn-sm btn-ghost" onClick={onClose}>Cancel</button>
        <button type="submit" className="btn btn-sm btn-primary" disabled={!ok} data-testid="service-submit">{busy ? "Creating…" : "Create identity"}</button>
      </div>
    </form>
  );
}

export function ServiceIdentitiesView({ onAuth }: { onAuth: (m: string) => void }) {
  const [list, reload] = useLoad<ServiceIdentityDto[]>(() => client.get("service_identities").then((r) => r.items), onAuth);
  const [creating, setCreating] = useState(false);
  const [secret, setSecret] = useState<OneTimeSecret | null>(null);
  const [toggle, setToggle] = useState<{ item: ServiceIdentityDto; disable: boolean } | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const { error, run } = useRun(onAuth);

  const change = (s: ServiceIdentityDto, disable: boolean) => {
    setNotice(null);
    void run(async () => {
      await client.request(disable ? "disable_service_identity" : "enable_service_identity", { params: { id: s.id } });
      setNotice(`${s.name} is ${disable ? "disabled" : "enabled"}.`);
      reload();
    });
  };

  return (
    <>
      <Section title="Service identities" level={1} state={list} testId="services" onRetry={reload}>
        {(items) => (
          <>
            <SectionHeader title="Service identities" level={1} count={items.length}>
              <button type="button" className="btn btn-sm btn-primary" onClick={() => setCreating(true)} data-testid="service-open">Create identity</button>
            </SectionHeader>
            <p className="text-sm text-base-content/70">The credentials Apelora uses for Incus and physical machines, and automation identities that call this API. Each kind can do one family of things, so a leaked credential does not open the others.</p>
            <NoSecrets testId="no-secrets-note" />
            {notice && <Notice tone="ok" testId="services-notice">{notice}</Notice>}
            {error && <ErrorPanel message={error} testId="services-action-error" />}
            {items.length === 0 ? <p className="text-sm text-base-content/70" data-testid="services-empty">No service identities yet.</p> : (
              <TableFrame>
                <thead><tr><th>Name</th><th>Kind</th><th className="max-lg:hidden">Allowed operations</th><th className="max-md:hidden">Credential</th><th className="max-sm:hidden">Expires</th><th>Status</th><th><span className="sr-only">Actions</span></th></tr></thead>
                <tbody>
                  {items.map((s) => (
                    <tr key={s.id} data-testid="service-row" className={s.disabled ? "text-base-content/60" : undefined}>
                      <td className="font-medium">{s.name}<span className="block text-xs font-normal text-base-content/60">Last used {fmtTime(s.lastUsedAt)}</span></td>
                      <td>{KINDS.find((k) => k.value === s.kind)?.label ?? s.kind}<span className="block text-xs text-base-content/60">for {s.backend}</span></td>
                      <td className="max-lg:hidden">
                        {s.operations.length === 0
                          ? <span>{s.apiAccess ? "Calls the Apelora API with its roles" : "None"}</span>
                          : <details><summary className="cursor-pointer">{s.operations.length} operations</summary><ul className="mt-1 font-mono text-xs">{s.operations.map((o) => <li key={o}>{o}</li>)}</ul></details>}
                      </td>
                      <td className="max-md:hidden">{s.credentialRef ? <span className="font-mono text-xs" data-testid="credential-ref">{s.credentialRef}</span> : s.apiAccess ? "API token (hashed)" : "—"}</td>
                      <td className="whitespace-nowrap max-sm:hidden">{s.expiresAt ? fmtDate(s.expiresAt) : "Never"}</td>
                      <td>{s.disabled ? <Badge tone="error">Disabled</Badge> : <Badge tone="ok">Enabled</Badge>}</td>
                      <td className="whitespace-nowrap text-right">
                        <button type="button" className={`btn btn-xs btn-ghost ${s.disabled ? "" : "text-error"}`} aria-label={`${s.disabled ? "Enable" : "Disable"} ${s.name}`} onClick={() => setToggle({ item: s, disable: !s.disabled })}>{s.disabled ? "Enable…" : "Disable…"}</button>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </TableFrame>
            )}
          </>
        )}
      </Section>

      <FormDialog open={creating} title="Create a service identity" onClose={() => setCreating(false)} testId="service-dialog">
        {creating && (
          <CreateForm onAuth={onAuth} onClose={() => setCreating(false)} onCreated={({ identity, token }) => {
            setCreating(false);
            reload();
            if (token) {
              setSecret({ title: `API token for ${identity.name}`, what: "API token", value: token });
              setNotice(`${identity.name} created. Its token is shown once.`);
            } else {
              setNotice(`${identity.name} registered. Apelora recorded only the credential name${identity.credentialRef ? ` ${identity.credentialRef}` : ""}, never the secret.`);
            }
          }} />
        )}
      </FormDialog>
      <SecretOnceDialog secret={secret} onClose={() => setSecret(null)} />
      <ConfirmDialog
        open={!!toggle}
        title={`${toggle?.disable ? "Disable" : "Enable"} ${toggle?.item.name ?? ""}?`}
        body={toggle?.disable ? <p>It stops working at once. Anything that depends on it fails until you enable it again.</p> : <p>It can be used again straight away.</p>}
        action={toggle?.disable ? "Disable identity" : "Enable identity"}
        danger={!!toggle?.disable}
        onCancel={() => setToggle(null)}
        onConfirm={() => { const t = toggle; setToggle(null); if (t) change(t.item, t.disable); }}
      />
    </>
  );
}

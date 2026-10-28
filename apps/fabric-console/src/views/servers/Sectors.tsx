// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";

import { ApiRequestError, type SectorInfo, type SectorList, type SectorState, type WhoAmI } from "@apelora/api-client";

import { Badge, ConfirmDialog, ErrorPanel, FormDialog, Notice, Section, SectionHeader, TableFrame, sentence, type Tone } from "../../components/ui";
import { client, messageOf, useLoad } from "../../lib/api";
import { holdsPlatform } from "../../lib/identity";

const STATE: Record<SectorState, { label: string; tone: Tone }> = {
  connected: { label: "Connected", tone: "ok" },
  unreachable: { label: "Unreachable", tone: "error" },
  "not-configured": { label: "Identity missing", tone: "warn" },
};

const NAME = /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/;
const CREDENTIALS = /^[A-Z0-9_]{1,40}$/;

/** Registers another Incus cluster. Only names are sent; the identity's files stay on the control plane. */
function AddSector({ open, onClose, onAdded, onAuth }: { open: boolean; onClose: () => void; onAdded: () => void; onAuth: (m: string) => void }) {
  const [form, setForm] = useState({ name: "", site: "", url: "https://", credentials: "" });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const set = (k: keyof typeof form) => (e: React.ChangeEvent<HTMLInputElement>) => setForm((f) => ({ ...f, [k]: e.target.value }));
  const valid = NAME.test(form.name) && NAME.test(form.site) && /^https:\/\/[^/?#@\s]+\/?$/.test(form.url) && CREDENTIALS.test(form.credentials);
  const vars = ["CLIENT_CERT", "CLIENT_KEY", "SERVER_CERT"].map((f) => `APELORA_SECTOR_${form.credentials || "<NAME>"}_${f}`);
  const save = async () => {
    setBusy(true);
    setError(null);
    try {
      await client.request("add_sector", { body: form });
      onAdded();
      onClose();
      setForm({ name: "", site: "", url: "https://", credentials: "" });
    } catch (e) {
      if (e instanceof ApiRequestError && e.unauthenticated) onAuth(e.message);
      setError(messageOf(e));
    } finally {
      setBusy(false);
    }
  };
  const field = (k: keyof typeof form, label: string, hint: string, mono = false) => (
    <label className="flex flex-col gap-1 text-sm">
      <span>{label}</span>
      <input className={`input input-sm w-full ${mono ? "font-mono" : ""}`} value={form[k]} onChange={set(k)} autoComplete="off" spellCheck={false} data-testid={`sector-${k}`} />
      <span className="text-xs text-base-content/60">{hint}</span>
    </label>
  );
  return (
    <FormDialog open={open} title="Register a sector" onClose={onClose} testId="sector-dialog">
      <p className="text-sm text-base-content/80">Another Incus cluster, in this site or another. Apelora reads its state; applications still deploy to the home sector only.</p>
      {field("name", "Sector", "Lowercase letters, digits and '-', e.g. rack-b1")}
      {field("site", "Site", "The data centre it is in, e.g. site-b")}
      {field("url", "Incus address", "https://host:port of its Incus API", true)}
      {field("credentials", "Credentials name", "Capital letters, digits and '_'", true)}
      <div className="rounded-box border border-base-300 bg-base-200 px-3 py-2 text-xs" data-testid="sector-vars">
        <p className="mb-1">On the control plane, these variables must name the identity's files:</p>
        <ul className="font-mono">{vars.map((v) => <li key={v}>{v}</li>)}</ul>
      </div>
      {error && <ErrorPanel message={error} testId="sector-error" />}
      <div className="modal-action mt-0">
        <button type="button" className="btn btn-sm btn-outline" onClick={onClose}>Cancel</button>
        <button type="button" className="btn btn-sm btn-primary" disabled={busy || !valid} onClick={() => void save()} data-testid="sector-save">{busy ? "Registering…" : "Register"}</button>
      </div>
    </FormDialog>
  );
}

function Row({ s, canManage, onForget }: { s: SectorInfo; canManage: boolean; onForget: (s: SectorInfo) => void }) {
  const st = STATE[s.state];
  return (
    <tr data-testid="sector-row" data-state={s.state}>
      <td>
        <span className="font-medium">{s.name}</span>
        {s.home && <span className="ml-2 badge badge-sm badge-ghost">home</span>}
      </td>
      <td>{s.site}</td>
      <td className="font-mono text-xs">{s.url ?? "this control plane's Incus"}</td>
      <td>
        <Badge tone={st.tone}>{st.label}</Badge>
        {s.detail && <span className="mt-1 block text-xs text-base-content/70">{sentence(s.detail)}</span>}
        {s.state === "not-configured" && s.credentialVars.length > 0 && (
          <span className="mt-1 block text-xs text-base-content/70">Set {s.credentialVars.join(", ")} on the control plane.</span>
        )}
      </td>
      <td className="tabular-nums">
        {s.members ?? "—"}
        {s.warning && <span className="mt-1 block text-xs text-warning">{sentence(s.warning)}</span>}
      </td>
      <td className="tabular-nums">{s.incusVersion ?? "—"}</td>
      <td className="text-right">
        {canManage && !s.home && (
          <button type="button" className="btn btn-xs btn-ghost text-error" onClick={() => onForget(s)} aria-label={`Forget ${s.name}`} data-testid="sector-forget">Forget…</button>
        )}
      </td>
    </tr>
  );
}

/**
 * Servers → Sectors (ADR 0008): the Incus cluster this control plane runs against and the others
 * it has been told about, each with whether it answers now. Platform administrators register and
 * forget sectors; nothing is changed in any cluster.
 */
export function SectorsTab({ me, onAuth }: { me: WhoAmI | null; onAuth: (m: string) => void }) {
  const [list, reload] = useLoad<SectorList>(() => client.get("sectors"), onAuth);
  const canManage = holdsPlatform(me, "cluster.manage");
  const [adding, setAdding] = useState(false);
  const [forget, setForget] = useState<SectorInfo | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const confirmForget = async () => {
    if (!forget) return;
    setBusy(true);
    setError(null);
    try {
      await client.request("remove_sector", { params: { name: forget.name }, body: { confirm: forget.name } });
      setForget(null);
      reload();
    } catch (e) {
      setError(messageOf(e));
      setForget(null);
    } finally {
      setBusy(false);
    }
  };
  return (
    <Section title="Sectors" state={list} testId="sectors" onRetry={reload}>
      {(l) => (
        <>
          <SectionHeader title="Sectors" count={l.items.length} source={l.source}>
            {canManage && <button type="button" className="btn btn-sm btn-primary" onClick={() => setAdding(true)} data-testid="sector-add">Register a sector…</button>}
          </SectionHeader>
          <Notice tone="info" testId="sectors-note">{l.note}</Notice>
          <TableFrame>
            <thead><tr><th>Sector</th><th>Site</th><th>Incus</th><th>State</th><th>Members</th><th>Release</th><th><span className="sr-only">Actions</span></th></tr></thead>
            <tbody>{l.items.map((s) => <Row key={`${s.home}-${s.name}`} s={s} canManage={canManage} onForget={setForget} />)}</tbody>
          </TableFrame>
          {error && <ErrorPanel message={error} testId="sectors-error" />}
          <AddSector open={adding} onClose={() => setAdding(false)} onAdded={reload} onAuth={onAuth} />
          <ConfirmDialog
            open={!!forget}
            title={`Forget ${forget?.name ?? ""}?`}
            body="Apelora stops reading this cluster. Nothing changes in it, and its instances keep running."
            action="Forget sector"
            typeToConfirm={forget?.name}
            busy={busy}
            onConfirm={() => void confirmForget()}
            onCancel={() => setForget(null)}
          />
        </>
      )}
    </Section>
  );
}

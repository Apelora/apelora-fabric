import { useState } from "react";

import { ApiRequestError, type MaasMachineView, type MaasMachines } from "@apelora/api-client";

import { ErrorPanel, FormDialog, Section, SectionHeader, TableFrame } from "../components/ui";
import { client, messageOf, useLoad } from "../lib/api";

const NAME = /^[A-Za-z](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?$/;

/** MAAS's power state in words, with its colour: only "on" and "off" are something MAAS read. */
const POWER: Record<string, { label: string; dot: string }> = {
  on: { label: "Powered on", dot: "status-success" },
  off: { label: "Powered off", dot: "status-neutral" },
  unknown: { label: "Power unknown", dot: "status-neutral" },
  error: { label: "Power check failed", dot: "status-error" },
};

const memory = (mib: number) => (mib >= 1024 ? `${Math.round(mib / 1024)} GiB` : `${mib} MiB`);

/** A member name to offer: the host's short name in lower case, hyphens for anything else. */
const suggest = (host: string) => {
  const s = host.split(".")[0]!.toLowerCase().replace(/[^a-z0-9-]/g, "-").replace(/^[^a-z]+/, "").replace(/-+$/, "").slice(0, 63);
  return s;
};

/**
 * The machines MAAS manages (a provisioner Apelora talks to read-only), next to the ones Apelora
 * registered for them. Registering one here records its MAAS system id; joining the cluster is the
 * same approval and admission as for any machine.
 */
export function MaasMachinesSection({ onAuth, onChanged }: { onAuth: (m: string) => void; onChanged: () => void }) {
  // An older control plane has no such endpoint (404): the section is then left out, not an error.
  const [list, reload] = useLoad<MaasMachines | null>(
    () => client.get("maas_machines").catch((e: unknown) => { if (e instanceof ApiRequestError && e.status === 404) return null; throw e; }),
    onAuth,
  );
  const [target, setTarget] = useState<MaasMachineView | null>(null);
  const [name, setName] = useState("");
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const nameError = name && !NAME.test(name) ? "Use 1–63 letters, digits and hyphens, starting with a letter." : null;

  const open = (m: MaasMachineView) => { setTarget(m); setName(suggest(m.hostname)); setNote(""); setError(null); };
  const register = async () => {
    if (!target) return;
    setBusy(true);
    setError(null);
    try {
      await client.request("register_machine", { body: { name, provider: "maas", systemId: target.systemId, description: note } });
      setNotice(`${name} is registered. Approve it under Machines to get its join token.`);
      setTarget(null);
      reload();
      onChanged();
    } catch (e) {
      setError(messageOf(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Section title="MAAS" state={list} testId="maas" onRetry={reload}>
      {(l) => !l ? null : (
        <>
          <SectionHeader title="Machines MAAS manages" count={l.configured ? l.items.length : undefined} source={l.source} />
          {!l.configured ? (
            <p className="text-sm text-base-content/70" data-testid="maas-not-configured">
              MAAS is not connected. Set <code className="font-mono text-xs">APELORA_MAAS_URL</code> and <code className="font-mono text-xs">APELORA_MAAS_API_KEY</code> on the control plane to see the machines it manages here. Servers you install by hand are added under Machines.
            </p>
          ) : (
            <>
              <p className="text-sm text-base-content/70">What MAAS reports, read-only. Register a machine to enrol it in the cluster; its BMC credentials stay in MAAS.</p>
              {notice && <p role="status" className="flex items-start gap-2 text-sm" data-testid="maas-notice"><span className="status status-success mt-1.5 shrink-0" aria-hidden="true" />{notice}</p>}
              {l.items.length === 0 ? <p className="text-sm text-base-content/70">MAAS manages no machines yet.</p> : (
                <TableFrame>
                  <thead><tr><th>Machine</th><th>Status</th><th>Power</th><th className="max-md:hidden">Hardware</th><th><span className="sr-only">Actions</span></th></tr></thead>
                  <tbody>
                    {l.items.map((m) => {
                      const p = POWER[m.powerState] ?? { label: m.powerState, dot: "status-neutral" };
                      return (
                        <tr key={m.systemId} data-testid="maas-row">
                          <td className="font-medium">{m.hostname}<span className="block font-mono text-xs font-normal text-base-content/60">{m.systemId}{m.addresses[0] ? ` · ${m.addresses[0]}` : ""}</span></td>
                          <td>{m.status}{m.os && <span className="block text-xs text-base-content/60">{m.os}</span>}</td>
                          <td className="sm:whitespace-nowrap"><span className={`status ${p.dot} mr-2`} aria-hidden="true" />{p.label}<span className="block text-xs text-base-content/60">via {m.powerType || "nothing"}</span></td>
                          <td className="text-sm max-md:hidden">{m.architecture}<span className="block text-xs text-base-content/60">{m.cpuCount} CPUs · {memory(m.memoryMib)}{m.zone && ` · ${m.zone}`}</span></td>
                          <td className="text-right">
                            {m.registeredAs
                              ? <span className="text-sm text-base-content/70" data-testid="maas-registered">Registered as {m.registeredAs}{m.memberStatus ? ` · ${m.memberStatus}` : ""}</span>
                              : <button type="button" className="btn btn-xs btn-outline" aria-label={`Register ${m.hostname}`} onClick={() => open(m)} data-testid="maas-register">Register…</button>}
                          </td>
                        </tr>
                      );
                    })}
                  </tbody>
                </TableFrame>
              )}
            </>
          )}
          <FormDialog open={!!target} title={`Register ${target?.hostname ?? ""}`} onClose={() => setTarget(null)} testId="maas-register-dialog">
            <p className="text-sm text-base-content/80">Apelora remembers its MAAS id ({target?.systemId}). It joins the cluster after you approve it and run the enrolment command on it.</p>
            <label className="flex flex-col gap-1 text-sm">
              Name in the cluster
              <input className="input input-sm w-full font-mono" value={name} onChange={(e) => setName(e.target.value)} autoComplete="off" spellCheck={false} aria-invalid={!!nameError} data-testid="maas-name" />
              {nameError && <span className="text-xs text-error">{nameError}</span>}
            </label>
            <label className="flex flex-col gap-1 text-sm">
              Note (optional)
              <input className="input input-sm w-full" maxLength={200} value={note} onChange={(e) => setNote(e.target.value)} autoComplete="off" />
            </label>
            {error && <ErrorPanel message={error} testId="maas-register-error" />}
            <div className="modal-action mt-0">
              <button type="button" className="btn btn-sm btn-outline" onClick={() => setTarget(null)}>Cancel</button>
              <button type="button" className="btn btn-sm btn-primary" disabled={busy || !name || !!nameError} onClick={() => void register()} data-testid="maas-register-confirm">{busy ? "Registering…" : "Register machine"}</button>
            </div>
          </FormDialog>
        </>
      )}
    </Section>
  );
}

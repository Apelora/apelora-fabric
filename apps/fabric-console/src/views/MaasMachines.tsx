import { useEffect, useRef, useState } from "react";

import { ApiRequestError, type MaasMachineView, type MaasMachines, type WhoAmI } from "@apelora/api-client";

import { ErrorPanel, FormDialog, Section, SectionHeader, TableFrame } from "../components/ui";
import { client, messageOf, useLoad } from "../lib/api";
import { holdsPlatform } from "../lib/identity";

const NAME = /^[A-Za-z](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?$/;

/** MAAS's power state in words, with its colour: only "on" and "off" are something MAAS read. */
const POWER: Record<string, { label: string; dot: string }> = {
  on: { label: "Powered on", dot: "status-success" },
  off: { label: "Powered off", dot: "status-neutral" },
  unknown: { label: "Power unknown", dot: "status-neutral" },
  error: { label: "Power check failed", dot: "status-error" },
};

/** MAAS states a hardware test run may start in (the API refuses the others). */
const TESTABLE = new Set(["Ready", "Broken", "Failed testing"]);

/** MAAS's last test result, with its colour. "None" (never tested) is shown as such. */
const TESTS: Record<string, { label: string; dot: string }> = {
  Passed: { label: "Tests passed", dot: "status-success" },
  Failed: { label: "Tests failed", dot: "status-error" },
  "Timed out": { label: "Tests timed out", dot: "status-error" },
  Degraded: { label: "Tests degraded", dot: "status-warning" },
  Running: { label: "Tests running", dot: "status-info" },
  Pending: { label: "Tests pending", dot: "status-info" },
  None: { label: "Not tested", dot: "status-neutral" },
  Unknown: { label: "Test result unknown", dot: "status-neutral" },
};

type Kind = "commission" | "test" | "deploy" | "release";

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
export function MaasMachinesSection({ me, onAuth, onChanged }: { me: WhoAmI | null; onAuth: (m: string) => void; onChanged: () => void }) {
  // An older control plane has no such endpoint (404): the section is then left out, not an error.
  const [list, reload] = useLoad<MaasMachines | null>(
    () => client.get("maas_machines").catch((e: unknown) => { if (e instanceof ApiRequestError && e.status === 404) return null; throw e; }),
    onAuth,
  );
  const legacy = !!me?.permissions.includes("manage-infrastructure");
  const canCommission = legacy || holdsPlatform(me, "hardware.commission");
  const canDeploy = legacy || holdsPlatform(me, "hardware.deploy");
  const canErase = legacy || holdsPlatform(me, "hardware.delete");
  const [target, setTarget] = useState<MaasMachineView | null>(null);
  const [act, setAct] = useState<{ machine: MaasMachineView; kind: Kind } | null>(null);
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
              <p className="text-sm text-base-content/70">What MAAS reports. Register a machine to enrol it in the cluster; its BMC credentials stay in MAAS. Commissioning, hardware tests (burn-in) and installing an operating system are done by MAAS.</p>
              {notice && <p role="status" className="flex items-start gap-2 text-sm" data-testid="maas-notice"><span className="status status-success mt-1.5 shrink-0" aria-hidden="true" />{notice}</p>}
              {l.items.length === 0 ? <p className="text-sm text-base-content/70">MAAS manages no machines yet.</p> : (
                <TableFrame>
                  <thead><tr><th>Machine</th><th>Status</th><th>Power</th><th className="max-md:hidden">Hardware</th><th><span className="sr-only">Actions</span></th></tr></thead>
                  <tbody>
                    {l.items.map((m) => {
                      const p = POWER[m.powerState] ?? { label: m.powerState, dot: "status-neutral" };
                      const t = m.testingStatus ? TESTS[m.testingStatus] ?? { label: `Tests: ${m.testingStatus}`, dot: "status-neutral" } : null;
                      return (
                        <tr key={m.systemId} data-testid="maas-row">
                          <td className="font-medium">{m.hostname}<span className="block font-mono text-xs font-normal text-base-content/60">{m.systemId}{m.addresses[0] ? ` · ${m.addresses[0]}` : ""}</span></td>
                          <td>{m.status}{m.os && <span className="block text-xs text-base-content/60">{m.os}</span>}{t && <span className="block text-xs text-base-content/60" data-testid="maas-testing"><span className={`status ${t.dot} mr-1.5`} aria-hidden="true" />{t.label}</span>}</td>
                          <td className="sm:whitespace-nowrap"><span className={`status ${p.dot} mr-2`} aria-hidden="true" />{p.label}<span className="block text-xs text-base-content/60">via {m.powerType || "nothing"}</span></td>
                          <td className="text-sm max-md:hidden">{m.architecture}<span className="block text-xs text-base-content/60">{m.cpuCount} CPUs · {memory(m.memoryMib)}{m.zone && ` · ${m.zone}`}</span></td>
                          <td className="text-right">
                            <span className="inline-flex flex-wrap justify-end gap-1">
                              {canCommission && <button type="button" className="btn btn-xs btn-outline" aria-label={`Commission ${m.hostname}`} onClick={() => setAct({ machine: m, kind: "commission" })} data-testid="maas-commission">Commission…</button>}
                              {canCommission && TESTABLE.has(m.status) && <button type="button" className="btn btn-xs btn-outline" aria-label={`Run hardware tests on ${m.hostname}`} onClick={() => setAct({ machine: m, kind: "test" })} data-testid="maas-test">Burn-in…</button>}
                              {canDeploy && <button type="button" className="btn btn-xs btn-outline" aria-label={`Install an operating system on ${m.hostname}`} onClick={() => setAct({ machine: m, kind: "deploy" })} data-testid="maas-deploy">Install…</button>}
                              {canDeploy && <button type="button" className="btn btn-xs btn-ghost text-error" aria-label={`Release ${m.hostname}`} onClick={() => setAct({ machine: m, kind: "release" })} data-testid="maas-release">Release…</button>}
                              {m.registeredAs
                                ? <span className="text-sm text-base-content/70" data-testid="maas-registered">Registered as {m.registeredAs}{m.memberStatus ? ` · ${m.memberStatus}` : ""}</span>
                                : <button type="button" className="btn btn-xs btn-outline" aria-label={`Register ${m.hostname}`} onClick={() => open(m)} data-testid="maas-register">Register…</button>}
                            </span>
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
          {act && <MaasAction machine={act.machine} kind={act.kind} canErase={canErase} onClose={() => setAct(null)} onDone={(text) => { setNotice(text); setAct(null); reload(); }} />}
        </>
      )}
    </Section>
  );
}

/** Focuses `ref` once its dialog is visible. The modal is `visibility: hidden` while it opens. */
function useFocusWhenShown(ref: { current: HTMLElement | null }, active: boolean) {
  useEffect(() => {
    if (!active) return;
    let frames = 30;
    let id = 0;
    const focus = () => {
      const el = ref.current;
      el?.focus();
      if (el && document.activeElement !== el && frames-- > 0) id = requestAnimationFrame(focus);
    };
    focus();
    return () => cancelAnimationFrame(id);
  }, [ref, active]);
}

/**
 * Commission, run hardware tests, or install an operating system. The hostname is typed again. Cloud-init is sent to
 * MAAS and is not kept here; a join token inside it is not stored.
 */
function MaasAction({ machine, kind, canErase, onClose, onDone }: { machine: MaasMachineView; kind: Kind; canErase: boolean; onClose: () => void; onDone: (notice: string) => void }) {
  const [typed, setTyped] = useState("");
  const [distro, setDistro] = useState("");
  const [cloud, setCloud] = useState("");
  const [erase, setErase] = useState(false);
  const [scripts, setScripts] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const field = useRef<HTMLInputElement>(null);
  useFocusWhenShown(field, true);
  const distroError = distro && !/^[A-Za-z0-9-]{1,64}$/.test(distro) ? "Use the short series name, such as noble." : null;
  const names = scripts.split(/[\s,]+/).filter(Boolean);
  const scriptsError = names.length > 16 ? "Name at most 16 tests." : names.some((n) => !/^[A-Za-z0-9._-]{1,64}$/.test(n)) ? "Test names use letters, digits, '.', '_' and '-'." : null;

  const go = async () => {
    setBusy(true);
    setError(null);
    try {
      if (kind === "commission") {
        await client.request("maas_commission", { params: { systemId: machine.systemId }, body: { confirm: typed } });
        onDone(`${machine.hostname} is being commissioned by MAAS.`);
      } else if (kind === "test") {
        await client.request("maas_test", { params: { systemId: machine.systemId }, body: { confirm: typed, scripts: names } });
        onDone(`MAAS is running ${names.length ? names.join(", ") : "its default tests"} on ${machine.hostname}. The result shows in the Status column.`);
      } else if (kind === "release") {
        await client.request("maas_release", { params: { systemId: machine.systemId }, body: { confirm: typed, eraseDisks: erase } });
        onDone(erase ? `${machine.hostname} is released and its disks are being erased.` : `${machine.hostname} is released. Its disks were not erased.`);
      } else {
        await client.request("maas_deploy", { params: { systemId: machine.systemId }, body: { confirm: typed, distroSeries: distro.trim(), userData: cloud } });
        onDone(`MAAS is installing ${distro.trim()} on ${machine.hostname}. The cloud-init was not stored here.`);
      }
    } catch (e) {
      setError(messageOf(e));
      setBusy(false);
    }
  };

  return (
    <FormDialog open title={kind === "commission" ? `Commission ${machine.hostname}` : kind === "test" ? `Run hardware tests on ${machine.hostname}` : kind === "release" ? `Release ${machine.hostname}` : `Install an operating system on ${machine.hostname}`} onClose={onClose} testId="maas-action-dialog">
      {kind === "commission" ? (
        <p className="text-sm text-base-content/80">MAAS tests the machine and records its hardware. This does not install an operating system.</p>
      ) : kind === "test" ? (
        <>
          <p className="text-sm text-base-content/80">MAAS boots the machine and runs hardware tests on it before it is used. Nothing is installed. Admit it to the cluster only after the tests pass.</p>
          <label className="flex flex-col gap-1 text-sm">
            Tests (optional)
            <input className="input input-sm w-full font-mono" value={scripts} onChange={(e) => setScripts(e.target.value)} autoComplete="off" spellCheck={false} placeholder="MAAS's default tests" aria-invalid={!!scriptsError} aria-describedby="maas-scripts-help" data-testid="maas-scripts" />
            <span id="maas-scripts-help" className="text-xs text-base-content/60">MAAS test script or tag names, separated by commas or spaces.</span>
            {scriptsError && <span className="text-xs text-error">{scriptsError}</span>}
          </label>
        </>
      ) : kind === "release" ? (
        <>
          <p className="text-sm text-base-content/80">MAAS returns this machine to its pool. The installed system stops. Disks stay unless you ask to erase them. A secure wipe is not offered.</p>
          {canErase && (
            <label className="flex items-start gap-2 text-sm">
              <input type="checkbox" className="checkbox checkbox-sm mt-0.5" checked={erase} onChange={(e) => setErase(e.target.checked)} data-testid="maas-erase" />
              Erase the disks. This destroys the installed system.
            </label>
          )}
        </>
      ) : (
        <>
          <p className="text-sm text-base-content/80">MAAS installs the series you name. The cloud-init is sent to MAAS and is not stored by Apelora. A join token in it is not kept.</p>
          <label className="flex flex-col gap-1 text-sm">
            Series
            <input className="input input-sm w-full font-mono" value={distro} onChange={(e) => setDistro(e.target.value)} autoComplete="off" spellCheck={false} placeholder="noble" aria-invalid={!!distroError} data-testid="maas-distro" />
            {distroError && <span className="text-xs text-error">{distroError}</span>}
          </label>
          <label className="flex flex-col gap-1 text-sm">
            Cloud-init
            <textarea className="textarea textarea-sm w-full font-mono" rows={6} value={cloud} onChange={(e) => setCloud(e.target.value)} spellCheck={false} data-testid="maas-userdata" />
          </label>
        </>
      )}
      <label className="flex flex-col gap-1 text-sm">
        Type <span className="font-mono">{machine.hostname}</span> to confirm
        <input ref={field} className="input input-sm w-full font-mono" value={typed} onChange={(e) => setTyped(e.target.value)} autoComplete="off" spellCheck={false} data-testid="maas-action-confirm" />
      </label>
      {error && <ErrorPanel message={error} testId="maas-action-error" />}
      <div className="modal-action mt-0">
        <button type="button" className="btn btn-sm btn-outline" onClick={onClose}>Cancel</button>
        <button type="button" className={kind === "release" ? "btn btn-sm btn-ghost text-error" : "btn btn-sm btn-primary"} disabled={busy || typed !== machine.hostname || (kind === "deploy" && (!distro || !!distroError || !cloud.trim())) || (kind === "test" && !!scriptsError)} onClick={() => void go()} data-testid="maas-action-go">
          {busy ? "Sending…" : kind === "commission" ? "Commission" : kind === "test" ? "Run tests" : kind === "release" ? (erase ? "Release and erase" : "Release") : "Install"}
        </button>
      </div>
    </FormDialog>
  );
}

import { useEffect, useMemo, useRef, useState } from "react";

import { ApiRequestError, followJob, newIdempotencyKey, type Job, type Plan, type PlanAction, type WhoAmI } from "@apelora/api-client";
import { lookupClass } from "@apelora/appliance-catalog";
import { applicationName, compileNetwork, deploymentAppliances, publishedPorts, serviceEnvName, usedCustomClasses, type NetworkPlan, type PublishedPort } from "@apelora/assembly-model";

import { useStudioApi } from "../../features/editor/context";
import { client, messageOf, writeToken } from "../../features/deploy/api";
import { useLive } from "../../features/deploy/live";

type Step =
  | { kind: "connecting" }
  | { kind: "unreachable" }
  | { kind: "sign-in"; message: string }
  | { kind: "not-allowed"; message: string }
  | { kind: "configure" }
  | { kind: "review"; plan: Plan }
  | { kind: "applying"; plan: Plan; jobs: { instance: string; action: PlanAction; job?: Job }[]; revision: number };

const ACTION: Record<PlanAction, { label: string; tone: string }> = {
  create: { label: "Create", tone: "status-success" },
  keep: { label: "Keep", tone: "status-neutral" },
  remove: { label: "Remove", tone: "status-warning" },
  conflict: { label: "Conflict", tone: "status-error" },
};
const NAME = /^[a-z](?:[a-z0-9-]{0,38}[a-z0-9])?$/;

export function DeployDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const api = useStudioApi();
  const ref = useRef<HTMLDialogElement>(null);
  const [step, setStep] = useState<Step>({ kind: "connecting" });
  const [me, setMe] = useState<WhoAmI | null>(null);
  const [token, setToken] = useState("");
  const [appName, setAppName] = useState("");
  const [project, setProject] = useState("");
  const [removeMissing, setRemoveMissing] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const live = useLive();

  const assembly = open ? api.getState().assembly : null;
  const split = useMemo(() => (assembly ? deploymentAppliances(assembly, lookupClass, project || "apelora") : { deployable: [], designOnly: [] }), [assembly, project]);

  const connect = async () => {
    setStep({ kind: "connecting" });
    setError(null);
    try {
      await client.get("health");
    } catch {
      setStep({ kind: "unreachable" });
      return;
    }
    try {
      const who = await client.get("whoami");
      setMe(who);
      if (!who.permissions.includes("deploy")) {
        setStep({ kind: "not-allowed", message: `Signed in as ${who.name} (${who.role}); deploying needs the operator or fabric-admin role.` });
        return;
      }
      if (!who.deployProjects.length) {
        setStep({ kind: "not-allowed", message: "The control plane has no deploy project configured (APELORA_DEPLOY_PROJECTS)." });
        return;
      }
      setProject((p) => (who.deployProjects.includes(p) ? p : who.deployProjects[0]!));
      setStep({ kind: "configure" });
    } catch (e) {
      if (e instanceof ApiRequestError && e.unauthenticated) setStep({ kind: "sign-in", message: e.message });
      else setStep({ kind: "not-allowed", message: messageOf(e) });
    }
  };

  useEffect(() => {
    const d = ref.current;
    if (!d) return;
    if (open && !d.open) {
      d.showModal();
      setAppName((n) => n || applicationName(api.getState().assembly.metadata.name));
      setRemoveMissing(false);
      void connect();
    }
    if (!open && d.open) d.close();
    // eslint-disable-next-line react-hooks/exhaustive-deps -- runs when the dialog opens
  }, [open]);

  const desired = () => split.deployable.map((d) => ({ appliance: d.appliance, spec: d.spec, replicas: d.replicas, availability: d.availability, internet: d.internet }));
  /** The design's connections between appliances that are deployed (rules for the server). */
  const connections = () => {
    const deployed = new Set(split.deployable.map((d) => d.appliance));
    return compileNetwork(api.getState().assembly, appName).intents
      .filter((i) => deployed.has(i.from) && deployed.has(i.to))
      .map((i) => ({ from: i.from, to: i.to, transport: i.transport, port: i.number }));
  };
  // A gateway in the design stands for the outside world: what it connects to is published.
  const published = () => {
    const a = api.getState().assembly;
    const gateways = new Set(a.spec.appliances.filter((x) => x.classRef.name === "gateway").map((x) => x.id));
    return publishedPorts(compileNetwork(a, appName), gateways, new Set(split.deployable.map((d) => d.appliance)));
  };

  const preview = async () => {
    setBusy(true);
    setError(null);
    try {
      const plan = await client.request("plan_application", { params: { name: appName }, body: { project, appliances: desired(), connections: connections(), published: published() } });
      setStep({ kind: "review", plan });
    } catch (e) {
      setError(messageOf(e));
    } finally {
      setBusy(false);
    }
  };

  const apply = async (plan: Plan) => {
    setBusy(true);
    setError(null);
    try {
      const base = api.getState().assembly;
      const used = usedCustomClasses(base, lookupClass);
      const document = { ...base, ...(used.length ? { catalog: { classes: used } } : {}) };
      const res = await client.request("deploy_application", {
        params: { name: appName },
        body: { project, document, appliances: desired(), connections: connections(), published: published(), removeMissing },
        idempotencyKey: newIdempotencyKey(),
      });
      const jobs = res.jobs.map((j) => ({ instance: j.instance, action: j.action }));
      setStep({ kind: "applying", plan: res.plan, jobs, revision: res.revision });
      live.watch(appName, project);
      await Promise.all(
        res.jobs.map((j, i) =>
          followJob(client, j.jobId, (job) =>
            setStep((s) => (s.kind === "applying" ? { ...s, jobs: s.jobs.map((x, k) => (k === i ? { ...x, job } : x)) } : s)),
          ),
        ),
      );
      await live.refresh();
      api.getState().toast("success", `Deployed revision ${res.revision} of ${appName}`);
    } catch (e) {
      setError(messageOf(e));
      if (plan) setStep({ kind: "review", plan });
    } finally {
      setBusy(false);
    }
  };

  const changes = (plan: Plan) => plan.items.filter((i) => i.action === "create" || (i.action === "remove" && removeMissing)).length;
  const conflicts = (plan: Plan) => plan.items.some((i) => i.action === "conflict");
  const nameError = appName && !NAME.test(appName) ? "Lowercase letters, digits and hyphens, starting with a letter (at most 40)." : null;

  return (
    <dialog ref={ref} className="modal" aria-labelledby="deploy-title" onCancel={(e) => { e.preventDefault(); onClose(); }} data-testid="deploy-dialog">
      <div className="modal-box flex max-w-2xl flex-col gap-4">
        <h3 id="deploy-title" className="font-semibold">Deploy to the cluster</h3>

        {step.kind === "connecting" && <p className="text-sm text-base-content/70">Connecting to the control plane…</p>}

        {step.kind === "unreachable" && (
          <div className="flex flex-col gap-2 text-sm" data-testid="deploy-unreachable">
            <p>The Apelora control plane is not reachable, so this design stays design-only.</p>
            <p className="text-base-content/70">Start it with <code className="rounded bg-base-200 px-1 py-0.5 text-xs">cargo run -p apelora-control-plane</code>, then try again.</p>
            <div className="modal-action mt-0">
              <button type="button" className="btn btn-sm btn-ghost" onClick={onClose}>Close</button>
              <button type="button" className="btn btn-sm btn-primary" onClick={() => void connect()}>Try again</button>
            </div>
          </div>
        )}

        {step.kind === "sign-in" && (
          <form className="flex flex-col gap-3 text-sm" data-testid="deploy-sign-in" onSubmit={(e) => { e.preventDefault(); writeToken(token.trim()); setToken(""); void connect(); }}>
            <p>{step.message} Ask an administrator for a token with the operator role. It is kept only for this browser tab.</p>
            <label className="flex flex-col gap-1">
              <span>API token</span>
              <input type="password" className="input input-sm w-full font-mono" autoComplete="off" spellCheck={false} value={token} onChange={(e) => setToken(e.target.value)} data-testid="deploy-token" />
            </label>
            <div className="modal-action mt-0">
              <button type="button" className="btn btn-sm btn-ghost" onClick={onClose}>Cancel</button>
              <button type="submit" className="btn btn-sm btn-primary" disabled={!token.trim()}>Sign in</button>
            </div>
          </form>
        )}

        {step.kind === "not-allowed" && (
          <div className="flex flex-col gap-2 text-sm" data-testid="deploy-not-allowed">
            <p>{step.message}</p>
            <div className="modal-action mt-0"><button type="button" className="btn btn-sm btn-ghost" onClick={onClose}>Close</button></div>
          </div>
        )}

        {step.kind === "configure" && (
          <form className="flex flex-col gap-3 text-sm" onSubmit={(e) => { e.preventDefault(); void preview(); }}>
            <div className="flex flex-wrap gap-3">
              <label className="flex flex-col gap-1">
                <span>Application name</span>
                <input className="input input-sm w-60 font-mono" value={appName} onChange={(e) => setAppName(e.target.value)} aria-invalid={!!nameError} aria-describedby="deploy-app-hint" data-testid="deploy-app" />
                <span id="deploy-app-hint" className={`text-xs ${nameError ? "text-error" : "text-base-content/60"}`}>{nameError ?? "Instances are named <application>-<appliance>."}</span>
              </label>
              <label className="flex flex-col gap-1">
                <span>Project</span>
                <select className="select select-sm w-40" value={project} onChange={(e) => setProject(e.target.value)} data-testid="deploy-project">
                  {me?.deployProjects.map((p) => <option key={p} value={p}>{p}</option>)}
                </select>
              </label>
            </div>
            <ul className="divide-y divide-base-300 rounded-box border border-base-300" data-testid="deploy-appliances">
              {split.deployable.map((d) => (
                <li key={d.appliance} className="flex items-center justify-between gap-3 px-3 py-2" data-testid="deploy-deployable">
                  <span className="font-medium">{d.name}{d.replicas > 1 && <span className="ml-1.5 text-xs font-normal text-base-content/60" data-testid="deploy-replicas">× {d.replicas} replicas</span>}</span>
                  <span className="truncate font-mono text-xs text-base-content/60">{d.spec.source.alias}</span>
                </li>
              ))}
              {split.designOnly.map((d) => (
                <li key={d.appliance} className="flex items-center justify-between gap-3 px-3 py-2 text-base-content/60" data-testid="deploy-design-only">
                  <span>{d.name}</span>
                  <span className="text-xs">Design only: {d.reason}</span>
                </li>
              ))}
            </ul>
            {error && <p role="alert" className="text-error" data-testid="deploy-error">{error}</p>}
            <div className="modal-action mt-0">
              <button type="button" className="btn btn-sm btn-ghost" onClick={onClose}>Cancel</button>
              <button type="submit" className="btn btn-sm btn-primary" disabled={busy || !!nameError || !appName || split.deployable.length === 0} data-testid="deploy-preview">
                {busy ? "Checking…" : `Preview changes (${split.deployable.length} deployable)`}
              </button>
            </div>
          </form>
        )}

        {step.kind === "review" && (
          <div className="flex flex-col gap-3 text-sm">
            <p className="text-base-content/70">What deploying <strong className="font-mono">{appName}</strong> to <strong>{project}</strong> changes. Nothing has changed yet.</p>
            <PlanTable plan={step.plan} removeMissing={removeMissing} />
            <NetworkRules plan={compileNetwork(api.getState().assembly, appName)} enforced={!step.plan.warnings.some((w) => w.includes("not enforced"))} published={published()} />
            {step.plan.warnings.map((w) => <p key={w} className="flex items-start gap-2" role="note"><span className="status status-warning mt-1.5 shrink-0" aria-hidden="true" />{w}</p>)}
            {step.plan.items.some((i) => i.action === "remove") && (
              <label className="flex items-center gap-2">
                <input type="checkbox" className="checkbox checkbox-sm" checked={removeMissing} onChange={(e) => setRemoveMissing(e.target.checked)} data-testid="deploy-remove-missing" />
                <span>Also delete the instances no longer in the design</span>
              </label>
            )}
            {conflicts(step.plan) && <p role="alert" className="text-error" data-testid="deploy-conflicts">Resolve the conflicts first: rename the appliance or the application. Nothing is deployed while any remain.</p>}
            {error && <p role="alert" className="text-error" data-testid="deploy-error">{error}</p>}
            <div className="modal-action mt-0">
              <button type="button" className="btn btn-sm btn-ghost" onClick={() => setStep({ kind: "configure" })}>Back</button>
              <button type="button" className="btn btn-sm btn-primary" disabled={busy || conflicts(step.plan)} onClick={() => void apply(step.plan)} data-testid="deploy-apply">
                {busy ? "Deploying…" : changes(step.plan) ? `Deploy ${changes(step.plan)} change${changes(step.plan) === 1 ? "" : "s"}` : "Save revision (no changes)"}
              </button>
            </div>
          </div>
        )}

        {step.kind === "applying" && (
          <div className="flex flex-col gap-3 text-sm" data-testid="deploy-progress">
            <p className="text-base-content/70">Revision {step.revision} saved. The canvas shows each appliance's state while this design is open.</p>
            {step.jobs.length === 0 ? <p>Nothing to change.</p> : (
              <ul className="divide-y divide-base-300 rounded-box border border-base-300">
                {step.jobs.map((j) => (
                  <li key={j.instance} className="flex flex-wrap items-center justify-between gap-2 px-3 py-2" data-testid="deploy-job">
                    <span><span className="font-mono">{j.instance}</span> · {ACTION[j.action].label}</span>
                    <span className={j.job?.status === "failed" ? "text-error" : "text-base-content/70"}>
                      {j.job ? `${j.job.status}${j.job.attempts > 1 ? ` (attempt ${j.job.attempts})` : ""}${j.job.lastError ? `: ${j.job.lastError}` : ""}` : "queued"}
                    </span>
                  </li>
                ))}
              </ul>
            )}
            {error && <p role="alert" className="text-error">{error}</p>}
            <div className="modal-action mt-0"><button type="button" className="btn btn-sm" onClick={onClose} data-testid="deploy-done">Close</button></div>
          </div>
        )}
      </div>
    </dialog>
  );
}

/** The connections the design declares, as allow rules by service name, and whether they are enforced. */
function NetworkRules({ plan, enforced, published }: { plan: NetworkPlan; enforced: boolean; published: PublishedPort[] }) {
  if (!plan.intents.length) return null;
  return (
    <details className="rounded-box border border-base-300 px-3 py-2" data-testid="network-rules">
      <summary className="cursor-pointer">Connections in this design ({plan.intents.length})</summary>
      <p className="mt-2 text-xs text-base-content/60">
        Each consumer finds its provider in the environment variable shown ($APELORA_SERVICE_…).{" "}
        {enforced
          ? "Each cable allows one service; everything else between the application's instances is rejected. Only cables between deployed appliances apply."
          : "Each cable allows one service, by name. The control plane does not enforce them: instances share the project's network."}
      </p>
      <ul className="mt-2 flex flex-col gap-1 font-mono text-xs">
        {plan.intents.map((i) => (
          <li key={i.id} data-testid="network-rule">{i.from} → {i.service} · {i.protocol} {i.number ?? "any"}/{i.transport} <span className="text-base-content/60">· ${serviceEnvName(i.to)}</span></li>
        ))}
      </ul>
      {published.length > 0 && (
        <>
          <p className="mt-2 text-xs text-base-content/60">Reachable from outside the application (connected from a gateway):</p>
          <ul className="mt-1 flex flex-col gap-1 font-mono text-xs">
            {published.map((p) => <li key={`${p.appliance}/${p.port}`} data-testid="published-port">{p.appliance} · {p.port}/{p.transport}</li>)}
          </ul>
        </>
      )}
      {plan.warnings.map((w) => <p key={w} className="mt-2 text-xs">{w}</p>)}
    </details>
  );
}

function PlanTable({ plan, removeMissing }: { plan: Plan; removeMissing: boolean }) {
  return (
    <div className="max-h-64 overflow-auto rounded-box border border-base-300">
      <table className="table table-sm">
        <thead><tr><th>Appliance</th><th>Instance</th><th>Change</th><th>Why</th></tr></thead>
        <tbody>
          {plan.items.map((i) => (
            <tr key={i.instance} data-testid="plan-row" data-action={i.action}>
              <td>{i.appliance}</td>
              <td className="font-mono text-xs">{i.instance}</td>
              <td className="whitespace-nowrap"><span className={`status ${ACTION[i.action].tone} mr-2`} aria-hidden="true" />{i.action === "remove" && !removeMissing ? "Left running" : ACTION[i.action].label}</td>
              <td className="text-base-content/70">{i.reason}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

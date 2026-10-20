import { useMemo, useState } from "react";

import type { Instance, InstanceList, Job, WhoAmI } from "@apelora/api-client";
import { APPLIANCE_CLASSES, logoUrl } from "@apelora/appliance-catalog";
import { instanceRequestFor, type ApplianceClass } from "@apelora/assembly-model";

import { ConfirmDialog, ErrorPanel, FormDialog, Section, SectionHeader, TableFrame } from "../components/ui";
import { client, followJob, idempotencyKey, messageOf, useLoad } from "../lib/api";

const KIND_LABEL: Record<Instance["kind"], string> = { container: "Container", "virtual-machine": "Virtual machine", unknown: "Unknown" };
const NAME = /^[A-Za-z](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?$/;

/** What is happening to one instance after an action was requested. */
type Pending = { label: string; job?: Job; error?: string };
const key = (project: string, name: string) => `${project}/${name}`;

const DEPLOYABLE = APPLIANCE_CLASSES.filter((c) => c.runtime);

/** The choices of the setting that picks the image (e.g. a version), with what each runs. */
function imageChoices(cls: ApplianceClass): { property?: string; label?: string; options: { value: string; label: string }[] } {
  const by = cls.runtime?.imageBy;
  if (!by) return { options: [] };
  const prop = cls.properties?.find((p) => p.id === by.property);
  return {
    property: by.property,
    label: prop?.label ?? by.property,
    options: Object.entries(by.images).map(([value, img]) => ({
      value,
      label: "version" in img && img.version ? `${value} (${img.version})` : value,
    })),
  };
}

function DeployDialog({ open, projects, onClose, onQueued }: { open: boolean; projects: string[]; onClose: () => void; onQueued: (target: string, label: string, jobId: string) => void }) {
  const [classId, setClassId] = useState(DEPLOYABLE[0]?.id ?? "");
  const cls = DEPLOYABLE.find((c) => c.id === classId);
  const choices = useMemo(() => (cls ? imageChoices(cls) : { options: [] }), [cls]);
  const [setting, setSetting] = useState<string>("");
  const [name, setName] = useState("");
  const [project, setProject] = useState(projects[0] ?? "");
  const [start, setStart] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const chosen = setting || choices.options[0]?.value || "";
  const nameError = name && !NAME.test(name) ? "Use 1–63 letters, digits and hyphens, starting with a letter." : null;

  const submit = async () => {
    if (!cls) return;
    setError(null);
    const config = choices.property ? { [choices.property]: chosen } : {};
    const r = instanceRequestFor(cls, { config }, { project, name, start });
    if (!r.ok) { setError(r.error); return; }
    setBusy(true);
    try {
      const res = await client.request("create_instance", { body: r.request, idempotencyKey: idempotencyKey() });
      onQueued(key(project, name), `Creating ${cls.displayName}`, res.jobId);
      setName("");
      onClose();
    } catch (e) {
      setError(messageOf(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <FormDialog open={open} title="Deploy an appliance" onClose={onClose} testId="deploy-dialog">
      <form className="flex flex-col gap-3" onSubmit={(e) => { e.preventDefault(); void submit(); }}>
        <p className="text-sm text-base-content/70">Lists appliances whose images have been tested on a cluster.</p>
        <label className="flex flex-col gap-1 text-sm">
          <span>Appliance</span>
          <select className="select select-sm w-full" value={classId} onChange={(e) => { setClassId(e.target.value); setSetting(""); }} data-testid="deploy-class">
            {DEPLOYABLE.map((c) => <option key={c.id} value={c.id}>{c.displayName}</option>)}
          </select>
        </label>
        {cls && (
          <div className="flex items-center gap-2 text-xs text-base-content/70">
            {logoUrl(cls.iconAssetPath) && <img src={logoUrl(cls.iconAssetPath)} alt="" className="size-5" />}
            <span>{cls.summary} Runs as {cls.runtime?.kind === "incus-oci-container" ? "an application container" : cls.runtime?.kind === "incus-vm" ? "a virtual machine" : "a system container"}.</span>
          </div>
        )}
        {choices.options.length > 0 && (
          <label className="flex flex-col gap-1 text-sm">
            <span>{choices.label}</span>
            <select className="select select-sm w-full" value={chosen} onChange={(e) => setSetting(e.target.value)} data-testid="deploy-setting">
              {choices.options.map((o) => <option key={o.value} value={o.value}>{o.label}</option>)}
            </select>
          </label>
        )}
        <label className="flex flex-col gap-1 text-sm">
          <span>Instance name</span>
          <input className="input input-sm w-full font-mono" value={name} onChange={(e) => setName(e.target.value)} placeholder="web-01" autoComplete="off" spellCheck={false} aria-invalid={!!nameError} aria-describedby="deploy-name-hint" data-testid="deploy-name" />
          <span id="deploy-name-hint" className={`text-xs ${nameError ? "text-error" : "text-base-content/60"}`}>{nameError ?? "Letters, digits and hyphens; must be unique in the project."}</span>
        </label>
        <label className="flex flex-col gap-1 text-sm">
          <span>Project</span>
          <select className="select select-sm w-full" value={project} onChange={(e) => setProject(e.target.value)} data-testid="deploy-project">
            {projects.map((p) => <option key={p} value={p}>{p}</option>)}
          </select>
        </label>
        <label className="flex items-center gap-2 text-sm">
          <input type="checkbox" className="checkbox checkbox-sm" checked={start} onChange={(e) => setStart(e.target.checked)} />
          <span>Start it once created</span>
        </label>
        {error && <ErrorPanel message={error} testId="deploy-error" />}
        <div className="modal-action mt-0">
          <button type="button" className="btn btn-sm btn-ghost" onClick={onClose}>Cancel</button>
          <button type="submit" className="btn btn-sm btn-primary" disabled={busy || !name || !!nameError || !project} data-testid="deploy-submit">
            {busy ? "Queuing…" : "Deploy"}
          </button>
        </div>
      </form>
    </FormDialog>
  );
}

export function InstancesView({ me, onAuth }: { me: WhoAmI | null; onAuth: (m: string) => void }) {
  const [list, reload] = useLoad<InstanceList>(() => client.get("instances"), onAuth);
  const [pending, setPending] = useState<Record<string, Pending>>({});
  const [deploying, setDeploying] = useState(false);
  const [toDelete, setToDelete] = useState<Instance | null>(null);
  const canDeploy = !!me?.permissions.includes("deploy");
  const projects = me?.deployProjects ?? [];

  const track = (target: string, label: string, jobId: string) => {
    setPending((p) => ({ ...p, [target]: { label } }));
    followJob(jobId, (job) => setPending((p) => ({ ...p, [target]: { label, job } })))
      .then((job) => {
        if (job.status === "succeeded") setPending((p) => { const { [target]: _, ...rest } = p; return rest; });
        else setPending((p) => ({ ...p, [target]: { label, job, error: job.lastError ?? `The job ${job.status}.` } }));
        reload();
      })
      .catch((e: unknown) => setPending((p) => ({ ...p, [target]: { label, error: messageOf(e) } })));
  };

  const act = async (i: Instance, action: "start" | "stop" | "delete") => {
    const target = key(i.project, i.name);
    const label = action === "start" ? "Starting" : action === "stop" ? "Stopping" : "Deleting";
    setPending((p) => ({ ...p, [target]: { label } }));
    try {
      const params = { project: i.project, name: i.name };
      const res = action === "delete"
        ? await client.request("delete_instance", { params, idempotencyKey: idempotencyKey() })
        : await client.request("change_state", { params, body: { action }, idempotencyKey: idempotencyKey() });
      track(target, label, res.jobId);
    } catch (e) {
      setPending((p) => ({ ...p, [target]: { label, error: messageOf(e) } }));
    }
  };

  const queuedOnly = Object.entries(pending).filter(([t]) => list.kind === "ok" && !list.data.items.some((i) => key(i.project, i.name) === t));

  return (
    <div className="flex flex-col gap-6">
      <Section title="Instances" level={1} state={list} testId="instances" onRetry={reload}>
        {(l) => (
          <>
            <SectionHeader title="Instances" level={1} count={l.items.length} source={l.source}>
              {canDeploy && (
                <button type="button" className="btn btn-sm btn-primary" onClick={() => setDeploying(true)} disabled={projects.length === 0} data-testid="deploy-open">
                  Deploy appliance
                </button>
              )}
            </SectionHeader>
            {queuedOnly.map(([target, p]) => (
              <PendingNote key={target} target={target} pending={p} />
            ))}
            {l.items.length === 0 ? (
              <p className="text-sm text-base-content/70">No instances yet.{canDeploy ? " Deploy an appliance to create one." : ""}</p>
            ) : (
              <TableFrame>
                <thead><tr><th>Instance</th><th className="max-sm:hidden">Kind</th><th>Status</th><th className="max-sm:hidden">Member</th><th>Project</th><th><span className="sr-only">Actions</span></th></tr></thead>
                <tbody>
                  {l.items.map((i) => {
                    const target = key(i.project, i.name);
                    const p = pending[target];
                    const actionable = canDeploy && i.managed && projects.includes(i.project);
                    return (
                      <tr key={target} data-testid="instance-row">
                        <td className="font-medium">
                          {i.name}
                          {i.managed && <span className="badge badge-xs badge-ghost ml-2" title="Created by Apelora">Apelora</span>}
                        </td>
                        <td className="max-sm:hidden">{KIND_LABEL[i.kind]}</td>
                        <td>{p && !p.error ? <span aria-live="polite">{p.label}…{p.job && p.job.attempts > 1 ? ` (attempt ${p.job.attempts})` : ""}</span> : i.status}</td>
                        <td className="max-sm:hidden">{i.location || "—"}</td>
                        <td>{i.project}</td>
                        <td className="text-right">
                          {p?.error && <span className="mr-2 text-xs text-error" role="alert" data-testid="instance-error">{p.error}</span>}
                          {actionable ? (
                            <span className="inline-flex gap-1">
                              {i.status === "Running"
                                ? <button type="button" className="btn btn-xs btn-ghost" aria-label={`Stop ${i.name}`} disabled={!!p && !p.error} onClick={() => void act(i, "stop")}>Stop</button>
                                : <button type="button" className="btn btn-xs btn-ghost" aria-label={`Start ${i.name}`} disabled={!!p && !p.error} onClick={() => void act(i, "start")}>Start</button>}
                              <button type="button" className="btn btn-xs btn-ghost text-error" aria-label={`Delete ${i.name}`} disabled={!!p && !p.error} onClick={() => setToDelete(i)}>Delete</button>
                            </span>
                          ) : (
                            !i.managed && <span className="text-xs text-base-content/50" title="Apelora changes only instances it created.">Not managed by Apelora</span>
                          )}
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </TableFrame>
            )}
          </>
        )}
      </Section>

      {canDeploy && <DeployDialog open={deploying} projects={projects} onClose={() => setDeploying(false)} onQueued={track} />}
      <ConfirmDialog
        open={!!toDelete}
        title={`Delete ${toDelete?.name ?? ""}?`}
        body={<p>The instance and its root disk are removed from Incus. A running instance is stopped first. This cannot be undone.</p>}
        action="Delete instance"
        typeToConfirm={toDelete?.name}
        onCancel={() => setToDelete(null)}
        onConfirm={() => { const i = toDelete; setToDelete(null); if (i) void act(i, "delete"); }}
      />
    </div>
  );
}

function PendingNote({ target, pending }: { target: string; pending: Pending }) {
  return (
    <div className={`rounded-box border px-4 py-3 text-sm ${pending.error ? "border-error/40" : "border-base-300"}`} data-testid="pending-deploy" aria-live="polite">
      <span className="font-medium">{target}</span>{" "}
      {pending.error ? <span className="text-error">could not be created: {pending.error}</span>
        : <span className="text-base-content/70">{pending.label}… {pending.job ? `job ${pending.job.status}${pending.job.attempts > 1 ? `, attempt ${pending.job.attempts}` : ""}` : "queued"}</span>}
    </div>
  );
}

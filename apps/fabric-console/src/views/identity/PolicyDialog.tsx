import { useId, useState } from "react";

import type { EnvKind, PermissionDto, PolicyDto } from "@apelora/api-client";

import { ErrorPanel, FormDialog } from "../../components/ui";
import { client } from "../../lib/api";
import { ENV_KINDS, ENV_LABEL, useRun } from "../../lib/identity";

const asStrings = (v: unknown): string[] => (Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : []);

/** The policy's parameters in words, for the list. */
export function summarize(p: PolicyDto): string {
  if (p.kind === "mfa") {
    const envs = asStrings(p.parameters.environments);
    const parts = [
      envs.length > 0 ? `actions in ${envs.map((e) => ENV_LABEL[e as EnvKind]?.toLowerCase() ?? e).join(", ")} environments` : "",
      p.parameters.infrastructure === true ? "infrastructure actions" : "",
    ].filter(Boolean);
    return parts.length > 0 ? `Needs multi-factor sign-in for ${parts.join(" and ")}.` : "No environments or infrastructure selected.";
  }
  const actions = asStrings(p.parameters.actions);
  return actions.length > 0 ? `Extra actions that need approval: ${actions.join(", ")}.` : "No actions listed.";
}

function Form({ existing, catalogue, onAuth, onClose, onSaved }: { existing: PolicyDto | null; catalogue: PermissionDto[]; onAuth: (m: string) => void; onClose: () => void; onSaved: (p: PolicyDto) => void }) {
  const [kind, setKind] = useState<"mfa" | "approval">(existing?.kind === "approval" ? "approval" : "mfa");
  const [name, setName] = useState(existing?.name ?? "");
  const [enabled, setEnabled] = useState(existing?.enabled ?? true);
  const [envs, setEnvs] = useState<string[]>(asStrings(existing?.parameters.environments));
  const [infra, setInfra] = useState(existing?.parameters.infrastructure === true);
  const [actions, setActions] = useState<string[]>(asStrings(existing?.parameters.actions));
  const [filter, setFilter] = useState("");
  const { busy, error, run } = useRun(onAuth);
  const nameId = useId();
  const kindId = useId();
  const filterId = useId();
  const nameOk = name.trim().length >= 1 && name.trim().length <= 80;
  const paramsOk = kind === "mfa" ? envs.length > 0 || infra : actions.length > 0;
  const q = filter.trim().toLowerCase();
  // Actions that always need approval are already covered; a policy adds the others.
  const candidates = catalogue.filter((p) => !p.needsApproval && (!q || p.id.includes(q) || p.description.toLowerCase().includes(q)));
  const submit = () => void run(async () => onSaved(await client.request("save_policy", {
    body: { name: name.trim(), kind, enabled, parameters: kind === "mfa" ? { environments: envs, infrastructure: infra } : { actions } },
  })));
  return (
    <form className="flex flex-col gap-3" onSubmit={(e) => { e.preventDefault(); if (nameOk && paramsOk && !busy) submit(); }}>
      <div className="grid gap-3 sm:grid-cols-2">
        <div className="flex flex-col gap-1 text-sm">
          <label htmlFor={kindId}>Kind</label>
          <select id={kindId} className="select select-sm w-full" value={kind} onChange={(e) => setKind(e.target.value as "mfa" | "approval")} disabled={!!existing} data-testid="policy-kind">
            <option value="mfa">Multi-factor sign-in</option>
            <option value="approval">Approval</option>
          </select>
        </div>
        <div className="flex flex-col gap-1 text-sm">
          <label htmlFor={nameId}>Name</label>
          <input id={nameId} className="input input-sm w-full" value={name} onChange={(e) => setName(e.target.value)} readOnly={!!existing} maxLength={80} autoComplete="off" data-testid="policy-name" />
          {existing && <span className="text-xs text-base-content/60">Saving with the same name updates this policy.</span>}
        </div>
      </div>

      {kind === "mfa" ? (
        <fieldset className="flex flex-col gap-2 rounded-box border border-base-300 p-3">
          <legend className="px-1 text-sm font-medium">Require multi-factor sign-in for</legend>
          {ENV_KINDS.map((k) => (
            <label key={k} className="flex items-center gap-2 text-sm">
              <input type="checkbox" className="checkbox checkbox-xs" checked={envs.includes(k)} onChange={(e) => setEnvs(e.target.checked ? [...envs, k] : envs.filter((x) => x !== k))} data-testid={`policy-env-${k}`} />
              Actions in {ENV_LABEL[k].toLowerCase()} environments
            </label>
          ))}
          <label className="flex items-center gap-2 text-sm">
            <input type="checkbox" className="checkbox checkbox-xs" checked={infra} onChange={(e) => setInfra(e.target.checked)} data-testid="policy-infra" />
            Infrastructure actions (cluster, networks, storage, hardware)
          </label>
        </fieldset>
      ) : (
        <div className="flex flex-col gap-2">
          <p className="text-sm text-base-content/70">Choose extra actions that need another person&apos;s approval, where the API already asks for it. Actions that always need approval are not listed.</p>
          <div className="flex flex-col gap-1 text-sm">
            <label htmlFor={filterId}>Filter actions</label>
            <input id={filterId} type="search" className="input input-sm w-full max-w-sm" value={filter} onChange={(e) => setFilter(e.target.value)} autoComplete="off" data-testid="policy-filter" />
          </div>
          <div className="flex max-h-56 flex-col gap-1 overflow-y-auto rounded-box border border-base-300 p-3" tabIndex={0} aria-label="Actions that need approval" data-testid="policy-actions">
            {candidates.length === 0 && <p className="text-sm text-base-content/60">No actions match.</p>}
            {candidates.map((p) => (
              <label key={p.id} className="flex items-start gap-2 text-sm">
                <input type="checkbox" className="checkbox checkbox-xs mt-0.5" checked={actions.includes(p.id)} onChange={(e) => setActions(e.target.checked ? [...actions, p.id] : actions.filter((x) => x !== p.id))} data-testid={`policy-action-${p.id}`} />
                <span><span className="font-mono text-xs">{p.id}</span> <span className="text-base-content/70">{p.description}</span></span>
              </label>
            ))}
          </div>
          <span className="text-xs text-base-content/60">{actions.length} selected.</span>
        </div>
      )}
      {!paramsOk && <p className="text-xs text-error" role="alert">{kind === "mfa" ? "Choose at least one environment kind or infrastructure." : "Choose at least one action."}</p>}

      <label className="flex items-center gap-2 text-sm">
        <input type="checkbox" className="checkbox checkbox-xs" checked={enabled} onChange={(e) => setEnabled(e.target.checked)} data-testid="policy-enabled" />
        Enabled
      </label>
      {error && <ErrorPanel message={error} testId="policy-error" />}
      <div className="modal-action mt-0">
        <button type="button" className="btn btn-sm btn-ghost" onClick={onClose}>Cancel</button>
        <button type="submit" className="btn btn-sm btn-primary" disabled={busy || !nameOk || !paramsOk} data-testid="policy-submit">{busy ? "Saving…" : "Save policy"}</button>
      </div>
    </form>
  );
}

/** `existing` null creates a policy; otherwise it edits that one (the name stays). */
export function PolicyDialog({ open, existing, catalogue, onAuth, onClose, onSaved }: { open: boolean; existing: PolicyDto | null; catalogue: PermissionDto[]; onAuth: (m: string) => void; onClose: () => void; onSaved: (p: PolicyDto) => void }) {
  return (
    <FormDialog open={open} title={existing ? `Edit ${existing.name}` : "Create an access policy"} onClose={onClose} testId="policy-dialog" wide>
      {open && <Form existing={existing} catalogue={catalogue} onAuth={onAuth} onClose={onClose} onSaved={onSaved} />}
    </FormDialog>
  );
}

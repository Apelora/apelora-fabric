import { useEffect, useId, useState } from "react";

import { ApiRequestError, type AssignmentPreview, type EnvKind, type NewAssignment, type PrincipalRef, type RoleDto, type Scope, type WhoAmI } from "@apelora/api-client";

import { ErrorPanel, FormDialog, Notice } from "../../components/ui";
import { client, useLoad } from "../../lib/api";
import {
  ENV_KINDS, ENV_LABEL, LEVEL_LABEL, UUID, describeError, personName, useDebounced, useDirectory, useRun, type Directory,
} from "../../lib/identity";
import { PreviewPanel } from "./AssignmentPreview";
import { Picker } from "./Picker";
import { PersonLookup } from "./PersonLookup";

/** What the dialog starts with: usually a person, group or scope the administrator came from. */
export type AssignPreset = { principal?: { ref: PrincipalRef; label: string }; scope?: Scope };

const POOL_NAME = /^[A-Za-z0-9._-]{1,80}$/;
const SCOPE_TYPES: Scope["type"][] = ["platform", "organization", "project", "application", "pool"];

type PreviewState =
  | { key: string; status: "loading" }
  | { key: string; status: "ok"; data: AssignmentPreview }
  | { key: string; status: "error"; message: string };

function Segmented<T extends string>({ legend, value, options, onChange }: { legend: string; value: T; options: { value: T; label: string }[]; onChange: (v: T) => void }) {
  return (
    <fieldset className="flex flex-col gap-1">
      <legend className="mb-1 text-sm font-medium">{legend}</legend>
      <div className="flex flex-wrap gap-x-4 gap-y-1">
        {options.map((o) => (
          <label key={o.value} className="flex items-center gap-2 text-sm">
            <input type="radio" className="radio radio-xs" name={legend} checked={value === o.value} onChange={() => onChange(o.value)} />
            {o.label}
          </label>
        ))}
      </div>
    </fieldset>
  );
}

/** The local date-time field's value as an RFC 3339 instant, or why it cannot be one. */
function parseExpiry(value: string): { iso: string | null; error: string | null } {
  if (!value) return { iso: null, error: null };
  const t = new Date(value);
  if (Number.isNaN(t.getTime())) return { iso: null, error: "That date and time is not valid." };
  if (t.getTime() <= Date.now()) return { iso: null, error: "The expiry must be in the future." };
  return { iso: t.toISOString(), error: null };
}

function PrincipalFields({ ptype, setPtype, person, setPerson, groupId, setGroupId, serviceId, setServiceId, dir, onAuth }: {
  ptype: PrincipalRef["type"]; setPtype: (t: PrincipalRef["type"]) => void;
  person: { id: string; label: string } | null; setPerson: (p: { id: string; label: string } | null) => void;
  groupId: string; setGroupId: (v: string) => void; serviceId: string; setServiceId: (v: string) => void;
  dir: Directory; onAuth: (m: string) => void;
}) {
  const automation = dir.services.kind === "ok" ? { kind: "ok" as const, data: dir.services.data.filter((s) => s.apiAccess && !s.disabled) } : dir.services;
  return (
    <div className="flex flex-col gap-3">
      <Segmented legend="Who" value={ptype} onChange={setPtype} options={[{ value: "user", label: "A person" }, { value: "group", label: "A group" }, { value: "service", label: "An automation identity" }]} />
      {ptype === "user" && (person ? (
        <div className="flex flex-wrap items-center gap-2 text-sm" data-testid="assignment-person">
          <span>Person: <strong>{person.label}</strong></span>
          <button type="button" className="btn btn-xs btn-ghost" onClick={() => setPerson(null)}>Change</button>
        </div>
      ) : (
        <PersonLookup onAuth={onAuth} label="Find the person" actionLabel="Choose" testId="assignment-lookup" onPick={(u) => setPerson({ id: u.id, label: personName(u) })} />
      ))}
      {ptype === "group" && (
        <Picker label="Group" data={dir.groups} value={groupId} onChange={setGroupId} id={(g) => g.id} text={(g) => g.name} testId="assignment-group" offMessage="You may not list groups." emptyMessage="There are no groups yet. Create one on the Groups page.">
          <span className="text-xs text-base-content/60">An organization&apos;s group can only be given roles inside that organization.</span>
        </Picker>
      )}
      {ptype === "service" && (
        <Picker label="Automation identity" data={automation} value={serviceId} onChange={setServiceId} id={(s) => s.id} text={(s) => s.name} testId="assignment-service" offMessage="Listing automation identities needs the service_identity.manage permission." emptyMessage="There are no enabled automation identities. Create one on the Service identities page." />
      )}
    </div>
  );
}

function ScopeFields({ scopeType, setScopeType, orgId, setOrgId, projectId, setProjectId, appId, setAppId, pool, setPool, dir, role }: {
  scopeType: Scope["type"]; setScopeType: (t: Scope["type"]) => void;
  orgId: string; setOrgId: (v: string) => void; projectId: string; setProjectId: (v: string) => void;
  appId: string; setAppId: (v: string) => void; pool: string; setPool: (v: string) => void;
  dir: Directory; role: RoleDto | undefined;
}) {
  const typeId = useId();
  const appInput = useId();
  const poolInput = useId();
  const allowed = role?.scopes.includes(scopeType);
  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-col gap-1 text-sm">
        <label htmlFor={typeId}>Where</label>
        <select id={typeId} className="select select-sm w-full" value={scopeType} onChange={(e) => setScopeType(e.target.value as Scope["type"])} data-testid="assignment-scope-type">
          {SCOPE_TYPES.map((t) => <option key={t} value={t}>{LEVEL_LABEL[t]}</option>)}
        </select>
        {role && !allowed && <span className="text-xs text-base-content/60">{role.name} cannot be assigned at this level; the preview shows the server&apos;s answer.</span>}
      </div>
      {scopeType === "platform" && <p className="text-xs text-base-content/60">The whole platform: everything below it as well.</p>}
      {scopeType === "organization" && (
        <Picker label="Organization" data={dir.orgs} value={orgId} onChange={setOrgId} id={(o) => o.id} text={(o) => o.name} testId="assignment-org" offMessage="You may not list organizations." emptyMessage="There are no organizations yet." />
      )}
      {scopeType === "project" && (
        <Picker label="Project" data={dir.projects} value={projectId} onChange={setProjectId} id={(p) => p.id} text={(p) => `${p.organizationSlug} / ${p.name}`} testId="assignment-project" offMessage="You may not list projects." emptyMessage="There are no projects yet." />
      )}
      {scopeType === "application" && (
        <div className="flex flex-col gap-1 text-sm">
          <label htmlFor={appInput}>Application id</label>
          <input id={appInput} className="input input-sm w-full font-mono" value={appId} onChange={(e) => setAppId(e.target.value)} placeholder="00000000-0000-0000-0000-000000000000" autoComplete="off" spellCheck={false} aria-invalid={!!appId.trim() && !UUID.test(appId.trim())} data-testid="assignment-app" />
          <span className="text-xs text-base-content/60">The API does not list application ids yet, so paste the id of the application.</span>
        </div>
      )}
      {scopeType === "pool" && (
        <div className="flex flex-col gap-1 text-sm">
          <label htmlFor={poolInput}>Hardware pool name</label>
          <input id={poolInput} className="input input-sm w-full font-mono" value={pool} onChange={(e) => setPool(e.target.value)} placeholder="rack-a" autoComplete="off" spellCheck={false} aria-invalid={!!pool && !POOL_NAME.test(pool)} data-testid="assignment-pool" />
          <span className="text-xs text-base-content/60">1–80 letters, digits, dots, underscores or hyphens.</span>
        </div>
      )}
    </div>
  );
}

function AssignmentForm({ preset, onAuth, onClose, onSaved, dir }: { preset: AssignPreset; onAuth: (m: string) => void; onClose: () => void; onSaved: (message: string) => void; dir: Directory }) {
  const [roles] = useLoad<RoleDto[]>(() => client.get("roles").then((r) => r.items), onAuth);
  const p = preset.principal?.ref;
  const s = preset.scope;
  const [ptype, setPtype] = useState<PrincipalRef["type"]>(p?.type ?? "user");
  const [person, setPerson] = useState<{ id: string; label: string } | null>(p?.type === "user" ? { id: p.id, label: preset.principal!.label } : null);
  const [groupId, setGroupId] = useState(p?.type === "group" ? p.id : "");
  const [serviceId, setServiceId] = useState(p?.type === "service" ? p.id : "");
  const [roleId, setRoleId] = useState("");
  const [scopeType, setScopeType] = useState<Scope["type"]>(s?.type ?? "organization");
  const [orgId, setOrgId] = useState(s?.type === "organization" ? s.id : "");
  const [projectId, setProjectId] = useState(s?.type === "project" ? s.id : "");
  const [appId, setAppId] = useState(s?.type === "application" ? s.id : "");
  const [pool, setPool] = useState(s?.type === "pool" ? s.name : "");
  const [requireMfa, setRequireMfa] = useState(false);
  const [envs, setEnvs] = useState<EnvKind[]>([]);
  const [expires, setExpires] = useState("");
  const [reason, setReason] = useState("");
  const { busy, error, run } = useRun(onAuth);
  const roleSel = useId();
  const expiresId = useId();
  const reasonId = useId();

  const roleList = roles.kind === "ok" ? roles.data : [];
  const role = roleList.find((r) => r.key === roleId);

  const principal: PrincipalRef | null =
    ptype === "user" ? (person ? { type: "user", id: person.id } : null)
      : ptype === "group" ? (groupId ? { type: "group", id: groupId } : null)
        : serviceId ? { type: "service", id: serviceId } : null;
  const scope: Scope | null =
    scopeType === "platform" ? { type: "platform" }
      : scopeType === "organization" ? (orgId ? { type: "organization", id: orgId } : null)
        : scopeType === "project" ? (projectId ? { type: "project", id: projectId } : null)
          : scopeType === "application" ? (UUID.test(appId.trim()) ? { type: "application", id: appId.trim().toLowerCase() } : null)
            : POOL_NAME.test(pool) ? { type: "pool", name: pool } : null;
  const expiry = parseExpiry(expires);
  const body: NewAssignment | null = principal && role && scope && !expiry.error
    ? { principal, role: role.reference, scope, conditions: { requireMfa, environments: envs.length > 0 ? envs : null }, expiresAt: expiry.iso, reason: reason.trim() || null }
    : null;
  const bodyKey = body ? JSON.stringify(body) : "";
  const settled = useDebounced(bodyKey, 350);

  const [preview, setPreview] = useState<PreviewState | null>(null);
  useEffect(() => {
    if (!settled) { setPreview(null); return; }
    let current = true;
    setPreview({ key: settled, status: "loading" });
    client.request("preview_assignment", { body: JSON.parse(settled) as NewAssignment }).then(
      (data) => { if (current) setPreview({ key: settled, status: "ok", data }); },
      (e: unknown) => {
        if (!current) return;
        if (e instanceof ApiRequestError && e.unauthenticated) onAuth(e.message);
        setPreview({ key: settled, status: "error", message: describeError(e) });
      },
    );
    return () => { current = false; };
  }, [settled, onAuth]);

  const fresh = preview && preview.key === bodyKey && bodyKey !== "";
  const who = ptype === "user" ? person?.label : ptype === "group" ? dir.names.group(groupId) : dir.names.service(serviceId);
  const where = scope ? dir.names.scope(scope) : "this scope";
  const canSave = !!body && !!fresh && preview.status === "ok" && preview.data.allowed && !busy;

  const save = () => {
    if (!body || !role) return;
    void run(async () => {
      await client.request("create_assignment", { body });
      onSaved(`${role.name} assigned to ${who ?? "the selected principal"} at ${where}.`);
    });
  };

  const missing: string[] = [];
  if (!principal) missing.push("who");
  if (!role) missing.push("a role");
  if (!scope) missing.push("where");

  return (
    <div className="flex flex-col gap-4">
      <p className="text-sm text-base-content/70">Choose who gets which role where. Nothing is saved until you press Save, and the preview shows exactly what would change.</p>
      <PrincipalFields ptype={ptype} setPtype={setPtype} person={person} setPerson={setPerson} groupId={groupId} setGroupId={setGroupId} serviceId={serviceId} setServiceId={setServiceId} dir={dir} onAuth={onAuth} />

      <div className="flex flex-col gap-1 text-sm">
        <label htmlFor={roleSel}>Role</label>
        <select id={roleSel} className="select select-sm w-full" value={roleId} onChange={(e) => setRoleId(e.target.value)} disabled={roles.kind !== "ok"} data-testid="assignment-role">
          <option value="">{roles.kind === "loading" ? "Loading…" : "Choose a role…"}</option>
          <optgroup label="Predefined roles">{roleList.filter((r) => r.builtin).map((r) => <option key={r.key} value={r.key}>{r.name}</option>)}</optgroup>
          {roleList.some((r) => !r.builtin) && <optgroup label="Custom roles">{roleList.filter((r) => !r.builtin).map((r) => <option key={r.key} value={r.key}>{r.name}</option>)}</optgroup>}
        </select>
        {roles.kind === "error" && <ErrorPanel message={roles.message} />}
        {role && (
          <span className="text-xs text-base-content/60" data-testid="assignment-role-note">
            {role.description || "No description."} Can be assigned at: {role.scopes.map((l) => LEVEL_LABEL[l]).join(", ")}.
          </span>
        )}
      </div>

      <ScopeFields scopeType={scopeType} setScopeType={setScopeType} orgId={orgId} setOrgId={setOrgId} projectId={projectId} setProjectId={setProjectId} appId={appId} setAppId={setAppId} pool={pool} setPool={setPool} dir={dir} role={role} />

      <fieldset className="flex flex-col gap-2 rounded-box border border-base-300 p-3">
        <legend className="px-1 text-sm font-medium">Conditions (optional)</legend>
        <label className="flex items-center gap-2 text-sm">
          <input type="checkbox" className="checkbox checkbox-xs" checked={requireMfa} onChange={(e) => setRequireMfa(e.target.checked)} data-testid="assignment-mfa" />
          Only when the person signed in with multi-factor authentication
        </label>
        <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-sm">
          <span>Only in environments of kind:</span>
          {ENV_KINDS.map((k) => (
            <label key={k} className="flex items-center gap-1.5">
              <input type="checkbox" className="checkbox checkbox-xs" checked={envs.includes(k)} onChange={(e) => setEnvs(e.target.checked ? [...envs, k] : envs.filter((x) => x !== k))} data-testid={`assignment-env-${k}`} />
              {ENV_LABEL[k]}
            </label>
          ))}
        </div>
        <span className="text-xs text-base-content/60">Leave all environment kinds unticked for no restriction.</span>
      </fieldset>

      <div className="grid gap-3 sm:grid-cols-2">
        <div className="flex flex-col gap-1 text-sm">
          <label htmlFor={expiresId}>Expires (optional)</label>
          <input id={expiresId} type="datetime-local" className="input input-sm w-full" value={expires} onChange={(e) => setExpires(e.target.value)} aria-invalid={!!expiry.error} data-testid="assignment-expires" />
          {expiry.error && <span className="text-xs text-error" role="alert">{expiry.error}</span>}
        </div>
        <div className="flex flex-col gap-1 text-sm">
          <label htmlFor={reasonId}>Reason (optional)</label>
          <input id={reasonId} className="input input-sm w-full" value={reason} onChange={(e) => setReason(e.target.value)} maxLength={300} placeholder="Why this access is needed" autoComplete="off" data-testid="assignment-reason" />
        </div>
      </div>

      <div className="flex flex-col gap-3" data-testid="assignment-preview-area">
        {!body && <p className="text-sm text-base-content/60" data-testid="preview-waiting">{expiry.error ? "Fix the expiry to see the preview." : `Choose ${missing.join(", ")} to see what this would grant.`}</p>}
        {body && !(fresh && preview.status !== "loading") && <p className="text-sm text-base-content/60" role="status" data-testid="preview-loading">Checking what this would grant…</p>}
        {body && fresh && preview.status === "error" && <Notice tone="error" testId="preview-error">{preview.message}</Notice>}
        {body && preview?.status === "ok" && <PreviewPanel preview={preview.data} names={dir.names} who={who ?? "They"} scopeLabel={where} stale={!fresh} />}
      </div>

      {error && <ErrorPanel message={error} testId="assignment-error" />}
      <div className="modal-action sticky bottom-0 mt-0 border-t border-base-300 bg-base-100 pt-3">
        <button type="button" className="btn btn-sm btn-outline" onClick={onClose}>Cancel</button>
        <button type="button" className="btn btn-sm btn-primary" disabled={!canSave} onClick={save} data-testid="assignment-save">{busy ? "Saving…" : "Save assignment"}</button>
      </div>
    </div>
  );
}

/**
 * Gives a role to a person, group or automation identity at a scope. Before saving, the server's
 * preview says whether you may, what they can do now and after, and where each permission comes from.
 */
export function AssignmentDialog({ open, preset, me, onAuth, onClose, onSaved }: {
  open: boolean; preset: AssignPreset; me: WhoAmI | null; onAuth: (m: string) => void; onClose: () => void; onSaved: (message: string) => void;
}) {
  return (
    <FormDialog open={open} title="Assign a role" onClose={onClose} testId="assignment-dialog" wide>
      {open && <AssignmentBody preset={preset} me={me} onAuth={onAuth} onClose={onClose} onSaved={onSaved} />}
    </FormDialog>
  );
}

function AssignmentBody({ preset, me, onAuth, onClose, onSaved }: { preset: AssignPreset; me: WhoAmI | null; onAuth: (m: string) => void; onClose: () => void; onSaved: (message: string) => void }) {
  const dir = useDirectory(me, onAuth, { orgs: true, projects: true, groups: true, services: true });
  return <AssignmentForm preset={preset} onAuth={onAuth} onClose={onClose} onSaved={onSaved} dir={dir} />;
}

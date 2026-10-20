import { useId, useMemo, useState } from "react";

import type { PermissionDto, RoleDto, WhoAmI } from "@apelora/api-client";

import { Badge, ErrorPanel, FormDialog } from "../../components/ui";
import { client } from "../../lib/api";
import { CLASS_LABEL, CLASS_NOTE, CLASS_ORDER, LEVEL_LABEL, customRoleLevels, holdsPlatform, useRun } from "../../lib/identity";

const NAME_OK = /^[\p{L}\p{N} _-]+$/u;

function Form({ catalogue, roles, me, onAuth, onClose, onCreated }: { catalogue: PermissionDto[]; roles: RoleDto[]; me: WhoAmI | null; onAuth: (m: string) => void; onClose: () => void; onCreated: (r: RoleDto) => void }) {
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [picked, setPicked] = useState<string[]>([]);
  const [filter, setFilter] = useState("");
  const { busy, error, run } = useRun(onAuth);
  const nameId = useId();
  const descId = useId();
  const filterId = useId();
  const trimmed = name.trim();
  const taken = roles.some((r) => r.name.toLowerCase() === trimmed.toLowerCase() || (r.reference.type === "builtin" && r.reference.key === trimmed.toLowerCase()));
  const nameError = !trimmed ? null : trimmed.length > 60 ? "At most 60 characters." : !NAME_OK.test(trimmed) ? "Letters, digits, spaces, hyphens and underscores only." : taken ? "A role with that name already exists." : null;
  const nameOk = !!trimmed && !nameError;
  const levels = useMemo(() => customRoleLevels(picked, catalogue), [picked, catalogue]);
  const q = filter.trim().toLowerCase();
  const toggle = (id: string, on: boolean) => setPicked((p) => (on ? [...p, id] : p.filter((x) => x !== id)));
  const submit = () => void run(async () => onCreated(await client.request("create_role", { body: { name: trimmed, description: description.trim(), permissions: [...picked].sort() } })));
  return (
    <form className="flex flex-col gap-4" onSubmit={(e) => { e.preventDefault(); if (nameOk && picked.length > 0 && !busy) submit(); }}>
      <p className="text-sm text-base-content/70">A custom role is exactly the permissions you tick. You can only include permissions you hold yourself.</p>
      <div className="grid gap-3 sm:grid-cols-2">
        <div className="flex flex-col gap-1 text-sm">
          <label htmlFor={nameId}>Name</label>
          <input id={nameId} className="input input-sm w-full" value={name} onChange={(e) => setName(e.target.value)} maxLength={60} autoComplete="off" aria-invalid={!!nameError} data-testid="role-name" />
          {nameError && <span className="text-xs text-error" role="alert">{nameError}</span>}
        </div>
        <div className="flex flex-col gap-1 text-sm">
          <label htmlFor={descId}>Description (optional)</label>
          <input id={descId} className="input input-sm w-full" value={description} onChange={(e) => setDescription(e.target.value)} maxLength={200} autoComplete="off" data-testid="role-description" />
        </div>
      </div>

      <div className="flex flex-col gap-1 text-sm">
        <label htmlFor={filterId}>Filter permissions</label>
        <input id={filterId} type="search" className="input input-sm w-full max-w-sm" value={filter} onChange={(e) => setFilter(e.target.value)} placeholder="e.g. application" autoComplete="off" data-testid="role-filter" />
      </div>

      <div className="flex max-h-72 flex-col gap-3 overflow-y-auto rounded-box border border-base-300 p-3" tabIndex={0} aria-label="Permissions to include" data-testid="role-permissions">
        {CLASS_ORDER.map((c) => {
          const items = catalogue.filter((p) => p.class === c && (!q || p.id.includes(q) || p.description.toLowerCase().includes(q)));
          if (items.length === 0) return null;
          return (
            <fieldset key={c} className="flex flex-col gap-1">
              <legend className="text-sm font-semibold">{CLASS_LABEL[c]} <span className="font-normal text-base-content/60">— {CLASS_NOTE[c]}</span></legend>
              {items.map((p) => {
                const held = holdsPlatform(me, p.id);
                return (
                  <label key={p.id} className={`flex items-start gap-2 py-0.5 text-sm ${held ? "" : "text-base-content/50"}`}>
                    <input type="checkbox" className="checkbox checkbox-xs mt-0.5" checked={picked.includes(p.id)} disabled={!held} onChange={(e) => toggle(p.id, e.target.checked)} data-testid={`perm-${p.id}`} />
                    <span>
                      <span className="font-mono text-xs">{p.id}</span> <span className="text-base-content/70">{p.description}</span>
                      {p.needsApproval && <> <Badge tone="warn">Needs approval</Badge></>}
                      {!held && <span className="block text-xs">You do not hold this permission, so you cannot include it.</span>}
                    </span>
                  </label>
                );
              })}
            </fieldset>
          );
        })}
      </div>

      <div className="rounded-box border border-base-300 p-3 text-sm" aria-live="polite" data-testid="role-summary">
        <strong>{picked.length}</strong> {picked.length === 1 ? "permission" : "permissions"} selected.{" "}
        {levels.length > 0
          ? <>This role could be assigned at: <span data-testid="role-levels">{levels.map((l) => LEVEL_LABEL[l]).join(", ")}</span>.</>
          : "Tick at least one permission to see where the role could be assigned."}
        {levels.length === 1 && levels[0] === "platform" && <span className="block text-xs text-base-content/60">Infrastructure and security permissions can only be granted across the whole platform (or a hardware pool).</span>}
      </div>

      {error && <ErrorPanel message={error} testId="role-error" />}
      <div className="modal-action mt-0">
        <button type="button" className="btn btn-sm btn-ghost" onClick={onClose}>Cancel</button>
        <button type="submit" className="btn btn-sm btn-primary" disabled={busy || !nameOk || picked.length === 0} data-testid="role-submit">{busy ? "Creating…" : "Create role"}</button>
      </div>
    </form>
  );
}

export function CreateRoleDialog({ open, catalogue, roles, me, onAuth, onClose, onCreated }: {
  open: boolean; catalogue: PermissionDto[]; roles: RoleDto[]; me: WhoAmI | null; onAuth: (m: string) => void; onClose: () => void; onCreated: (r: RoleDto) => void;
}) {
  return (
    <FormDialog open={open} title="Create a custom role" onClose={onClose} testId="role-dialog" wide>
      {open && <Form catalogue={catalogue} roles={roles} me={me} onAuth={onAuth} onClose={onClose} onCreated={onCreated} />}
    </FormDialog>
  );
}

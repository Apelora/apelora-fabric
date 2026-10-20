import { useRef, useState, type KeyboardEvent } from "react";

import type { PermissionDto, PolicyDto, WhoAmI } from "@apelora/api-client";

import { Badge, ConfirmDialog, ErrorPanel, Notice, Section, SectionHeader, TableFrame } from "../../components/ui";
import { client, useLoad, type Load } from "../../lib/api";
import { actorLabel, fmtTime, holds, holdsPlatform, useDirectory, useRun } from "../../lib/identity";
import { ApprovalsPanel } from "./Approvals";
import { PolicyDialog, summarize } from "./PolicyDialog";

type Tab = "policies" | "approvals";

function PoliciesPanel({ me, catalogue, onAuth, policies, reload }: { me: WhoAmI | null; catalogue: PermissionDto[]; onAuth: (m: string) => void; policies: Load<PolicyDto[]>; reload: () => void }) {
  const dir = useDirectory(me, onAuth, { users: true });
  const [editing, setEditing] = useState<PolicyDto | "new" | null>(null);
  const [toDelete, setToDelete] = useState<PolicyDto | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const { error, run } = useRun(onAuth);
  const canManage = holdsPlatform(me, "access_policy.manage");
  const remove = (p: PolicyDto) => {
    setNotice(null);
    void run(async () => {
      await client.request("delete_policy", { params: { id: p.id } });
      setNotice(`The policy ${p.name} was deleted.`);
      reload();
    });
  };
  return (
    <>
      <Section title="Policies" state={policies} testId="policies" onRetry={reload}>
        {(list) => (
          <>
            <SectionHeader title="Policies" count={list.length}>
              {canManage && <button type="button" className="btn btn-sm btn-primary" onClick={() => setEditing("new")} data-testid="policy-open">Create policy</button>}
            </SectionHeader>
            <p className="text-sm text-base-content/70">Rules that apply on top of roles: which actions need multi-factor sign-in, and which extra actions need another person&apos;s approval.{canManage ? "" : " You can read them; platform administrators change them."}</p>
            {notice && <Notice tone="ok" testId="policies-notice">{notice}</Notice>}
            {error && <ErrorPanel message={error} testId="policies-error" />}
            {list.length === 0 ? <p className="text-sm text-base-content/70" data-testid="policies-empty">No access policies yet.</p> : (
              <TableFrame>
                <thead><tr><th>Policy</th><th>Kind</th><th>State</th><th className="max-md:hidden">Updated</th><th><span className="sr-only">Actions</span></th></tr></thead>
                <tbody>
                  {list.map((p) => (
                    <tr key={p.id} data-testid="policy-row">
                      <td className="font-medium">{p.name}<span className="block text-xs font-normal text-base-content/70">{summarize(p)}</span></td>
                      <td>{p.kind === "mfa" ? "Multi-factor" : p.kind === "approval" ? "Approval" : p.kind}</td>
                      <td>{p.enabled ? <Badge tone="ok">Enabled</Badge> : <Badge>Disabled</Badge>}</td>
                      <td className="max-md:hidden">{fmtTime(p.updatedAt)}<span className="block text-xs text-base-content/60">{actorLabel(p.updatedBy, dir.names)}</span></td>
                      <td className="whitespace-nowrap text-right">
                        {canManage && (
                          <>
                            <button type="button" className="btn btn-xs btn-ghost" aria-label={`Edit ${p.name}`} onClick={() => setEditing(p)}>Edit…</button>
                            <button type="button" className="btn btn-xs btn-ghost text-error" aria-label={`Delete ${p.name}`} onClick={() => setToDelete(p)}>Delete…</button>
                          </>
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </TableFrame>
            )}
          </>
        )}
      </Section>
      <PolicyDialog open={!!editing} existing={editing === "new" ? null : editing} catalogue={catalogue} onAuth={onAuth} onClose={() => setEditing(null)} onSaved={(p) => { setEditing(null); setNotice(`The policy ${p.name} was saved.`); reload(); }} />
      <ConfirmDialog
        open={!!toDelete}
        title={`Delete the policy ${toDelete?.name ?? ""}?`}
        body={<p>The rule stops applying at once. {toDelete ? summarize(toDelete) : ""}</p>}
        action="Delete policy"
        onCancel={() => setToDelete(null)}
        onConfirm={() => { const p = toDelete; setToDelete(null); if (p) remove(p); }}
      />
    </>
  );
}

const TABS: { id: Tab; label: string }[] = [
  { id: "policies", label: "Policies" },
  { id: "approvals", label: "Approvals" },
];

export function PoliciesView({ me, onAuth }: { me: WhoAmI | null; onAuth: (m: string) => void }) {
  const showPolicies = holds(me, "access_policy.manage", "audit.read");
  const showApprovals = holds(me, "approval.request", "approval.grant");
  const tabs = TABS.filter((t) => (t.id === "policies" ? showPolicies : showApprovals));
  const [tab, setTab] = useState<Tab>(tabs[0]?.id ?? "policies");
  const refs = useRef<Record<string, HTMLButtonElement | null>>({});
  // Policies feed both tabs: the list itself and the extra actions that may be requested.
  const [policies, reloadPolicies] = useLoad<PolicyDto[]>(() => (showPolicies ? client.get("policies").then((r) => r.items) : Promise.resolve([])), onAuth);
  const [catalogue] = useLoad<PermissionDto[]>(() => client.get("permissions").then((r) => r.items), onAuth);
  const cat = catalogue.kind === "ok" ? catalogue.data : [];
  const active = tabs.some((t) => t.id === tab) ? tab : tabs[0]?.id;

  const onKey = (e: KeyboardEvent<HTMLButtonElement>, i: number) => {
    const next = e.key === "ArrowRight" ? (i + 1) % tabs.length : e.key === "ArrowLeft" ? (i - 1 + tabs.length) % tabs.length : e.key === "Home" ? 0 : e.key === "End" ? tabs.length - 1 : -1;
    if (next < 0) return;
    e.preventDefault();
    const t = tabs[next]!;
    setTab(t.id);
    refs.current[t.id]?.focus();
  };

  return (
    <div className="flex flex-col gap-4">
      <h1 className="text-lg font-semibold">Access policies</h1>
      <div role="tablist" aria-label="Access policies" className="tabs tabs-border" data-testid="policy-tabs">
        {tabs.map((t, i) => (
          <button key={t.id} ref={(el) => { refs.current[t.id] = el; }} type="button" role="tab" id={`tab-${t.id}`} aria-selected={active === t.id} aria-controls={`panel-${t.id}`} tabIndex={active === t.id ? 0 : -1} className={`tab text-base-content/80 ${active === t.id ? "tab-active" : ""}`} onClick={() => setTab(t.id)} onKeyDown={(e) => onKey(e, i)} data-testid={`tab-${t.id}`}>{t.label}</button>
        ))}
      </div>
      {active === "policies" && (
        <div role="tabpanel" id="panel-policies" aria-labelledby="tab-policies" className="flex flex-col gap-2">
          <PoliciesPanel me={me} catalogue={cat} onAuth={onAuth} policies={policies} reload={reloadPolicies} />
        </div>
      )}
      {active === "approvals" && (
        <div role="tabpanel" id="panel-approvals" aria-labelledby="tab-approvals" className="flex flex-col gap-2">
          <ApprovalsPanel me={me} catalogue={cat} policies={policies.kind === "ok" ? policies.data : []} onAuth={onAuth} />
        </div>
      )}
    </div>
  );
}

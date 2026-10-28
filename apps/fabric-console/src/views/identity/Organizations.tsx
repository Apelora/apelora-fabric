import { useId, useState, type ReactNode } from "react";

import type { EnvKind, EnvironmentDto, OrganizationDto, ProjectDto, WhoAmI } from "@apelora/api-client";

import { Badge, ErrorPanel, FormDialog, Notice, Section, SectionHeader, sentence } from "../../components/ui";
import { client, useLoad } from "../../lib/api";
import { ENV_KINDS, ENV_LABEL, SLUG_HINT, fmtDate, holds, holdsPlatform, useRun, validSlug } from "../../lib/identity";

// ---- creating things --------------------------------------------------------------------------

type NamedForm = { title: string; noun: string; testId: string; extra?: ReactNode; busyText: string };

/** A name and an identifier, plus whatever `extra` adds; used for organizations, projects and environments. */
function NameSlugDialog({ open, form, onClose, submit, canSubmit, error, busy, name, setName, slug, setSlug }: {
  open: boolean; form: NamedForm; onClose: () => void; submit: () => void; canSubmit: boolean; error: string | null; busy: boolean;
  name: string; setName: (v: string) => void; slug: string; setSlug: (v: string) => void;
}) {
  const nameId = useId();
  const slugId = useId();
  const slugHint = useId();
  const slugBad = !!slug && !validSlug(slug);
  return (
    <FormDialog open={open} title={form.title} onClose={onClose} testId={form.testId}>
      <form className="flex flex-col gap-3" onSubmit={(e) => { e.preventDefault(); if (canSubmit) submit(); }}>
        <div className="flex flex-col gap-1 text-sm">
          <label htmlFor={nameId}>Name</label>
          <input id={nameId} className="input input-sm w-full" value={name} onChange={(e) => setName(e.target.value)} maxLength={80} autoComplete="off" data-testid={`${form.testId}-name`} />
        </div>
        <div className="flex flex-col gap-1 text-sm">
          <label htmlFor={slugId}>Identifier</label>
          <input id={slugId} className="input input-sm w-full font-mono" value={slug} onChange={(e) => setSlug(e.target.value.toLowerCase())} maxLength={40} autoComplete="off" spellCheck={false} aria-invalid={slugBad} aria-describedby={slugHint} data-testid={`${form.testId}-slug`} />
          <span id={slugHint} className={`text-xs ${slugBad ? "text-error" : "text-base-content/60"}`}>{SLUG_HINT}. It cannot be changed later.</span>
        </div>
        {form.extra}
        {error && <ErrorPanel message={error} testId={`${form.testId}-error`} />}
        <div className="modal-action mt-0">
          <button type="button" className="btn btn-sm btn-ghost" onClick={onClose}>Cancel</button>
          <button type="submit" className="btn btn-sm btn-primary" disabled={busy || !canSubmit} data-testid={`${form.testId}-submit`}>{busy ? form.busyText : `Create ${form.noun}`}</button>
        </div>
      </form>
    </FormDialog>
  );
}

function CreateOrganization({ open, onAuth, onClose, onCreated }: { open: boolean; onAuth: (m: string) => void; onClose: () => void; onCreated: (o: OrganizationDto) => void }) {
  const [name, setName] = useState("");
  const [slug, setSlug] = useState("");
  const { busy, error, setError, run } = useRun(onAuth);
  return (
    <NameSlugDialog open={open} form={{ title: "Create an organization", noun: "organization", testId: "org-dialog", busyText: "Creating…" }} onClose={() => { setError(null); onClose(); }} busy={busy} error={error}
      name={name} setName={setName} slug={slug} setSlug={setSlug} canSubmit={!!name.trim() && validSlug(slug)}
      submit={() => void run(async () => { const o = await client.request("create_organization", { body: { name: name.trim(), slug } }); setName(""); setSlug(""); onCreated(o); })} />
  );
}

function CreateProject({ org, open, onAuth, onClose, onCreated }: { org: OrganizationDto; open: boolean; onAuth: (m: string) => void; onClose: () => void; onCreated: (p: ProjectDto) => void }) {
  const [name, setName] = useState("");
  const [slug, setSlug] = useState("");
  const { busy, error, setError, run } = useRun(onAuth);
  return (
    <NameSlugDialog open={open} form={{ title: `Create a project in ${org.name}`, noun: "project", testId: "project-dialog", busyText: "Creating…" }} onClose={() => { setError(null); onClose(); }} busy={busy} error={error}
      name={name} setName={setName} slug={slug} setSlug={setSlug} canSubmit={!!name.trim() && validSlug(slug)}
      submit={() => void run(async () => { const p = await client.request("create_project", { params: { org: org.id }, body: { name: name.trim(), slug } }); setName(""); setSlug(""); onCreated(p); })} />
  );
}

function CreateEnvironment({ project, me, open, onAuth, onClose, onCreated }: { project: ProjectDto; me: WhoAmI | null; open: boolean; onAuth: (m: string) => void; onClose: () => void; onCreated: (e: EnvironmentDto) => void }) {
  const [name, setName] = useState("");
  const [slug, setSlug] = useState("");
  const [kind, setKind] = useState<EnvKind>("development");
  const [existing, setExisting] = useState("");
  const { busy, error, setError, run } = useRun(onAuth);
  const kindId = useId();
  const existingId = useId();
  const platform = holdsPlatform(me, "cluster.manage");
  const extra = (
    <>
      <div className="flex flex-col gap-1 text-sm">
        <label htmlFor={kindId}>Kind</label>
        <select id={kindId} className="select select-sm w-full" value={kind} onChange={(e) => setKind(e.target.value as EnvKind)} data-testid="env-kind">
          {ENV_KINDS.map((k) => <option key={k} value={k}>{ENV_LABEL[k]}</option>)}
        </select>
        <span className="text-xs text-base-content/60">Policies such as “production needs multi-factor sign-in” refer to the kind.</span>
      </div>
      <p className="text-xs text-base-content/60">The Incus project name comes from the organization, project and environment identifiers. Apelora creates that restricted Incus project with the environment.</p>
      {platform && (
        <div className="flex flex-col gap-1 text-sm">
          <label htmlFor={existingId}>Existing Incus project (platform administrators, optional)</label>
          <input id={existingId} className="input input-sm w-full font-mono" value={existing} onChange={(e) => setExisting(e.target.value)} autoComplete="off" spellCheck={false} data-testid="env-existing" />
          <span className="text-xs text-base-content/60">Maps this environment to an Incus project that already exists instead of the derived name.</span>
        </div>
      )}
    </>
  );
  return (
    <NameSlugDialog open={open} form={{ title: `Create an environment in ${project.name}`, noun: "environment", testId: "env-dialog", busyText: "Creating…", extra }} onClose={() => { setError(null); onClose(); }} busy={busy} error={error}
      name={name} setName={setName} slug={slug} setSlug={setSlug} canSubmit={!!name.trim() && validSlug(slug)}
      submit={() => void run(async () => {
        const e = await client.request("create_environment", { params: { projectId: project.id }, body: { name: name.trim(), slug, kind, incusProject: platform && existing.trim() ? existing.trim() : null } });
        setName(""); setSlug(""); setExisting("");
        onCreated(e);
      })} />
  );
}

// ---- environments, projects, organizations ----------------------------------------------------

function Environment({ env }: { env: EnvironmentDto }) {
  const config = Object.entries(env.incusProjectConfig).sort(([a], [b]) => a.localeCompare(b));
  return (
    <li className="flex flex-col gap-2 py-3" data-testid="environment">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <span className="font-medium">{env.name}</span>
        <span className="font-mono text-xs text-base-content/60">{env.slug}</span>
        <Badge tone="neutral">{ENV_LABEL[env.kind]}</Badge>
        <span className="text-sm">Incus project <span className="font-mono text-xs" data-testid="env-incus-project">{env.incusProject ?? "not set"}</span></span>
      </div>
      {env.incusState === "created" && <p className="text-sm text-base-content/70" data-testid="env-provisioned">Incus project created by Apelora with the settings below.</p>}
      {env.incusState === "adopted" && <p className="text-sm text-base-content/70" data-testid="env-provisioned">An existing restricted Incus project, used as it is. Tenancy &amp; quotas shows its settings.</p>}
      {env.incusState === "pending" && (
        <Notice tone="warn" testId="env-not-provisioned" title="Incus project not prepared yet">{env.incusDetail ? `${sentence(env.incusDetail)}.` : "Tenancy & quotas can prepare it."}</Notice>
      )}
      <details className="text-sm" data-testid="env-config">
        <summary className="cursor-pointer text-base-content/80">Settings of an Incus project Apelora creates ({config.length})</summary>
        <pre tabIndex={0} aria-label={`Incus project settings for ${env.name}`} className="mt-2 overflow-x-auto rounded-box border border-base-300 bg-base-200 p-3 font-mono text-xs">
          {config.map(([k, v]) => `${k}=${String(v)}`).join("\n")}
        </pre>
      </details>
    </li>
  );
}

function Environments({ project, me, onAuth }: { project: ProjectDto; me: WhoAmI | null; onAuth: (m: string) => void }) {
  const [envs, reload] = useLoad<EnvironmentDto[]>(() => client.request("environments", { params: { projectId: project.id } }).then((r) => r.items), onAuth);
  const [creating, setCreating] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  return (
    <div className="flex flex-col gap-2 pl-4" data-testid="environments">
      <Section title={`Environments of ${project.name}`} state={envs} testId="environments-section" onRetry={reload}>
        {(list) => (
          <>
            <div className="flex flex-wrap items-center justify-between gap-2">
              <h3 className="text-sm font-semibold">Environments <span className="font-normal text-base-content/60">({list.length})</span></h3>
              {holds(me, "environment.manage") && <button type="button" className="btn btn-xs btn-outline" onClick={() => setCreating(true)} data-testid="env-open">Create environment…</button>}
            </div>
            {notice && <Notice tone="ok" testId="env-notice">{notice}</Notice>}
            {list.length === 0 ? <p className="text-sm text-base-content/70" data-testid="envs-empty">No environments yet.</p> : <ul className="divide-y divide-base-300">{list.map((e) => <Environment key={e.id} env={e} />)}</ul>}
          </>
        )}
      </Section>
      <CreateEnvironment project={project} me={me} open={creating} onAuth={onAuth} onClose={() => setCreating(false)} onCreated={(e) => { setCreating(false); setNotice(e.incusState === "pending" ? `Environment ${e.name} created. Its Incus project ${e.incusProject ?? ""} is not prepared yet: ${e.incusDetail}.` : `Environment ${e.name} created, with the restricted Incus project ${e.incusProject ?? ""}.`); reload(); }} />
    </div>
  );
}

/** One row that opens and closes the panel under it. */
function Disclosure({ title, meta, level, open, onToggle, controls, children }: { title: string; meta?: ReactNode; level: "organization" | "project"; open: boolean; onToggle: () => void; controls: string; children: ReactNode }) {
  return (
    <li className="flex flex-col gap-2 px-4 py-3" data-testid={`${level}-item`}>
      <button type="button" className="flex w-full flex-wrap items-center gap-x-3 gap-y-1 text-left" aria-expanded={open} aria-controls={controls} onClick={onToggle} aria-label={`${open ? "Collapse" : "Expand"} ${level} ${title}`}>
        <span aria-hidden="true" className="w-3 text-base-content/60">{open ? "▾" : "▸"}</span>
        <span className="font-medium">{title}</span>
        {meta}
      </button>
      <div id={controls} hidden={!open} className="flex flex-col gap-2">{open && children}</div>
    </li>
  );
}

function Projects({ org, me, onAuth }: { org: OrganizationDto; me: WhoAmI | null; onAuth: (m: string) => void }) {
  const [projects, reload] = useLoad<ProjectDto[]>(() => client.request("projects", { query: { organization: org.id } }).then((r) => r.items), onAuth);
  const [creating, setCreating] = useState(false);
  const [open, setOpen] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  return (
    <div className="flex flex-col gap-2 pl-4" data-testid="projects">
      <Section title={`Projects of ${org.name}`} state={projects} testId="projects-section" onRetry={reload}>
        {(list) => (
          <>
            <div className="flex flex-wrap items-center justify-between gap-2">
              <h2 className="text-sm font-semibold">Projects <span className="font-normal text-base-content/60">({list.length})</span></h2>
              {holds(me, "project.create") && <button type="button" className="btn btn-xs btn-outline" onClick={() => setCreating(true)} data-testid="project-open">Create project…</button>}
            </div>
            {notice && <Notice tone="ok" testId="project-notice">{notice}</Notice>}
            {list.length === 0 ? <p className="text-sm text-base-content/70" data-testid="projects-empty">No projects in this organization yet.</p> : (
              <ul className="divide-y divide-base-300 rounded-box border border-base-300">
                {list.map((p) => (
                  <Disclosure key={p.id} level="project" title={p.name} meta={<><span className="font-mono text-xs text-base-content/60">{p.slug}</span><span className="text-xs text-base-content/60">created {fmtDate(p.createdAt)}</span></>} open={open === p.id} onToggle={() => setOpen(open === p.id ? null : p.id)} controls={`env-${p.id}`}>
                    <Environments project={p} me={me} onAuth={onAuth} />
                  </Disclosure>
                ))}
              </ul>
            )}
          </>
        )}
      </Section>
      <CreateProject org={org} open={creating} onAuth={onAuth} onClose={() => setCreating(false)} onCreated={(p) => { setCreating(false); setNotice(`Project ${p.name} created.`); setOpen(p.id); reload(); }} />
    </div>
  );
}

export function OrganizationsView({ me, onAuth }: { me: WhoAmI | null; onAuth: (m: string) => void }) {
  const [orgs, reload] = useLoad<OrganizationDto[]>(() => client.get("organizations").then((r) => r.items), onAuth);
  const [creating, setCreating] = useState(false);
  const [open, setOpen] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  return (
    <>
      <Section title="Organizations" level={1} state={orgs} testId="organizations" onRetry={reload}>
        {(list) => (
          <>
            <SectionHeader title="Organizations" level={1} count={list.length}>
              {holdsPlatform(me, "organization.create") && <button type="button" className="btn btn-sm btn-primary" onClick={() => setCreating(true)} data-testid="org-open">Create organization</button>}
            </SectionHeader>
            <p className="text-sm text-base-content/70">Organizations hold projects, and projects hold environments. Each environment deploys into its own restricted Incus project.</p>
            {notice && <Notice tone="ok" testId="org-notice">{notice}</Notice>}
            {list.length === 0 ? <p className="text-sm text-base-content/70" data-testid="orgs-empty">No organizations yet.</p> : (
              <ul className="divide-y divide-base-300 rounded-box border border-base-300">
                {list.map((o) => (
                  <Disclosure key={o.id} level="organization" title={o.name} meta={<><span className="font-mono text-xs text-base-content/60">{o.slug}</span>{o.disabled && <Badge tone="error">Disabled</Badge>}<span className="text-xs text-base-content/60">created {fmtDate(o.createdAt)}</span></>} open={open === o.id} onToggle={() => setOpen(open === o.id ? null : o.id)} controls={`projects-${o.id}`}>
                    <Projects org={o} me={me} onAuth={onAuth} />
                  </Disclosure>
                ))}
              </ul>
            )}
          </>
        )}
      </Section>
      <CreateOrganization open={creating} onAuth={onAuth} onClose={() => setCreating(false)} onCreated={(o) => { setCreating(false); setNotice(`Organization ${o.name} created.`); setOpen(o.id); reload(); }} />
    </>
  );
}

import { useEffect, useMemo, useRef, useState } from "react";

import type { ApplicationAssembly } from "@apelora/assembly-model";

import { useBackend } from "../../features/applications/backend";
import { useAppList } from "../../features/applications/hooks";
import { useLifecycle } from "../../features/applications/lifecycle";
import { readImport } from "../../features/applications/transfer";
import { useAppToasts } from "../../features/applications/toast";
import { DESIGN_LABEL, NEEDS_ATTENTION, RUNTIME_LABEL, type AppView, type DeploymentStatus, type DesignStatus } from "../../features/applications/model";
import { DESIGN_ORDER, STATUS_ORDER, loadPrefs, savePrefs, type Prefs, type SortKey } from "../../features/applications/prefs";
import { IconCards, IconList, IconPlus, IconSearch, IconUpload } from "../icons";
import { ApplicationCard } from "./ApplicationCard";
import { ApplicationRow } from "./ApplicationRow";
import { NewApplicationDialog } from "./NewApplicationDialog";
import { Empty, ErrorState, Skeleton } from "./common";

const STATUSES = Object.keys(RUNTIME_LABEL) as DeploymentStatus[];

function Tile({ label, value, testId }: { label: string; value: number; testId: string }) {
  return (
    <div className="rounded-box border border-base-300 bg-base-100 px-4 py-3" data-testid={testId}>
      <div className="text-xs text-base-content/60">{label}</div>
      <div className="text-2xl font-semibold tabular-nums" data-testid={`${testId}-value`}>{value}</div>
    </div>
  );
}

/** What the search box matches: name, description and where the application lives. */
export const matches = (a: AppView, q: string): boolean => {
  const n = q.trim().toLowerCase();
  if (!n) return true;
  return [a.name, a.description, a.project.name, a.project.slug, a.environment?.name ?? "", a.environment?.slug ?? ""].some((s) => s.toLowerCase().includes(n));
};

export function sortApps(items: AppView[], sort: SortKey, desc: boolean): AppView[] {
  const dir = desc ? -1 : 1;
  const key = (a: AppView): string | number => (sort === "name" ? a.name : sort === "status" ? STATUS_ORDER[a.deployment.status] * 10 + DESIGN_ORDER[a.design.status] : new Date(a.deployment.lastActivityAt ?? a.updatedAt).getTime());
  return [...items].sort((p, q) => {
    const x = key(p), y = key(q);
    // "Last update" lists the newest first unless reversed.
    const c = typeof x === "number" && typeof y === "number" ? (sort === "updated" ? y - x : x - y) : String(x).localeCompare(String(y));
    return (c || p.name.localeCompare(q.name)) * dir;
  });
}

export function ApplicationsPage() {
  const [prefs, setPrefs] = useState<Prefs>(loadPrefs);
  const [query, setQuery] = useState("");
  const [project, setProject] = useState("");
  const [environment, setEnvironment] = useState("");
  const [status, setStatus] = useState<"" | DeploymentStatus>("");
  const [design, setDesign] = useState<"" | DesignStatus>("");
  const [archived, setArchived] = useState(false);
  const [page, setPage] = useState(0);
  const [creating, setCreating] = useState(false);
  const [imported, setImported] = useState<{ assembly: ApplicationAssembly; fileName: string } | null>(null);
  const file = useRef<HTMLInputElement>(null);
  const toast = useAppToasts((s) => s.push);
  const doImport = async (f: File) => {
    const r = await readImport(f);
    if (r.ok) setImported({ assembly: r.assembly, fileName: f.name });
    else toast("error", `${f.name} was not imported: ${r.errors.slice(0, 2).join("; ")}`);
  };
  const [list, reload] = useAppList(archived);
  const bump = useLifecycle((s) => s.bump);
  const backend = useBackend((s) => s.backend);
  useEffect(() => { if (bump) reload(); }, [bump]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => savePrefs(prefs), [prefs]);
  const set = (p: Partial<Prefs>) => setPrefs((c) => ({ ...c, ...p }));

  const items = useMemo(() => (list.kind === "ready" ? list.data.items : []), [list]);
  const projects = useMemo(() => [...new Set(items.map((a) => a.project.name))].sort(), [items]);
  const environments = useMemo(() => [...new Set(items.map((a) => a.environment?.name).filter((x): x is string => !!x))].sort(), [items]);
  const filtered = useMemo(
    () => sortApps(items.filter((a) => matches(a, query) && (!project || a.project.name === project) && (!environment || a.environment?.name === environment) && (!status || a.deployment.status === status) && (!design || a.design.status === design)), prefs.sort, prefs.desc),
    [items, query, project, environment, status, design, prefs.sort, prefs.desc],
  );
  useEffect(() => setPage(0), [query, project, environment, status, design, archived, prefs.pageSize, prefs.sort, prefs.desc]);
  const pages = Math.max(1, Math.ceil(filtered.length / prefs.pageSize));
  const safePage = Math.min(page, pages - 1);
  const shown = filtered.slice(safePage * prefs.pageSize, (safePage + 1) * prefs.pageSize);
  const stats = { total: items.length, running: items.filter((a) => a.deployment.status === "running").length, attention: items.filter((a) => NEEDS_ATTENTION.has(a.deployment.status)).length, stopped: items.filter((a) => a.deployment.status === "stopped").length };
  const filtering = !!(query || project || environment || status || design);

  return (
    <main className="mx-auto flex w-full max-w-6xl flex-col gap-5 px-4 py-6" data-testid="applications-page">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h1 className="text-2xl font-semibold">Applications</h1>
          <p className="text-sm text-base-content/60">Design, deploy, and manage your application assemblies.</p>
        </div>
        <div className="flex items-center gap-2">
          <button type="button" className="btn btn-sm" onClick={() => file.current?.click()} data-testid="import-application"><IconUpload size={14} />Import</button>
          <input ref={file} type="file" accept="application/json,.json" hidden data-testid="import-application-input" onChange={(e) => { const f = e.target.files?.[0]; if (f) void doImport(f); e.target.value = ""; }} />
          <button type="button" className="btn btn-sm btn-primary" onClick={() => setCreating(true)} data-testid="new-application"><IconPlus size={14} />New application</button>
        </div>
      </div>

      <section aria-label="Summary" className="grid grid-cols-2 gap-3 lg:grid-cols-4">
        <Tile label="Total applications" value={stats.total} testId="tile-total" />
        <Tile label="Running" value={stats.running} testId="tile-running" />
        <Tile label="Needs attention" value={stats.attention} testId="tile-attention" />
        <Tile label="Stopped" value={stats.stopped} testId="tile-stopped" />
      </section>

      {list.kind === "ready" && list.data.stale && (
        <div role="status" className="rounded-box border border-warning/50 bg-warning/10 px-3 py-2 text-xs" data-testid="stale-banner">
          {backend.kind === "api"
            ? "The control plane could not observe the cluster, so runtime status is shown as Unknown and nothing can be started or stopped until it can."
            : "Applications are kept in this browser. Their runtime status is unknown here; connect to a control plane to deploy, start and stop them."}
        </div>
      )}

      <div className="flex flex-col gap-2 border-t border-base-300 pt-4">
        <div className="flex flex-wrap items-center gap-2">
          <label className="input input-sm flex min-w-52 flex-1 items-center gap-2">
            <IconSearch size={14} />
            <input type="search" placeholder="Search by name, description or project" value={query} onChange={(e) => setQuery(e.target.value)} aria-label="Search applications" data-testid="app-search" />
          </label>
          <select className="select select-sm w-36" value={project} onChange={(e) => setProject(e.target.value)} aria-label="Project" data-testid="filter-project"><option value="">All projects</option>{projects.map((p) => <option key={p}>{p}</option>)}</select>
          <select className="select select-sm w-40" value={environment} onChange={(e) => setEnvironment(e.target.value)} aria-label="Environment" data-testid="filter-environment"><option value="">All environments</option>{environments.map((p) => <option key={p}>{p}</option>)}</select>
          <select className="select select-sm w-36" value={status} onChange={(e) => setStatus(e.target.value as "" | DeploymentStatus)} aria-label="Status" data-testid="filter-status"><option value="">All statuses</option>{STATUSES.map((s) => <option key={s} value={s}>{RUNTIME_LABEL[s]}</option>)}</select>
          <select className="select select-sm w-36" value={design} onChange={(e) => setDesign(e.target.value as "" | DesignStatus)} aria-label="Design" data-testid="filter-design"><option value="">Any design</option>{(Object.keys(DESIGN_LABEL) as DesignStatus[]).map((s) => <option key={s} value={s}>{DESIGN_LABEL[s]}</option>)}</select>
          <div className="join" role="group" aria-label="Layout">
            <button type="button" className={`btn btn-sm join-item ${prefs.view === "cards" ? "btn-active" : ""}`} aria-pressed={prefs.view === "cards"} onClick={() => set({ view: "cards" })} data-testid="view-cards"><IconCards size={14} />Cards</button>
            <button type="button" className={`btn btn-sm join-item ${prefs.view === "list" ? "btn-active" : ""}`} aria-pressed={prefs.view === "list"} onClick={() => set({ view: "list" })} data-testid="view-list"><IconList size={14} />List</button>
          </div>
        </div>
        <div className="flex flex-wrap items-center justify-between gap-2 text-xs text-base-content/60">
          <span data-testid="result-count" aria-live="polite">{list.kind === "ready" ? `${filtered.length} of ${items.length} applications` : ""}</span>
          <div className="flex items-center gap-3">
            <label className="flex items-center gap-1.5"><input type="checkbox" className="checkbox checkbox-xs" checked={archived} onChange={(e) => setArchived(e.target.checked)} data-testid="show-archived" />Show archived</label>
            <label className="flex items-center gap-1.5">Sort
              <select className="select select-xs" value={prefs.sort} onChange={(e) => set({ sort: e.target.value as SortKey })} aria-label="Sort by" data-testid="sort-by"><option value="name">Name</option><option value="status">Status</option><option value="updated">Last update</option></select>
            </label>
            <button type="button" className="btn btn-ghost btn-xs" aria-pressed={prefs.desc} onClick={() => set({ desc: !prefs.desc })} data-testid="sort-direction">{prefs.desc ? "Descending" : "Ascending"}</button>
          </div>
        </div>
      </div>

      {list.kind === "loading" && <Skeleton />}
      {list.kind === "error" && <ErrorState message={list.message} onRetry={reload} />}
      {list.kind === "ready" && items.length === 0 && (
        <Empty title="No applications yet" action={<button type="button" className="btn btn-sm btn-primary" onClick={() => setCreating(true)}>New application</button>}>
          Create one from a blank canvas or a template. Each application has its own saved canvas, versions and deployments.
        </Empty>
      )}
      {list.kind === "ready" && items.length > 0 && filtered.length === 0 && (
        <Empty title="No applications match" action={filtering ? <button type="button" className="btn btn-sm" data-testid="clear-filters" onClick={() => { setQuery(""); setProject(""); setEnvironment(""); setStatus(""); setDesign(""); }}>Clear filters</button> : undefined}>Try a different search or filter.</Empty>
      )}
      {list.kind === "ready" && shown.length > 0 && (prefs.view === "cards" ? (
        <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3" data-testid="cards-view">{shown.map((a) => <ApplicationCard key={a.name} app={a} />)}</div>
      ) : (
        <div className="overflow-x-auto rounded-box border border-base-300" data-testid="list-view">
          <table className="table table-sm">
            <thead><tr><th>Application</th><th>Status</th><th>Design</th><th>Project</th><th>Health</th><th>Last activity</th><th className="text-right">Actions</th></tr></thead>
            <tbody>{shown.map((a) => <ApplicationRow key={a.name} app={a} />)}</tbody>
          </table>
        </div>
      ))}

      {list.kind === "ready" && filtered.length > prefs.pageSize && (
        <nav className="flex items-center justify-between text-xs" aria-label="Pages" data-testid="pagination">
          <span>{safePage * prefs.pageSize + 1}–{Math.min(filtered.length, (safePage + 1) * prefs.pageSize)} of {filtered.length}</span>
          <div className="join">
            <button className="btn btn-xs join-item" disabled={safePage === 0} onClick={() => setPage(safePage - 1)} data-testid="page-prev">Previous</button>
            <span className="btn btn-xs join-item pointer-events-none">Page {safePage + 1} of {pages}</span>
            <button className="btn btn-xs join-item" disabled={safePage >= pages - 1} onClick={() => setPage(safePage + 1)} data-testid="page-next">Next</button>
          </div>
        </nav>
      )}
      {list.kind === "ready" && filtered.length > 12 && (
        <label className="flex items-center gap-1.5 self-end text-xs">Per page <select className="select select-xs" value={prefs.pageSize} onChange={(e) => set({ pageSize: Number(e.target.value) })} aria-label="Per page">{[12, 24, 48].map((n) => <option key={n}>{n}</option>)}</select></label>
      )}
      {creating && <NewApplicationDialog onClose={() => setCreating(false)} />}
      {imported && <NewApplicationDialog imported={imported} onClose={() => setImported(null)} />}
    </main>
  );
}

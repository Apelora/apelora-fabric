import { useEffect, useMemo, useState } from "react";

import { createEmptyAssembly, type ApplicationAssembly } from "@apelora/assembly-model";

import { useBackend } from "../../features/applications/backend";
import type { DesignerBinding } from "../../features/applications/draftSync";
import { assemblyFromDocument } from "../../features/applications/document";
import { useAppDetail } from "../../features/applications/hooks";
import { useLifecycle } from "../../features/applications/lifecycle";
import type { AppDetailView, AppBackend } from "../../features/applications/model";
import { APP_TABS, onLinkClick, paths, type AppTab } from "../../features/router";
import { EditorApp } from "../../EditorApp";
import { AppShell } from "./AppShell";
import { DeployVersionDialog } from "./DeployVersionDialog";
import { LifecycleActions } from "./LifecycleActions";
import { PublishDialog } from "./PublishDialog";
import { SessionHost } from "../sessions/SessionHost";
import { AppIcon, DesignBadge, Empty, ErrorState, RuntimeBadge, Skeleton } from "./common";
import { Deployments } from "./tabs/Deployments";
import { Monitoring } from "./tabs/Monitoring";
import { Overview } from "./tabs/Overview";
import { Settings } from "./tabs/Settings";
import { Versions } from "./tabs/Versions";

const TAB_LABEL: Record<AppTab, string> = { overview: "Overview", designer: "Designer", deployments: "Deployments", monitoring: "Monitoring", versions: "Versions", settings: "Settings" };

type DraftLoad = { kind: "loading" } | { kind: "error"; message: string } | { kind: "ready"; assembly: ApplicationAssembly; version: number };

/** Loads what the Designer opens: the draft; else the latest version; else a blank canvas. Invalid documents are never replaced silently. */
async function loadForDesigner(backend: AppBackend, name: string, latest: number | null | undefined): Promise<DraftLoad> {
  const draft = await backend.getDraft(name);
  let doc: unknown;
  let version = 0;
  if (draft) { doc = draft.document; version = draft.version; }
  else if (latest) doc = await backend.versionDocument(name, latest);
  else return { kind: "ready", assembly: createEmptyAssembly(name), version: 0 };
  const r = assemblyFromDocument(doc);
  if (!r.ok) return { kind: "error", message: `The saved document cannot be opened: ${r.errors.slice(0, 3).join("; ")}` };
  return { kind: "ready", assembly: r.assembly, version };
}

function DesignerTab({ detail, onChanged }: { detail: AppDetailView; onChanged: () => void }) {
  const backend = useBackend((s) => s.backend);
  const name = detail.application.name;
  const latest = detail.application.latestRevision;
  const [load, setLoad] = useState<DraftLoad>({ kind: "loading" });
  const [publishing, setPublishing] = useState(false);
  const [deploying, setDeploying] = useState(false);
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    let live = true;
    setLoad({ kind: "loading" });
    loadForDesigner(backend, name, latest).then((r) => { if (live) setLoad(r); }, (e: unknown) => { if (live) setLoad({ kind: "error", message: e instanceof Error ? e.message : String(e) }); });
    return () => { live = false; };
    // The document is loaded once per application and backend; later status refreshes must not replace what is being edited.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [backend, name, attempt]);
  const binding = useMemo<DesignerBinding | undefined>(
    () => (load.kind === "ready" ? { name, backend, baseVersion: load.version, projectLabel: detail.application.project.name, onPublish: () => setPublishing(true), onDeploy: () => setDeploying(true) } : undefined),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [load, name, backend],
  );
  if (load.kind === "loading") return <div className="p-6" data-testid="designer-loading"><Skeleton rows={2} /></div>;
  if (load.kind === "error") {
    return (
      <div className="mx-auto max-w-xl p-6">
        <ErrorState message={load.message} onRetry={() => setAttempt((n) => n + 1)} />
        <a href={paths.application(name)} onClick={onLinkClick} className="btn btn-sm mt-3">Back to the application</a>
      </div>
    );
  }
  return (
    <>
      <EditorApp key={`${name}:${attempt}`} initial={load.assembly} binding={binding} />
      {publishing && <PublishDialog name={name} onClose={() => setPublishing(false)} onDone={onChanged} />}
      {deploying && <DeployVersionDialog app={detail.application} versions={detail.versions} onClose={() => setDeploying(false)} onDone={onChanged} />}
    </>
  );
}

function Workspace({ app: name, tab }: { app: string; tab: AppTab }) {
  const [detail, reload] = useAppDetail(name);
  const backend = useBackend((s) => s.backend);
  const bump = useLifecycle((s) => s.bump);
  const [publishing, setPublishing] = useState(false);
  const [deploy, setDeploy] = useState<{ version?: number } | null>(null);
  useEffect(() => { if (bump) reload(); }, [bump]); // eslint-disable-line react-hooks/exhaustive-deps

  if (detail.kind === "loading") return <main className="mx-auto max-w-6xl p-6"><Skeleton rows={3} /></main>;
  if (detail.kind === "error") {
    const missing = /does not exist|not found|404/i.test(detail.message);
    return (
      <main className="mx-auto flex max-w-6xl flex-col gap-3 p-6">
        {missing ? <Empty title="Application not found" action={<a href={paths.applications()} onClick={onLinkClick} className="btn btn-sm btn-primary">Back to applications</a>}>There is no application called “{name}”, or you do not have access to it.</Empty> : <ErrorState message={detail.message} onRetry={reload} />}
      </main>
    );
  }
  const d = detail.data;
  const a = d.application;
  const canDeploy = backend.kind === "api";
  const deployReason = canDeploy ? null : "Deploying needs a control plane. Connect to one from the header.";

  if (tab === "designer") return <div className="h-screen" data-testid="designer-tab"><DesignerTab detail={d} onChanged={reload} /></div>;

  return (
    <main className="mx-auto flex w-full max-w-6xl flex-col gap-4 px-4 py-5" data-testid="application-workspace" data-app={a.name}>
      <nav className="text-xs text-base-content/60" aria-label="Breadcrumb"><a href={paths.applications()} onClick={onLinkClick} className="hover:underline">Applications</a> / <span className="text-base-content">{a.name}</span></nav>
      <header className="flex flex-wrap items-center gap-3">
        <AppIcon icon={a.icon} size={44} />
        <div className="min-w-0 flex-1">
          <h1 className="truncate text-xl font-semibold" data-testid="workspace-name">{a.name}</h1>
          <p className="truncate text-xs text-base-content/60">{a.description || "No description"}</p>
        </div>
        <div className="flex items-center gap-2"><DesignBadge app={a} /><RuntimeBadge app={a} /></div>
        <LifecycleActions app={a} size="sm" />
      </header>
      {a.archived && <div role="status" className="rounded-box border border-warning/50 bg-warning/10 px-3 py-2 text-xs" data-testid="archived-banner">This application is archived. Restore it in Settings to use it again.</div>}
      <div role="tablist" className="tabs tabs-border border-b border-base-300" aria-label="Application sections">
        {APP_TABS.map((t) => (
          <a key={t} role="tab" href={paths.application(a.name, t)} onClick={onLinkClick} className={`tab ${t === tab ? "tab-active text-base-content" : "text-base-content/80"}`} aria-selected={t === tab} data-testid={`tab-${t}`}>{TAB_LABEL[t]}</a>
        ))}
      </div>
      {tab === "overview" && <Overview detail={d} onPublish={() => setPublishing(true)} />}
      {tab === "deployments" && <Deployments detail={d} onDeploy={(version) => setDeploy({ version })} canDeploy={canDeploy} deployReason={deployReason} />}
      {tab === "monitoring" && <Monitoring detail={d} />}
      {tab === "versions" && <Versions detail={d} onPublish={() => setPublishing(true)} onDeploy={(version) => setDeploy({ version })} canDeploy={canDeploy} deployReason={deployReason} />}
      {tab === "settings" && <Settings app={a} onChanged={reload} />}
      {publishing && <PublishDialog name={a.name} onClose={() => setPublishing(false)} onDone={reload} />}
      {deploy && <DeployVersionDialog app={a} versions={d.versions} initial={deploy.version} onClose={() => setDeploy(null)} onDone={reload} />}
    </main>
  );
}

/** One application's own workspace. The Designer is the full-screen editor; every other tab sits inside the Applications shell. */
export function ApplicationRoute({ app, tab }: { app: string; tab: AppTab }) {
  return (
    <AppShell route={{ name: "application", app, tab }} bare={tab === "designer"}>
      <Workspace app={app} tab={tab} />
      <SessionHost app={app} />
    </AppShell>
  );
}

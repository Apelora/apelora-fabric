import { useEffect, useMemo, useState } from "react";

import { createStudioStore } from "../../features/editor/store";
import { StudioContext } from "../../features/editor/context";
import { loadView } from "../../features/editor/persistence";
import { useBackend } from "../../features/applications/backend";
import { assemblyFromDocument } from "../../features/applications/document";
import type { AppView, VersionInfo } from "../../features/applications/model";
import type { ApplicationAssembly } from "@apelora/assembly-model";
import { DeployDialog } from "../deploy/DeployDialog";

/** Loads one published version and opens the deployment review for exactly that version. */
function DeployLoaded({ app, version, onClose, onDone }: { app: AppView; version: number; onClose: () => void; onDone: () => void }) {
  const backend = useBackend((s) => s.backend);
  const [state, setState] = useState<{ kind: "loading" } | { kind: "error"; message: string } | { kind: "ready"; assembly: ApplicationAssembly }>({ kind: "loading" });
  useEffect(() => {
    let live = true;
    backend.versionDocument(app.name, version).then(
      (doc) => {
        if (!live) return;
        const r = assemblyFromDocument(doc);
        setState(r.ok ? { kind: "ready", assembly: r.assembly } : { kind: "error", message: `Version ${version} cannot be read: ${r.errors[0] ?? "invalid document"}` });
      },
      (e: unknown) => { if (live) setState({ kind: "error", message: e instanceof Error ? e.message : String(e) }); },
    );
    return () => { live = false; };
  }, [backend, app.name, version]);
  // DeployDialog reads the editor store for toasts; give it a throwaway one holding this version.
  const store = useMemo(() => (state.kind === "ready" ? createStudioStore(state.assembly, loadView()) : null), [state]);
  if (state.kind === "loading") return null;
  if (state.kind === "error" || !store) {
    return (
      <dialog className="modal modal-open" aria-labelledby="dv-err">
        <div className="modal-box max-w-md"><h3 id="dv-err" className="font-semibold">Cannot deploy version {version}</h3><p role="alert" className="mt-2 text-sm text-error">{state.kind === "error" ? state.message : ""}</p><div className="modal-action"><button className="btn btn-sm" onClick={onClose}>Close</button></div></div>
      </dialog>
    );
  }
  const project = app.environment?.incusProject ?? app.project.slug;
  return (
    <StudioContext.Provider value={store}>
      <DeployDialog open onClose={onClose} onDone={onDone} target={{ application: app.name, version, project, assembly: state.assembly }} />
    </StudioContext.Provider>
  );
}

/**
 * Deploys a published version. With `initial` it goes straight to the review; without, it first asks
 * which version. Only the control plane can deploy, so without one this says why instead.
 */
export function DeployVersionDialog({ app, versions, initial, onClose, onDone }: { app: AppView; versions: VersionInfo[]; initial?: number; onClose: () => void; onDone?: () => void }) {
  const backend = useBackend((s) => s.backend);
  const published = versions.filter((v) => v.published).sort((a, b) => b.revision - a.revision);
  const [version, setVersion] = useState<number | undefined>(initial);
  const [pick, setPick] = useState<number | undefined>(published[0]?.revision);
  if (version !== undefined) return <DeployLoaded app={app} version={version} onClose={onClose} onDone={() => onDone?.()} />;
  return (
    <dialog className="modal modal-open" aria-labelledby="dv-title" data-testid="deploy-version-dialog">
      <div className="modal-box max-w-md">
        <h3 id="dv-title" className="text-base font-semibold">Deploy a version of {app.name}</h3>
        {backend.kind !== "api" ? (
          <p className="mt-2 text-sm text-base-content/80" data-testid="deploy-needs-control-plane">Deploying needs a control plane. Applications in this browser can be designed and published, but nothing here can run. Connect to a control plane from the header.</p>
        ) : published.length === 0 ? (
          <p className="mt-2 text-sm text-base-content/80" data-testid="deploy-no-versions">Nothing is published yet. Publish a version from the Designer first; drafts cannot be deployed.</p>
        ) : (
          <label className="mt-3 flex flex-col gap-1 text-xs">Version
            <select className="select select-sm" value={pick} onChange={(e) => setPick(Number(e.target.value))} data-testid="deploy-version-select">
              {published.map((v) => <option key={v.revision} value={v.revision}>Version {v.revision}{v.label ? ` · ${v.label}` : ""}</option>)}
            </select>
          </label>
        )}
        <div className="modal-action">
          <button type="button" className="btn btn-sm" onClick={onClose}>Cancel</button>
          {backend.kind === "api" && published.length > 0 && <button type="button" className="btn btn-sm btn-primary" disabled={pick === undefined} onClick={() => setVersion(pick)} data-testid="deploy-version-continue">Review deployment</button>}
        </div>
      </div>
      <div className="modal-backdrop" onClick={onClose} />
    </dialog>
  );
}

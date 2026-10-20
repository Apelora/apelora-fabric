import { useState } from "react";

import { useBackend } from "../../../features/applications/backend";
import { AppError, type AppView } from "../../../features/applications/model";
import { useAppToasts } from "../../../features/applications/toast";
import { navigate, paths } from "../../../features/router";
import { IconPicker } from "../IconPicker";
import { AppIcon } from "../common";

export function Settings({ app, onChanged }: { app: AppView; onChanged: () => void }) {
  const backend = useBackend((s) => s.backend);
  const toast = useAppToasts((s) => s.push);
  const [description, setDescription] = useState(app.description);
  const [icon, setIcon] = useState(app.icon);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [archiving, setArchiving] = useState(false);
  const dirty = description !== app.description || icon !== app.icon;
  const running = app.deployment.status !== "not-deployed" && app.deployment.status !== "stopped" && app.deployment.status !== "unknown";

  const save = async (e: React.FormEvent) => {
    e.preventDefault();
    if (busy || !dirty) return;
    setBusy(true);
    setError(null);
    try {
      await backend.update(app.name, { ...(description !== app.description ? { description } : {}), ...(icon !== app.icon ? { icon } : {}) });
      toast("success", `Saved the settings of ${app.name}.`);
      onChanged();
    } catch (err) {
      setError(err instanceof AppError ? err.message : String(err));
    } finally { setBusy(false); }
  };
  const archive = async (archived: boolean) => {
    setBusy(true);
    setError(null);
    try {
      await backend.archive(app.name, archived);
      toast("success", archived ? `${app.name} is archived. Show archived applications to find it.` : `${app.name} is restored.`);
      if (archived) navigate(paths.applications()); else onChanged();
    } catch (err) {
      setError(err instanceof AppError ? err.message : String(err));
      setArchiving(false);
    } finally { setBusy(false); }
  };

  return (
    <div className="grid gap-6 lg:grid-cols-[1fr_20rem]" data-testid="tab-settings">
      <form className="flex max-w-xl flex-col gap-4" onSubmit={(e) => void save(e)}>
        <label className="flex flex-col gap-1 text-xs">Name<input className="input input-sm" value={app.name} readOnly aria-readonly="true" data-testid="settings-name" /><span className="text-base-content/70">The name identifies the application and cannot change.</span></label>
        <label className="flex flex-col gap-1 text-xs">Description
          <textarea className="textarea textarea-sm h-20" value={description} maxLength={500} onChange={(e) => setDescription(e.target.value)} data-testid="settings-description" />
        </label>
        <div className="flex flex-col gap-1 text-xs">
          <span className="flex items-center gap-2">Icon <AppIcon icon={icon} size={24} /></span>
          <IconPicker value={icon} onChange={setIcon} idPrefix="settings-icon" />
        </div>
        {error && <p role="alert" className="rounded-box border border-error/40 bg-error/5 px-3 py-2 text-xs text-error" data-testid="settings-error">{error}</p>}
        <div className="flex gap-2">
          <button type="submit" className="btn btn-sm btn-primary" disabled={!dirty || busy} data-testid="settings-save">{busy ? "Saving…" : "Save changes"}</button>
          <button type="button" className="btn btn-sm" disabled={!dirty || busy} onClick={() => { setDescription(app.description); setIcon(app.icon); }}>Discard</button>
        </div>
      </form>
      <aside className="flex flex-col gap-4">
        <dl className="grid grid-cols-2 gap-1 rounded-box border border-base-300 p-3 text-xs">
          <dt className="text-base-content/60">Project</dt><dd data-testid="settings-project">{app.project.name}</dd>
          <dt className="text-base-content/60">Environment</dt><dd data-testid="settings-environment">{app.environment?.name ?? "—"}</dd>
          <dt className="text-base-content/60">Created</dt><dd>{new Date(app.createdAt).toLocaleDateString()}</dd>
        </dl>
        <section className="flex flex-col gap-2 rounded-box border border-base-300 p-3">
          <h2 className="text-xs font-semibold">{app.archived ? "Archived" : "Archive"}</h2>
          {app.archived ? (
            <button type="button" className="btn btn-sm" disabled={busy} onClick={() => void archive(false)} data-testid="restore-app">Restore application</button>
          ) : archiving ? (
            <div className="flex flex-col gap-2 text-xs">
              <p>{running ? `${app.name} is deployed. Archiving hides it from the list; it does not stop or remove anything that runs.` : "Archiving hides it from the list. Its drafts and versions are kept."}</p>
              <div className="flex gap-2"><button type="button" className="btn btn-xs" onClick={() => setArchiving(false)}>Cancel</button><button type="button" className="btn btn-xs btn-error" disabled={busy} onClick={() => void archive(true)} data-testid="archive-confirm">Archive</button></div>
            </div>
          ) : (
            <button type="button" className="btn btn-sm" onClick={() => setArchiving(true)} data-testid="archive-app">Archive application…</button>
          )}
        </section>
      </aside>
    </div>
  );
}

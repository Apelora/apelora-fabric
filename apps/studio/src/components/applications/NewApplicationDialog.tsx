import { useEffect, useMemo, useState } from "react";

import { TEMPLATES } from "@apelora/module-catalog";
import { applicationName, randomUid, serializeAssembly, type ApplicationAssembly } from "@apelora/assembly-model";

import { useBackend } from "../../features/applications/backend";
import { AppError, type EnvChoice } from "../../features/applications/model";
import { useAppToasts } from "../../features/applications/toast";
import { loadAssembly } from "../../features/editor/persistence";
import { navigate, paths } from "../../features/router";
import { IconPicker } from "./IconPicker";

const NAME = /^[a-z](?:[a-z0-9-]{0,38}[a-z0-9])?$/;

/** Creates an application: where it lives, what it is called, its icon, and what to start from. */
export function NewApplicationDialog({ onClose, imported }: { onClose: () => void; imported?: { assembly: ApplicationAssembly; fileName: string } }) {
  const backend = useBackend((s) => s.backend);
  const toast = useAppToasts((s) => s.push);
  const [name, setName] = useState(() => (imported ? applicationName(imported.assembly.metadata.name) : ""));
  const [description, setDescription] = useState("");
  const [icon, setIcon] = useState("generic-box");
  const [env, setEnv] = useState("");
  const [envs, setEnvs] = useState<EnvChoice[] | null>(null);
  const [envError, setEnvError] = useState<string | null>(null);
  const [from, setFrom] = useState<"blank" | "template" | "library" | "file">(imported ? "file" : "blank");
  const [template, setTemplate] = useState(TEMPLATES[0]?.id ?? "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    backend.environments().then((e) => { if (live) { setEnvs(e); setEnv((cur) => cur || e[0]?.id || ""); } }, (e: unknown) => { if (live) { setEnvs([]); setEnvError(e instanceof Error ? e.message : String(e)); } });
    return () => { live = false; };
  }, [backend]);
  useEffect(() => {
    const on = (e: KeyboardEvent) => { if (e.key === "Escape") onClose(); };
    window.addEventListener("keydown", on);
    return () => window.removeEventListener("keydown", on);
  }, [onClose]);

  const nameError = name && !NAME.test(name) ? "Lowercase letters, digits and hyphens, starting with a letter (at most 40). It cannot be changed later." : null;
  const library = useMemo(() => loadAssembly().assembly, []);
  const ready = !!name && !nameError && !!env && !busy;

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!ready) return;
    setBusy(true);
    setError(null);
    try {
      let document: unknown;
      if (from === "template") document = JSON.parse(serializeAssembly({ ...TEMPLATES.find((t) => t.id === template)!.build(), metadata: { name, uid: `assembly-${randomUid()}` } }));
      else if (from === "file" && imported) document = JSON.parse(serializeAssembly({ ...imported.assembly, metadata: { ...imported.assembly.metadata, name } }));
      else if (from === "library" && library) document = JSON.parse(serializeAssembly({ ...library, metadata: { ...library.metadata, name } }));
      await backend.create({ name, description: description.trim(), icon, environmentId: env, document });
      toast("success", `Created ${name}.`);
      onClose();
      navigate(paths.application(name, "designer"));
    } catch (err) {
      setError(err instanceof AppError ? err.message : String(err));
      setBusy(false);
    }
  };

  return (
    <dialog className="modal modal-open" aria-labelledby="new-app-title" data-testid="new-application-dialog">
      <form className="modal-box flex max-h-[92vh] max-w-2xl flex-col gap-4 overflow-y-auto" onSubmit={(e) => void submit(e)}>
        <div>
          <h3 id="new-app-title" className="text-base font-semibold">{imported ? "Import an application" : "New application"}</h3>
          <p className="text-xs text-base-content/60">An application is a design you can version, deploy and operate. Its name becomes the prefix of its instances.</p>
        </div>
        <div className="grid gap-3 sm:grid-cols-2">
          <label className="flex flex-col gap-1 text-sm">
            <span className="font-medium">Name</span>
            <input className="input input-sm w-full font-mono" value={name} maxLength={40} autoFocus onChange={(e) => setName(e.target.value.toLowerCase())} aria-invalid={!!nameError} aria-describedby="new-app-name-hint" data-testid="new-app-name" placeholder="e.g. customer-portal" />
            <span id="new-app-name-hint" className={`text-[11px] ${nameError ? "text-error" : "text-base-content/60"}`}>{nameError ?? "Letters, digits and hyphens. Fixed once created."}</span>
          </label>
          <label className="flex flex-col gap-1 text-sm">
            <span className="font-medium">Where it lives</span>
            <select className="select select-sm w-full" value={env} onChange={(e) => setEnv(e.target.value)} disabled={!envs?.length} data-testid="new-app-env" aria-label="Project and environment">
              {envs === null && <option>Loading…</option>}
              {envs?.map((e) => <option key={e.id} value={e.id}>{e.label}</option>)}
            </select>
            {envs && envs.length === 0 && <span className="text-[11px] text-error" data-testid="new-app-no-env">{envError ?? "You have no environment you may create applications in. An administrator can create one under Identity & Access."}</span>}
          </label>
        </div>
        <label className="flex flex-col gap-1 text-sm">
          <span className="font-medium">Description <span className="font-normal text-base-content/60">(optional)</span></span>
          <textarea className="textarea textarea-sm h-16 w-full" maxLength={200} value={description} onChange={(e) => setDescription(e.target.value)} data-testid="new-app-description" />
        </label>
        <div className="flex flex-col gap-1 text-sm">
          <span className="font-medium">Icon</span>
          <IconPicker value={icon} onChange={setIcon} idPrefix="new-icon" />
        </div>
        <fieldset className="flex flex-col gap-2 text-sm">
          <legend className="mb-1 font-medium">Start from</legend>
          {imported && <label className="flex items-center gap-2"><input type="radio" className="radio radio-xs" name="from" checked={from === "file"} onChange={() => setFrom("file")} data-testid="from-file" />The imported file {imported.fileName} ({imported.assembly.spec.modules.length} modules)</label>}
          <label className="flex items-center gap-2"><input type="radio" className="radio radio-xs" name="from" checked={from === "blank"} onChange={() => setFrom("blank")} data-testid="from-blank" />A blank canvas</label>
          <label className="flex flex-wrap items-center gap-2">
            <input type="radio" className="radio radio-xs" name="from" checked={from === "template"} onChange={() => setFrom("template")} data-testid="from-template" />
            A template
            <select className="select select-xs" value={template} onChange={(e) => { setTemplate(e.target.value); setFrom("template"); }} aria-label="Template" data-testid="new-app-template">
              {TEMPLATES.map((t) => <option key={t.id} value={t.id}>{t.title}</option>)}
            </select>
          </label>
          <label className={`flex items-center gap-2 ${library ? "" : "opacity-50"}`}><input type="radio" className="radio radio-xs" name="from" disabled={!library} checked={from === "library"} onChange={() => setFrom("library")} data-testid="from-library" />A copy of the diagram in the Assembly Library{library ? ` (${library.spec.modules.length} modules)` : " (none saved)"}</label>
        </fieldset>
        {error && <p role="alert" className="text-sm text-error" data-testid="new-app-error">{error}</p>}
        <div className="modal-action mt-0">
          <button type="button" className="btn btn-sm" onClick={onClose}>Cancel</button>
          <button type="submit" className="btn btn-sm btn-primary" disabled={!ready} data-testid="new-app-create">{busy ? "Creating…" : imported ? "Import application" : "Create application"}</button>
        </div>
      </form>
      <div className="modal-backdrop" onClick={onClose} />
    </dialog>
  );
}

import { useId } from "react";
import { effectiveValue, validateProperty, type ModuleInstance, type ConfigValue, type Property } from "@apelora/assembly-model";
import { lookupClass } from "@apelora/module-catalog";
import { useStudioApi } from "../../features/editor/context";

function Field({ app, prop }: { app: ModuleInstance; prop: Property }) {
  const api = useStudioApi();
  const uid = useId();
  const override = app.config?.[prop.id];
  const value = effectiveValue(prop, app.config);
  const problem = validateProperty(prop, value);
  const modified = override !== undefined;
  const commit = (v: ConfigValue | undefined) => api.getState().setCfg(app.id, prop.id, v === prop.default ? undefined : v);
  const common = { id: uid, "data-testid": `cfg-${prop.id}`, "aria-invalid": !!problem, "aria-describedby": `${uid}-d` } as const;

  let control;
  if (prop.type === "bool") {
    control = <input type="checkbox" className="toggle toggle-sm" {...common} checked={value === true} onChange={(e) => commit(e.target.checked)} />;
  } else if (prop.type === "enum") {
    control = <select className={`select select-sm w-full ${problem ? "select-error" : ""}`} {...common} value={String(value ?? "")} onChange={(e) => commit(e.target.value)}>{!prop.required && <option value="">(default)</option>}{(prop.options ?? []).map((o) => <option key={o}>{o}</option>)}</select>;
  } else if (prop.type === "int") {
    control = (
      <input key={String(value)} type="number" step={1} min={prop.min} max={prop.max} className={`input input-sm w-full ${problem ? "input-error" : ""}`} {...common} defaultValue={value === undefined ? "" : String(value)}
        onBlur={(e) => { const t = e.currentTarget.value.trim(); if (t === "") { commit(undefined); return; } const n = Number(t); if (Number.isFinite(n)) commit(n); else e.currentTarget.value = String(value ?? ""); }}
        onKeyDown={(e) => { if (e.key === "Enter") e.currentTarget.blur(); }} />
    );
  } else {
    control = (
      <input key={String(value)} type="text" maxLength={256} className={`input input-sm w-full ${problem ? "input-error" : ""} ${prop.type === "secret" ? "font-mono" : ""}`} {...common} defaultValue={String(value ?? "")}
        placeholder={prop.type === "secret" ? "secret reference name, e.g. db-password" : undefined} autoComplete="off" spellCheck={false}
        onBlur={(e) => { const t = e.currentTarget.value; commit(t === "" ? undefined : t); }} onKeyDown={(e) => { if (e.key === "Enter") e.currentTarget.blur(); }} />
    );
  }
  return (
    <div className="flex flex-col gap-0.5" data-testid="cfg-field" data-prop-id={prop.id} data-modified={modified}>
      <div className="flex items-center justify-between gap-2">
        <label htmlFor={uid} className="text-[11px] font-semibold">{prop.label}{prop.required && <span className="text-error" aria-hidden="true"> *</span>}{modified && <span className="ml-1 text-base-content/70" title="Changed from the default">•</span>}</label>
        {modified && <button type="button" className="link text-[10px]" data-testid={`cfg-reset-${prop.id}`} onClick={() => api.getState().setCfg(app.id, prop.id, undefined)}>reset</button>}
      </div>
      {prop.type === "bool" ? <div>{control}</div> : control}
      <p id={`${uid}-d`} className={`text-[10px] leading-snug ${problem ? "text-error" : "text-base-content/70"}`} role={problem ? "alert" : undefined} data-testid={problem ? "cfg-error" : undefined}>
        {problem ? `${problem.message} ${problem.hint}` : `${prop.supply === "deploy" ? "Asked when the application is deployed. " : ""}${prop.type === "secret" ? `Reference name only; the secret itself is never stored here.${prop.description ? ` ${prop.description}` : ""}` : prop.description ?? ""}`}
      </p>
    </div>
  );
}

export function ConfigForm({ app }: { app: ModuleInstance }) {
  const api = useStudioApi();
  const cls = lookupClass(app.classRef.name, app.classRef.version);
  const props = cls?.properties ?? [];
  if (props.length === 0) return null;
  const groups = new Map<string, Property[]>();
  for (const p of props) groups.set(p.group ?? "General", [...(groups.get(p.group ?? "General") ?? []), p]);
  const overrides = Object.keys(app.config ?? {}).length;
  return (
    <section aria-label="Configuration" data-testid="config-form">
      <div className="mb-1 flex items-center justify-between">
        <h4 className="text-[11px] font-semibold uppercase tracking-wide text-base-content/60">Configuration</h4>
        <button type="button" className="btn btn-ghost btn-xs" data-testid="cfg-reset-all" disabled={overrides === 0} onClick={() => api.getState().resetCfg(app.id)}>Reset all</button>
      </div>
      <div className="flex flex-col gap-2">
        {[...groups].map(([name, list], i) => (
          <details key={name} open={i === 0 || list.some((p) => app.config?.[p.id] !== undefined || validateProperty(p, effectiveValue(p, app.config)) !== null)} className="apl-details rounded-box border border-base-300" data-testid="cfg-group" data-group={name}>
            <summary className="flex cursor-pointer select-none items-center justify-between px-2 py-1 text-[11px] font-medium">{name}<span className="text-base-content/70">{list.length}</span></summary>
            <div className="flex flex-col gap-2.5 border-t border-base-300 p-2">{list.map((p) => <Field key={p.id} app={app} prop={p} />)}</div>
          </details>
        ))}
      </div>
    </section>
  );
}

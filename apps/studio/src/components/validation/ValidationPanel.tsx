import { useState } from "react";
import { useReactFlow } from "@xyflow/react";
import type { Diagnostic } from "@apelora/assembly-model";
import { useStudioApi } from "../../features/editor/context";
import { IconAlert, IconCheck, IconChevron } from "../icons";

export function ValidationPanel({ diagnostics }: { diagnostics: Diagnostic[] }) {
  const api = useStudioApi();
  const { fitView } = useReactFlow();
  const [open, setOpen] = useState(false);
  const errors = diagnostics.filter((d) => d.severity === "error").length;
  const warnings = diagnostics.length - errors;

  const focus = (d: Diagnostic) => {
    if (!d.applianceId) return;
    api.getState().selectNode(d.applianceId);
    void fitView({ nodes: [{ id: d.applianceId }], padding: 0.8, maxZoom: 1.2, duration: 250 });
  };

  return (
    <div className="absolute inset-x-3 bottom-3 z-10 flex justify-start" data-testid="validation-panel">
      <div className="max-w-full rounded-box border border-base-300 bg-base-100 text-xs shadow-sm" style={{ width: open ? 460 : "auto" }}>
        <button type="button" className="flex w-full items-center gap-2 px-3 py-1.5 text-left" aria-expanded={open} onClick={() => setOpen(!open)} data-testid="validation-toggle">
          {diagnostics.length === 0 ? <span className="text-success"><IconCheck size={14} /></span> : <span className={errors ? "text-error" : "text-warning"}><IconAlert size={14} /></span>}
          <span className="font-medium">{diagnostics.length === 0 ? "No issues" : "Validation"}</span>
          {errors > 0 && <span className="badge badge-error badge-soft badge-sm" data-testid="error-count">{errors} error{errors > 1 ? "s" : ""}</span>}
          {warnings > 0 && <span className="badge badge-warning badge-soft badge-sm" data-testid="warning-count">{warnings} warning{warnings > 1 ? "s" : ""}</span>}
          <span className={`ml-auto transition-transform ${open ? "-rotate-90" : "rotate-90"}`}><IconChevron size={14} /></span>
        </button>
        {open && (
          <ul className="max-h-44 divide-y divide-base-300 overflow-y-auto border-t border-base-300" data-testid="diagnostics">
            {diagnostics.length === 0 && <li className="px-3 py-2 text-base-content/60">The assembly is structurally valid. Planning checks against real hosts are not available in this preview.</li>}
            {diagnostics.map((d, i) => (
              <li key={`${d.code}-${i}`}>
                <button type="button" className="flex w-full flex-col items-start gap-0.5 px-3 py-1.5 text-left hover:bg-base-200 disabled:hover:bg-transparent" onClick={() => focus(d)} disabled={!d.applianceId}>
                  <span className="flex items-center gap-1.5"><span className={`badge badge-xs ${d.severity === "error" ? "badge-error" : "badge-warning"}`}>{d.severity}</span>{d.message}</span>
                  {d.hint && <span className="pl-1 text-base-content/60">{d.hint}</span>}
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}

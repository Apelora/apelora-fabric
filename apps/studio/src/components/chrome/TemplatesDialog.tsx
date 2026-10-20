import { TEMPLATES } from "@apelora/appliance-catalog";
import { PLATFORM_INFO, platformOf, setPlatform } from "@apelora/assembly-model";
import { useStudio, useStudioApi } from "../../features/editor/context";
import { Modal } from "./Modal";

/** Starter diagrams built only from catalog appliances. Choosing one replaces the canvas; undo brings the old diagram back. */
export function TemplatesDialog() {
  const api = useStudioApi();
  const open = useStudio((s) => s.dialog === "templates");
  const target = useStudio((s) => platformOf(s.assembly));
  if (!open) return null;
  const close = () => api.getState().setDialog(null);
  return (
    <Modal testId="templates-dialog" title="Start from a template" hint={`Ready-made diagrams for the ${PLATFORM_INFO[target].name} target. Your current diagram is kept in undo history.`} onClose={close} size="max-w-3xl">
      <ul className="grid gap-3 sm:grid-cols-2">
        {TEMPLATES.map((t) => (
          <li key={t.id}>
            <button type="button" className="flex h-full w-full flex-col items-start gap-1.5 rounded-box border border-base-300 p-3 text-left hover:border-base-content/40 hover:bg-base-200" data-testid="template-card" data-template-id={t.id}
              onClick={() => { api.getState().replace(setPlatform(t.build(), target)); api.getState().setDialog(null); api.getState().toast("success", `Opened the “${t.title}” template. Undo restores the previous diagram.`); }}>
              <span className="text-sm font-semibold">{t.title}</span>
              <span className="text-xs text-base-content/70">{t.summary}</span>
              <span className="mt-auto flex flex-wrap gap-1 pt-1">{t.tags.map((x) => <span key={x} className="badge badge-ghost badge-xs">{x}</span>)}</span>
            </button>
          </li>
        ))}
      </ul>
    </Modal>
  );
}

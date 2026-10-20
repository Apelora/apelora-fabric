import { useStudio, useStudioApi } from "../../features/editor/context";
import { Modal } from "./Modal";

const GROUPS: { title: string; rows: [string, string][] }[] = [
  { title: "Canvas", rows: [["V", "Select tool"], ["H", "Pan tool (or hold Space)"], ["F", "Fit to view"], ["+ / −", "Zoom in / out"], ["Ctrl+K", "Find a module"], ["Arrow keys", "Nudge the selection (Shift = 10×)"]] },
  { title: "Editing", rows: [["Ctrl+Z / Ctrl+Shift+Z", "Undo / redo"], ["Ctrl+C / V / D", "Copy / paste / duplicate"], ["Ctrl+A", "Select all"], ["Del", "Delete the selection"], ["F2", "Rename"], ["Enter", "Open properties"]] },
  { title: "Structure", rows: [["Ctrl+G", "Group into a component"], ["Ctrl+Shift+G", "Ungroup"], ["Shift+T", "Tidy layout"], ["Right-click", "Every action for what you click on"], ["?", "This sheet"]] },
];

export function HelpDialog() {
  const api = useStudioApi();
  const open = useStudio((s) => s.dialog === "help");
  if (!open) return null;
  const close = () => api.getState().setDialog(null);
  return (
    <Modal testId="help-dialog" title="Keyboard shortcuts" hint="Everything here is also in the right-click menus." onClose={close}
      footer={<button type="button" className="btn btn-sm" data-testid="tour-start" onClick={() => { api.getState().setDialog(null); window.dispatchEvent(new Event("apelora:tour")); }}>Take the tour</button>}>
      <div className="grid gap-5 sm:grid-cols-3">
        {GROUPS.map((g) => (
          <section key={g.title} aria-label={g.title}>
            <h4 className="mb-1 text-[11px] font-semibold uppercase tracking-wide text-base-content/60">{g.title}</h4>
            <dl className="flex flex-col gap-1.5 text-xs">{g.rows.map(([k, v]) => <div key={k} className="flex flex-col"><dt><kbd className="kbd kbd-xs">{k}</kbd></dt><dd className="text-base-content/70">{v}</dd></div>)}</dl>
          </section>
        ))}
      </div>
    </Modal>
  );
}

import { onLinkClick, paths, type AppTab } from "../../features/router";
import { IconMore } from "../icons";

const ITEMS: { tab: AppTab; label: string }[] = [
  { tab: "overview", label: "Overview" },
  { tab: "designer", label: "Open designer" },
  { tab: "deployments", label: "Deployments" },
  { tab: "versions", label: "Versions" },
  { tab: "settings", label: "Settings" },
];

export function MoreMenu({ name }: { name: string }) {
  return (
    <div className="dropdown dropdown-end">
      <button type="button" tabIndex={0} className="btn btn-ghost btn-xs btn-square" aria-label={`More actions for ${name}`} data-testid="more-actions"><IconMore size={16} /></button>
      <ul tabIndex={0} className="menu dropdown-content z-20 mt-1 w-44 rounded-box border border-base-300 bg-base-100 p-1 text-xs shadow-sm">
        {ITEMS.map((i) => <li key={i.tab}><a href={paths.application(name, i.tab)} onClick={(e) => { onLinkClick(e); (document.activeElement as HTMLElement | null)?.blur(); }}>{i.label}</a></li>)}
        <li><button type="button" onClick={() => { void navigator.clipboard?.writeText(name); (document.activeElement as HTMLElement | null)?.blur(); }}>Copy name</button></li>
      </ul>
    </div>
  );
}

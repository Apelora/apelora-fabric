import { useMemo, useState } from "react";
import { useReactFlow } from "@xyflow/react";
import { APPLIANCE_CLASSES, logoUrl } from "@apelora/appliance-catalog";
import { CATEGORIES, DENSITY_SIZE, type ApplianceClass } from "@apelora/assembly-model";
import { useStudioApi } from "../../features/editor/context";
import { canvasBridge, CATALOG_MIME } from "../../features/editor/bridge";
import { IconSearch } from "../icons";

function Tile({ cls, onAdd }: { cls: ApplianceClass; onAdd: (c: ApplianceClass) => void }) {
  const url = logoUrl(cls.iconAssetPath);
  return (
    <button
      type="button"
      draggable
      data-testid="catalog-tile"
      data-class-id={cls.id}
      aria-label={`Add ${cls.displayName}`}
      title={`${cls.displayName} — ${cls.summary}\nDrag onto the canvas, or click to add.`}
      onDragStart={(e) => { e.dataTransfer.setData(CATALOG_MIME, cls.id); e.dataTransfer.effectAllowed = "copy"; }}
      onClick={() => onAdd(cls)}
      className="flex h-[72px] cursor-grab flex-col items-center justify-center gap-1.5 rounded-box border border-base-300 bg-base-100 p-1 text-[11px] leading-tight transition-colors hover:border-base-content/30 hover:bg-base-200 focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-base-content active:cursor-grabbing"
    >
      {url ? <img src={url} alt="" width={28} height={28} className="size-7" draggable={false} /> : <span className="size-7 rounded bg-base-300" />}
      <span className="w-full truncate text-center">{cls.displayName}</span>
    </button>
  );
}

export function ApplianceCatalog() {
  const api = useStudioApi();
  const { screenToFlowPosition } = useReactFlow();
  const [query, setQuery] = useState("");
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  const q = query.trim().toLowerCase();

  const groups = useMemo(() => CATEGORIES.map((cat) => ({
    cat,
    items: APPLIANCE_CLASSES.filter((c) => c.category === cat && (!q || `${c.displayName} ${c.id} ${c.summary}`.toLowerCase().includes(q))),
  })).filter((g) => g.items.length > 0), [q]);

  const add = (cls: ApplianceClass) => {
    const el = canvasBridge.el;
    const size = DENSITY_SIZE[api.getState().assembly.editor.density];
    const count = api.getState().assembly.spec.appliances.length;
    const r = el?.getBoundingClientRect();
    const jitter = (count % 6) * 14;
    const p = r ? screenToFlowPosition({ x: r.left + r.width / 2 + jitter, y: r.top + r.height / 2 + jitter }) : { x: 120, y: 120 };
    api.getState().addFromCatalog(cls.id, { x: p.x - size.width / 2, y: p.y - size.height / 2 });
  };

  return (
    <aside className="flex w-44 shrink-0 flex-col border-r border-base-300 bg-base-100 lg:w-48" aria-label="Appliance catalog" data-testid="catalog">
      <div className="p-2">
        <label className="input input-sm flex w-full items-center gap-1.5">
          <IconSearch size={14} />
          <input type="search" value={query} onChange={(e) => setQuery(e.target.value)} placeholder="Search appliances" aria-label="Search appliances" data-testid="catalog-search" className="grow" />
        </label>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto px-2 pb-3">
        {groups.length === 0 && <p className="px-1 py-4 text-xs text-base-content/60">No appliances match “{query}”.</p>}
        {groups.map(({ cat, items }) => {
          const open = q ? true : !collapsed[cat];
          return (
            <div key={cat} className="mb-1" data-testid="catalog-section" data-category={cat}>
              <button
                type="button"
                className="flex w-full items-center justify-between rounded px-1 py-1.5 text-left text-[11px] font-semibold uppercase tracking-wide text-base-content/70 hover:bg-base-200"
                aria-expanded={open}
                onClick={() => setCollapsed((c) => ({ ...c, [cat]: open }))}
              >
                <span className="truncate">{cat}</span>
                <span className="ml-1 flex items-center gap-1 font-normal normal-case tracking-normal text-base-content/50">{items.length}<span className={`inline-block transition-transform ${open ? "rotate-90" : ""}`}>›</span></span>
              </button>
              {open && <div className="grid grid-cols-2 gap-1.5 pb-1.5 pt-0.5">{items.map((c) => <Tile key={c.id} cls={c} onAdd={add} />)}</div>}
            </div>
          );
        })}
      </div>
    </aside>
  );
}

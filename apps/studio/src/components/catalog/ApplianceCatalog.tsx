import { useMemo, useState } from "react";
import { useReactFlow } from "@xyflow/react";
import { APPLIANCE_CLASSES, logoUrl } from "@apelora/appliance-catalog";
import { categoryColor } from "../../features/editor/categories";
import { draftFromClass, emptyDraft, useCustomCatalog } from "../../features/catalog/customCatalog";
import { CATEGORIES, DENSITY_SIZE, type ApplianceClass } from "@apelora/assembly-model";
import { useStudioApi } from "../../features/editor/context";
import { canvasBridge, CATALOG_MIME } from "../../features/editor/bridge";
import { IconPencil, IconPlus, IconSearch } from "../icons";

function Tile({ cls, onAdd, onEdit }: { cls: ApplianceClass; onAdd: (c: ApplianceClass) => void; onEdit: (c: ApplianceClass, editing: boolean) => void }) {
  const url = logoUrl(cls.iconAssetPath);
  return (
    <div className="group relative" style={{ ["--apl-cat" as string]: categoryColor(cls.category) }}>
    <button
      type="button"
      draggable
      data-testid="catalog-tile"
      data-class-id={cls.id}
      aria-label={`Add ${cls.displayName}`}
      title={`${cls.displayName} — ${cls.summary}\nDrag onto the canvas, or click to add.`}
      onDragStart={(e) => { e.dataTransfer.setData(CATALOG_MIME, cls.id); e.dataTransfer.effectAllowed = "copy"; }}
      onClick={() => onAdd(cls)}
      className="apl-tile-accent flex h-[72px] w-full cursor-grab flex-col items-center justify-center gap-1.5 rounded-box border border-base-300 bg-base-100 p-1 text-[11px] leading-tight transition-colors hover:border-base-content/30 hover:bg-base-200 focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-base-content active:cursor-grabbing"
    >
      {url ? <img src={url} alt="" width={28} height={28} className="size-7" draggable={false} /> : <span className="size-7 rounded bg-base-300" />}
      <span className="w-full truncate text-center">{cls.displayName}</span>
    </button>
    {!cls.composition && <button
      type="button"
      className="btn btn-ghost btn-xs btn-square absolute right-0.5 top-0.5 size-5 min-h-0 opacity-0 focus-visible:opacity-100 group-hover:opacity-100"
      aria-label={cls.custom ? `Edit ${cls.displayName}` : `Customize a copy of ${cls.displayName}`}
      title={cls.custom ? "Edit this custom appliance" : "Make an editable custom copy"}
      data-testid="tile-edit"
      onClick={() => onEdit(cls, !!cls.custom)}
    ><IconPencil size={11} /></button>}
    </div>
  );
}

export function ApplianceCatalog() {
  const api = useStudioApi();
  const { screenToFlowPosition } = useReactFlow();
  const [query, setQuery] = useState("");
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  const q = query.trim().toLowerCase();

  const custom = useCustomCatalog((s) => s.classes);
  const all = useMemo(() => [...APPLIANCE_CLASSES, ...custom], [custom]);
  const groups = useMemo(() => CATEGORIES.map((cat) => ({
    cat,
    items: all.filter((c) => c.category === cat && (!q || `${c.displayName} ${c.id} ${c.summary}`.toLowerCase().includes(q))),
    // The Custom section is always shown (with an empty state) unless the user is searching.
  })).filter((g) => g.items.length > 0 || (g.cat === "Custom" && !q)), [q, all]);

  const add = (cls: ApplianceClass) => {
    const el = canvasBridge.el;
    const size = DENSITY_SIZE[api.getState().assembly.editor.density];
    const r = el?.getBoundingClientRect();
    const p = r ? screenToFlowPosition({ x: r.left + r.width / 2, y: r.top + r.height / 2 }) : { x: 120, y: 120 };
    api.getState().addFromCatalog(cls.id, { x: p.x - size.width / 2, y: p.y - size.height / 2 }, { avoidOverlap: true });
  };

  return (
    <aside className="flex w-44 shrink-0 flex-col border-r border-base-300 bg-base-100 lg:w-48" aria-label="Appliance catalog" data-testid="catalog">
      <div className="p-2">
        <label className="input input-sm flex w-full items-center gap-1.5">
          <IconSearch size={14} />
          <input type="search" value={query} onChange={(e) => setQuery(e.target.value)} placeholder="Search appliances" aria-label="Search appliances" data-testid="catalog-search" className="grow" />
        </label>
        <div className="mt-1 flex items-center justify-between px-0.5 text-[10px] text-base-content/50">
          <span data-testid="catalog-count">{all.length} appliances</span>
          <span><button type="button" className="link" data-testid="catalog-collapse-all" onClick={() => setCollapsed(Object.fromEntries(CATEGORIES.map((c) => [c, true])))}>collapse all</button> · <button type="button" className="link" data-testid="catalog-expand-all" onClick={() => setCollapsed({})}>expand all</button></span>
        </div>
        <button type="button" className="btn btn-sm btn-outline mt-2 w-full" data-testid="new-appliance" onClick={() => useCustomCatalog.getState().openDesigner(emptyDraft())}><IconPlus size={14} />New appliance</button>
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
                <span className="flex min-w-0 items-center gap-1.5"><span className="size-2 shrink-0 rounded-full" style={{ background: categoryColor(cat) }} aria-hidden="true" /><span className="truncate">{cat}</span></span>
                <span className="ml-1 flex items-center gap-1 font-normal normal-case tracking-normal text-base-content/50">{items.length}<span className={`inline-block transition-transform ${open ? "rotate-90" : ""}`}>›</span></span>
              </button>
              {open && items.length === 0 && cat === "Custom" && (
                <div className="apl-catalog-grid mb-1.5 rounded-box border border-dashed border-base-300 p-2 text-[11px] leading-snug text-base-content/60" data-testid="custom-empty">
                  No custom appliances yet. Create one with its own ports and settings, save a configured appliance as one, or group several appliances (Ctrl+G).
                  <button type="button" className="btn btn-xs btn-outline mt-1.5 w-full" data-testid="custom-empty-new" onClick={() => useCustomCatalog.getState().openDesigner(emptyDraft())}>Create appliance…</button>
                </div>
              )}
              {open && items.length > 0 && <div className="apl-catalog-grid grid grid-cols-2 gap-1.5 pb-1.5 pt-0.5">{items.map((c) => <Tile key={`${c.id}@${c.version}`} cls={c} onAdd={add} onEdit={(k, editing) => useCustomCatalog.getState().openDesigner(draftFromClass(k, editing))} />)}</div>}
            </div>
          );
        })}
      </div>
    </aside>
  );
}

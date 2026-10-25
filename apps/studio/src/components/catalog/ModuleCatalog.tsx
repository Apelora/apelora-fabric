import { useCallback, useId, useMemo, useRef, useState, type ReactNode } from "react";
import { useReactFlow } from "@xyflow/react";
import { MODULE_CLASSES, logoUrl } from "@apelora/module-catalog";
import { categoryColor } from "../../features/editor/categories";
import { draftFromClass, emptyDraft, useCustomCatalog } from "../../features/catalog/customCatalog";
import { catalogTree, isOpen, searchCatalog, setAll, type CategoryGroup } from "../../features/catalog/groups";
import { DENSITY_SIZE, type ModuleClass } from "@apelora/assembly-model";
import { useStudioApi } from "../../features/editor/context";
import { loadCatalogOpen, saveCatalogOpen } from "../../features/editor/persistence";
import { readCatalog } from "../../features/applications/transfer";
import { catalogFile, saveText } from "../../features/export/file";
import { canvasBridge, CATALOG_MIME } from "../../features/editor/bridge";
import { IconChevron, IconPencil, IconPlus, IconSearch } from "../icons";

function Tile({ cls, onAdd, onEdit }: { cls: ModuleClass; onAdd: (c: ModuleClass) => void; onEdit: (c: ModuleClass, editing: boolean) => void }) {
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
      {url ? <img src={url} alt="" width={28} height={28} className="apl-logo size-7" draggable={false} /> : <span className="size-7 rounded bg-base-300" />}
      <span className="w-full truncate text-center">{cls.displayName}</span>
    </button>
    {!cls.composition && <button
      type="button"
      className="btn btn-ghost btn-xs btn-square absolute right-0.5 top-0.5 size-5 min-h-0 opacity-0 focus-visible:opacity-100 group-hover:opacity-100"
      aria-label={cls.custom ? `Edit ${cls.displayName}` : `Customize a copy of ${cls.displayName}`}
      title={cls.custom ? "Edit this custom module" : "Make an editable custom copy"}
      data-testid="tile-edit"
      onClick={() => onEdit(cls, !!cls.custom)}
    ><IconPencil size={11} /></button>}
    </div>
  );
}

function Tiles({ items, onAdd, onEdit }: { items: readonly ModuleClass[]; onAdd: (c: ModuleClass) => void; onEdit: (c: ModuleClass, editing: boolean) => void }) {
  return <div className="apl-catalog-grid grid grid-cols-2 gap-1.5 pb-1.5 pt-0.5">{items.map((c) => <Tile key={`${c.id}@${c.version}`} cls={c} onAdd={onAdd} onEdit={onEdit} />)}</div>;
}

const Chevron = ({ open }: { open: boolean }) => (
  <span className={`inline-flex shrink-0 text-base-content/60 transition-transform ${open ? "rotate-90" : ""}`} aria-hidden="true"><IconChevron size={10} /></span>
);

/** A disclosure header: a heading that holds the button which opens and closes its group. */
function Disclosure({ level, open, controls, onToggle, label, count, testId, children }: {
  level: 1 | 2; open: boolean; controls: string; onToggle: () => void; label: string; count: number; testId: string; children?: ReactNode;
}) {
  const Heading = level === 1 ? "h2" : "h3";
  const look = level === 1
    ? "gap-2 py-1.5 text-xs font-semibold text-base-content/85"
    : "gap-2 py-1 text-[11px] text-base-content/70";
  return (
    <Heading className={level === 1 ? "m-0" : "m-0 ml-4"}>
      <button
        type="button"
        aria-expanded={open}
        aria-controls={controls}
        data-testid={testId}
        onClick={onToggle}
        className={`flex w-full items-center rounded px-1 text-left hover:bg-base-200 focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-base-content ${look}`}
      >
        {children}
        <span className="min-w-0 flex-1 break-words leading-snug">{label}</span>
        <span className="flex shrink-0 items-center gap-1">
          <span className="text-[11px] font-normal tabular-nums text-base-content/70" aria-label={`${count} module${count === 1 ? "" : "s"}`}>{count}</span>
          <Chevron open={open} />
        </span>
      </button>
    </Heading>
  );
}

function CustomActions({ count }: { count: number }) {
  const api = useStudioApi();
  const input = useRef<HTMLInputElement>(null);
  return (
    <div className="mb-1.5 flex flex-col gap-1">
      <button type="button" className="btn btn-xs btn-outline w-full" data-testid="new-module" onClick={() => useCustomCatalog.getState().openDesigner(emptyDraft())}><IconPlus size={12} />Create module…</button>
      <div className="flex gap-1">
        <button type="button" className="btn btn-xs btn-ghost flex-1" data-testid="catalog-export" disabled={count === 0} onClick={() => { const file = catalogFile(useCustomCatalog.getState().classes); saveText(file.name, file.text); }}>Export</button>
        <button type="button" className="btn btn-xs btn-ghost flex-1" data-testid="catalog-import" onClick={() => input.current?.click()}>Import</button>
      </div>
      <input ref={input} type="file" accept="application/json,.json" hidden data-testid="catalog-import-input" onChange={(e) => {
        const f = e.target.files?.[0];
        e.target.value = "";
        if (!f) return;
        void readCatalog(f).then((r) => {
          if (!r.ok) { api.getState().setImportErrors(r.errors); return; }
          api.getState().toast("success", r.added === 0 ? "Those modules are already in this catalog." : `Imported ${r.added} custom module${r.added === 1 ? "" : "s"}.`);
        });
      }} />
    </div>
  );
}

export function ModuleCatalog() {
  const api = useStudioApi();
  const { screenToFlowPosition } = useReactFlow();
  const uid = useId();
  const [query, setQuery] = useState("");
  const [open, setOpen] = useState<Record<string, boolean>>(loadCatalogOpen);
  const searching = query.trim().length > 0;

  const custom = useCustomCatalog((s) => s.classes);
  const all = useMemo(() => [...MODULE_CLASSES, ...custom], [custom]);
  const tree = useMemo(() => catalogTree(all), [all]);
  const results = useMemo(() => searchCatalog(all, query), [all, query]);
  const matches = results.reduce((n, g) => n + g.items.length, 0);

  const remember = useCallback((next: Record<string, boolean>) => { setOpen(next); saveCatalogOpen(next); }, []);
  const toggle = (key: string) => remember({ ...open, [key]: !isOpen(open, tree, key) });
  const panelId = (key: string) => `${uid}-${key.replace(/[^A-Za-z0-9]+/g, "-")}`;

  const add = (cls: ModuleClass) => {
    const el = canvasBridge.el;
    const size = DENSITY_SIZE[api.getState().assembly.editor.density];
    const r = el?.getBoundingClientRect();
    const p = r ? screenToFlowPosition({ x: r.left + r.width / 2, y: r.top + r.height / 2 }) : { x: 120, y: 120 };
    api.getState().addFromCatalog(cls.id, { x: p.x - size.width / 2, y: p.y - size.height / 2 }, { avoidOverlap: true });
  };
  const edit = (k: ModuleClass, editing: boolean) => useCustomCatalog.getState().openDesigner(draftFromClass(k, editing));

  const category = (g: CategoryGroup) => {
    const catOpen = isOpen(open, tree, g.key);
    const id = panelId(g.key);
    return (
      <section key={g.key} className="mb-0.5" data-testid="catalog-section" data-category={g.cat} data-open={catOpen}>
        <Disclosure level={1} open={catOpen} controls={id} onToggle={() => toggle(g.key)} label={g.cat} count={g.count} testId="catalog-category">
          <span className="size-2 shrink-0 rounded-full" style={{ background: categoryColor(g.cat) }} aria-hidden="true" />
        </Disclosure>
        {catOpen && (
          <div id={id} role="group" aria-label={g.cat} className="pb-1">
            {g.cat === "Custom" && g.loose.length === 0 && (
              <div className="apl-catalog-grid mb-1.5 rounded-box border border-dashed border-base-300 p-2 text-[11px] leading-snug text-base-content/70" data-testid="custom-empty">
                No custom modules yet. Create one, group placed modules (Ctrl+G) or import a catalog file.
              </div>
            )}
            {g.loose.length > 0 && <Tiles items={g.loose} onAdd={add} onEdit={edit} />}
            {g.subs.map((s) => {
              const subOpen = isOpen(open, tree, s.key);
              const sid = panelId(s.key);
              return (
                <div key={s.key} data-testid="catalog-subsection" data-subcategory={s.name} data-open={subOpen}>
                  <Disclosure level={2} open={subOpen} controls={sid} onToggle={() => toggle(s.key)} label={s.name} count={s.items.length} testId="catalog-subcategory" />
                  {subOpen && <div id={sid} role="group" aria-label={`${g.cat}: ${s.name}`}><Tiles items={s.items} onAdd={add} onEdit={edit} /></div>}
                </div>
              );
            })}
            {g.cat === "Custom" && <CustomActions count={custom.length} />}
          </div>
        )}
      </section>
    );
  };

  return (
    <aside className="flex w-44 shrink-0 flex-col border-r border-base-300 bg-base-100 lg:w-48" aria-label="Module catalog" data-testid="catalog">
      <div className="p-2">
        <label className="input input-sm flex w-full items-center gap-1.5">
          <IconSearch size={14} />
          <input type="search" value={query} onChange={(e) => setQuery(e.target.value)} placeholder="Search modules" aria-label="Search modules" data-testid="catalog-search" className="grow" />
        </label>
        <div className="mt-1 flex flex-wrap items-center justify-between gap-x-0.5 text-[10px] text-base-content/70">
          <span className="whitespace-nowrap" data-testid="catalog-count" aria-live="polite">{searching ? `${matches} of ${all.length}` : `${all.length} modules`}</span>
          <span className="ml-auto whitespace-nowrap">
            <button type="button" className="link inline-flex min-h-6 items-center disabled:cursor-default disabled:no-underline disabled:opacity-40" data-testid="catalog-collapse-all" disabled={searching} onClick={() => remember(setAll(tree, false))}>collapse all</button>
            <span className="inline-block w-1.5" aria-hidden="true" />
            <button type="button" className="link inline-flex min-h-6 items-center disabled:cursor-default disabled:no-underline disabled:opacity-40" data-testid="catalog-expand-all" disabled={searching} onClick={() => remember(setAll(tree, true))}>expand all</button>
          </span>
        </div>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto px-2 pb-3" data-testid="catalog-list">
        {searching && results.length === 0 && (
          <div className="px-1 py-4 text-xs text-base-content/70" data-testid="catalog-no-match">
            <p>No modules match “{query.trim()}”.</p>
            <button type="button" className="link mt-1" onClick={() => setQuery("")}>Clear search</button>
          </div>
        )}
        {searching && results.map((r) => (
          <section key={r.path} className="mb-1" data-testid="catalog-result-group" data-path={r.path} data-category={r.cat} aria-label={r.path}>
            <h2 className="m-0 flex items-baseline gap-1.5 px-1 py-1 text-[11px] font-medium leading-tight text-base-content/75">
              <span className="size-1.5 shrink-0 translate-y-[-1px] rounded-full" style={{ background: categoryColor(r.cat) }} aria-hidden="true" />
              <span className="min-w-0 break-words">{r.path}</span>
            </h2>
            <Tiles items={r.items} onAdd={add} onEdit={edit} />
          </section>
        ))}
        {!searching && tree.map(category)}
      </div>
    </aside>
  );
}

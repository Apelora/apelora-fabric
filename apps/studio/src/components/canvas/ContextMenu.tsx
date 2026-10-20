import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useReactFlow } from "@xyflow/react";
import { useStudio, useStudioApi } from "../../features/editor/context";
import { buildMenu, type MenuItem } from "../../features/editor/menus";
import { IconCheck, IconChevron } from "../icons";

function Panel({ items, level, title, pos, onRun, onClose, autoFocus }: {
  items: MenuItem[]; level: number; title?: string; pos: { x: number; y: number }; onRun: () => void; onClose: (all: boolean) => void; autoFocus: boolean;
}) {
  const ref = useRef<HTMLUListElement>(null);
  const [sub, setSub] = useState<string | null>(null);
  const [placed, setPlaced] = useState(pos);

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    const pad = 8;
    setPlaced({ x: Math.max(pad, Math.min(pos.x, window.innerWidth - r.width - pad)), y: Math.max(pad, Math.min(pos.y, window.innerHeight - r.height - pad)) });
  }, [pos.x, pos.y, items.length]);

  useEffect(() => { if (autoFocus) ref.current?.querySelector<HTMLButtonElement>("[role^=menuitem]:not(:disabled)")?.focus(); }, [autoFocus]);

  const buttons = () => [...(ref.current?.querySelectorAll<HTMLButtonElement>(":scope > li > [role^=menuitem]:not(:disabled)") ?? [])];
  const onKeyDown = (e: React.KeyboardEvent) => {
    const list = buttons();
    const i = list.indexOf(document.activeElement as HTMLButtonElement);
    if (e.key === "ArrowDown") { e.preventDefault(); list[(i + 1) % list.length]?.focus(); }
    else if (e.key === "ArrowUp") { e.preventDefault(); list[(i - 1 + list.length) % list.length]?.focus(); }
    else if (e.key === "Home") { e.preventDefault(); list[0]?.focus(); }
    else if (e.key === "End") { e.preventDefault(); list[list.length - 1]?.focus(); }
    else if (e.key === "ArrowLeft" && level > 0) { e.preventDefault(); e.stopPropagation(); onClose(false); }
    else if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); onClose(level === 0); }
  };

  return (
    <ul
      ref={ref}
      role="menu"
      aria-label={title}
      data-testid={level === 0 ? "context-menu" : "context-submenu"}
      onKeyDown={onKeyDown}
      className="menu fixed z-[60] max-h-[70vh] w-56 flex-nowrap overflow-y-auto rounded-box border border-base-300 bg-base-100 p-1 text-xs shadow-lg"
      style={{ left: placed.x, top: placed.y }}
      onContextMenu={(e) => e.preventDefault()}
    >
      {title && level === 0 && <li className="menu-title truncate px-2 py-1 text-[11px]" role="presentation">{title}</li>}
      {items.map((it, i) => {
        if (it.type === "separator") return <li key={`s${i}`} role="separator" className="my-0.5 h-px bg-base-300 p-0" />;
        if (it.type === "header") return <li key={`h${i}`} role="presentation" className="menu-title px-2 pb-0.5 pt-1.5 text-[10px] uppercase tracking-wide">{it.label}</li>;
        const hasSub = !!it.children;
        const open = sub === it.id;
        return (
          <li key={it.id} role="none" className="relative" onMouseEnter={() => setSub(hasSub ? it.id : null)}>
            <button
              type="button"
              role={it.checked !== undefined ? "menuitemcheckbox" : "menuitem"}
              aria-checked={it.checked}
              aria-haspopup={hasSub ? "menu" : undefined}
              aria-expanded={hasSub ? open : undefined}
              disabled={it.disabled}
              data-testid={`menu-${it.id}`}
              tabIndex={-1}
              className={`flex w-full items-center gap-2 py-1 ${it.danger ? "text-error" : ""}`}
              // A submenu opens on hover and on click; a click never closes it (the pointer has
              // usually opened it on the way, so toggling would close it again).
              onClick={() => { if (hasSub) { setSub(it.id); return; } onRun(); it.run?.(); }}
              onKeyDown={(e) => { if (hasSub && (e.key === "ArrowRight" || e.key === "Enter" || e.key === " ")) { e.preventDefault(); e.stopPropagation(); setSub(it.id); } }}
            >
              <span className="inline-flex w-3 justify-center">{it.checked ? <IconCheck size={12} /> : null}</span>
              <span className="flex-1 truncate text-left">{it.label}</span>
              {it.shortcut && <kbd className="kbd kbd-xs">{it.shortcut}</kbd>}
              {hasSub && <IconChevron size={12} />}
            </button>
            {hasSub && open && it.children && (
              <Panel
                items={it.children}
                level={level + 1}
                pos={{ x: placed.x + (ref.current?.getBoundingClientRect().width ?? 224) - 2, y: (ref.current?.querySelector(`[data-testid="menu-${it.id}"]`)?.getBoundingClientRect().top ?? placed.y) - 4 }}
                onRun={onRun}
                onClose={(all) => { if (all) onClose(true); else setSub(null); }}
                autoFocus
              />
            )}
          </li>
        );
      })}
    </ul>
  );
}

export function ContextMenu() {
  const api = useStudioApi();
  const menu = useStudio((s) => s.menu);
  const rf = useReactFlow();
  const model = useMemo(() => (menu ? buildMenu(api, menu.target, { fitView: (o) => void rf.fitView(o) }) : null), [api, menu, rf]);
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!menu) return;
    document.body.dataset.menu = "open";
    const close = () => api.getState().closeMenu();
    const onDown = (e: PointerEvent) => { if (!rootRef.current?.contains(e.target as Node)) close(); };
    window.addEventListener("pointerdown", onDown, true);
    window.addEventListener("resize", close);
    window.addEventListener("blur", close);
    window.addEventListener("wheel", close, { passive: true });
    return () => {
      delete document.body.dataset.menu;
      window.removeEventListener("pointerdown", onDown, true);
      window.removeEventListener("resize", close);
      window.removeEventListener("blur", close);
      window.removeEventListener("wheel", close);
    };
  }, [menu, api]);

  if (!menu || !model || model.items.length === 0) return null;
  return (
    <div ref={rootRef}>
      <Panel items={model.items} title={model.title} level={0} pos={{ x: menu.x, y: menu.y }} autoFocus onRun={() => api.getState().closeMenu()} onClose={() => api.getState().closeMenu()} />
    </div>
  );
}

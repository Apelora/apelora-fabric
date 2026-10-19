import { allClasses, lookupClass } from "@apelora/appliance-catalog";
import { CATEGORIES, DENSITY_SIZE, PORT_TYPES, SHAPE_PALETTE, cableColorOf, STROKE_STYLES, shapeContents, tagNames, tagNet, tagOf, type AlignMode, type ShapeKind } from "@apelora/assembly-model";
import { draftFromClass, draftFromPorts, useCustomCatalog } from "../catalog/customCatalog";
import type { MenuTarget, StudioStore } from "./store";
import { ms } from "./motion";

export type MenuItem =
  | { type: "header"; label: string }
  | { type: "separator" }
  | { type: "item"; id: string; label: string; shortcut?: string; checked?: boolean; disabled?: boolean; danger?: boolean; run?: () => void; children?: MenuItem[] };

export interface MenuModel { title: string; items: MenuItem[] }

export interface RfActions {
  fitView: (o?: { nodes?: { id: string }[]; padding?: number; maxZoom?: number; duration?: number }) => void;
}

const item = (id: string, label: string, run: () => void, o: Partial<Extract<MenuItem, { type: "item" }>> = {}): MenuItem => ({ type: "item", id, label, run, ...o });
const sep: MenuItem = { type: "separator" };

/** Focuses a field in the inspector once React has rendered the selection. */
function focusField(selector: string) {
  requestAnimationFrame(() => requestAnimationFrame(() => {
    const el = document.querySelector<HTMLInputElement | HTMLSelectElement>(selector);
    el?.focus();
    if (el && "select" in el && typeof el.select === "function") el.select();
  }));
}
/** Opens the properties window on a tab and focuses a field once React has rendered it. */
export function openPropsAndFocus(api: StudioStore, tab: "general" | "config" | "ports", selector: string, id?: string) {
  const st = api.getState();
  const target = id ?? st.selection.nodes[0];
  if (!target) return;
  st.openProps({ kind: "appliance", id: target, tab });
  focusField(selector);
}
export const renameSelected = (api: StudioStore) => openPropsAndFocus(api, "general", "[data-testid=instance-name]");

const SHAPE_KINDS_MENU: { kind: ShapeKind; label: string }[] = [{ kind: "rect", label: "Square" }, { kind: "ellipse", label: "Circle" }, { kind: "rounded", label: "Rounded square" }, { kind: "triangle", label: "Triangle" }];

export function buildMenu(api: StudioStore, target: MenuTarget, rf: RfActions): MenuModel {
  const s = () => api.getState();
  const a = s().assembly;
  const sel = s().selection;

  if (target.kind === "node") {
    const multi = sel.nodes.length > 1 && sel.nodes.includes(target.id);
    if (multi) {
      const ids = sel.nodes;
      const align = (m: AlignMode) => () => s().align(m);
      return {
        title: `${ids.length} appliances selected`,
        items: [
          item("duplicate", "Duplicate", () => s().duplicateSelection(), { shortcut: "Ctrl+D" }),
          item("copy", "Copy", () => s().copySelection(), { shortcut: "Ctrl+C" }),
          item("align", "Align", () => undefined, {
            children: [
              item("align-left", "Left edges", align("left")), item("align-right", "Right edges", align("right")),
              item("align-top", "Top edges", align("top")), item("align-bottom", "Bottom edges", align("bottom")),
              item("align-cx", "Centre horizontally", align("center-x")), item("align-cy", "Centre vertically", align("center-y")),
            ],
          }),
          item("wrap", "Wrap in zone", () => undefined, { children: SHAPE_KINDS_MENU.map((k) => item(`wrap-${k.kind}`, k.label, () => s().wrapInZone(k.kind))) }),
          item("group", "Group into component…", () => s().openGroupDialog(), { shortcut: "Ctrl+G" }),
          item("zoom", "Zoom to selection", () => rf.fitView({ nodes: ids.map((id) => ({ id })), padding: 0.5, maxZoom: 1.4, duration: ms(250) })),
          sep,
          item("delete", `Delete ${ids.length} appliances`, () => s().deleteElements(ids, []), { shortcut: "Del", danger: true }),
        ],
      };
    }
    const app = a.spec.appliances.find((x) => x.id === target.id);
    if (!app) return { title: "", items: [] };
    const cls = lookupClass(app.classRef.name, app.classRef.version);
    const layout = a.editor.nodes.find((n) => n.id === app.id);
    const cables = a.spec.connections.filter((c) => c.source.applianceId === app.id || c.target.applianceId === app.id).length;
    const hasProps = (cls?.properties?.length ?? 0) > 0;
    const overrides = Object.keys(app.config ?? {}).length;
    return {
      title: `${app.name} · ${cls?.displayName ?? app.classRef.name}`,
      items: [
        item("properties", "Properties…", () => s().openProps({ kind: "appliance", id: app.id }), { shortcut: "Enter" }),
        item("rename", "Rename…", () => renameSelected(api), { shortcut: "F2" }),
        item("configure", "Configure…", () => openPropsAndFocus(api, "config", "[data-testid=config-form] input, [data-testid=config-form] select", app.id), { disabled: !hasProps }),
        item("reset-config", overrides ? `Reset configuration (${overrides} changed)` : "Reset configuration", () => s().resetCfg(app.id), { disabled: overrides === 0 }),
        item("duplicate", "Duplicate", () => s().duplicateSelection(), { shortcut: "Ctrl+D" }),
        item("copy", "Copy", () => s().copySelection(), { shortcut: "Ctrl+C" }),
        sep,
        item("add-port", "Add port", () => undefined, {
          children: [item("add-input", "Input port (left)", () => s().addPortTo(app.id, "input")), item("add-output", "Output port (right)", () => s().addPortTo(app.id, "output"))],
        }),
        item("reset-size", "Reset size", () => s().resetSize(app.id), { disabled: !layout?.custom }),
        item("disconnect", cables ? `Unplug all cables (${cables})` : "Unplug all cables", () => s().disconnectAllCables(app.id), { disabled: cables === 0 }),
        item("zoom", "Zoom to appliance", () => rf.fitView({ nodes: [{ id: app.id }], padding: 0.8, maxZoom: 1.5, duration: ms(250) })),
        sep,
        ...(cls?.composition ? [item("ungroup", "Ungroup component", () => s().ungroupSelection(app.id), { shortcut: "Ctrl+Shift+G" })] : []),
        item("save-class", "Save as custom appliance…", () => useCustomCatalog.getState().openDesigner(draftFromPorts(app.name, app.ports, app.resources, cls?.iconAssetPath ?? "generic-box", cls?.properties, app.config)), { disabled: !!cls?.composition }),
        ...(cls?.custom && !cls.composition ? [item("edit-class", "Edit appliance class…", () => useCustomCatalog.getState().openDesigner(draftFromClass(cls, true)))] : []),
        sep,
        item("delete", "Delete", () => s().deleteElements([app.id], []), { shortcut: "Del", danger: true }),
      ],
    };
  }

  if (target.kind === "port") {
    const app = a.spec.appliances.find((x) => x.id === target.nodeId);
    const port = app?.ports.find((p) => p.id === target.portId);
    if (!app || !port) return { title: "", items: [] };
    const side = app.ports.filter((p) => p.direction === port.direction);
    const idx = side.findIndex((p) => p.id === port.id);
    const cables = a.spec.connections.filter((c) => (c.source.applianceId === app.id && c.source.portId === port.id) || (c.target.applianceId === app.id && c.target.portId === port.id)).length;
    return {
      title: `${app.name} · ${port.name} (${port.type})`,
      items: [
        item("rename-port", "Rename port…", () => openPropsAndFocus(api, "ports", `[data-testid=port-row][data-port-id="${port.id}"] input`, app.id)),
        item("type", "Protocol type", () => undefined, { children: PORT_TYPES.map((t) => item(`type-${t}`, t, () => s().editPort(app.id, port.id, { type: t }), { checked: port.type === t })) }),
        item("link-tag", tagOf(a, { applianceId: app.id, portId: port.id }) ? `Link by name (${tagOf(a, { applianceId: app.id, portId: port.id })!.tag})` : "Link by name", () => undefined, {
          children: [
            ...tagNames(a).map((n) => item(`tag-${n}`, n, () => void s().tagPortTo({ applianceId: app.id, portId: port.id }, n), { checked: tagOf(a, { applianceId: app.id, portId: port.id })?.tag === n })),
            item("tag-new", "New tag…", () => openPropsAndFocus(api, "ports", `[data-testid=port-row][data-port-id="${port.id}"] [data-testid=port-tag]`, app.id)),
          ],
        }),
        ...(tagOf(a, { applianceId: app.id, portId: port.id }) ? [
          item("tag-remove", "Remove link tag", () => s().untagPortAction({ applianceId: app.id, portId: port.id })),
          item("tag-select", "Select linked appliances", () => { const t = tagOf(a, { applianceId: app.id, portId: port.id })!.tag; s().setSelection({ nodes: [...new Set(tagNet(a, t).map((l) => l.endpoint.applianceId))], edges: [] }); }),
        ] : []),
        item("multi", "Allow several cables", () => s().editPort(app.id, port.id, { multiplicity: port.multiplicity === "many" ? "one" : "many" }), { checked: port.multiplicity === "many" }),
        item("required", "Required", () => s().editPort(app.id, port.id, { optional: !port.optional }), { checked: !port.optional }),
        sep,
        item("unplug", cables ? `Unplug cables (${cables})` : "Unplug cables", () => s().disconnectPortCables(app.id, port.id), { disabled: cables === 0 }),
        item("up", "Move up", () => s().shiftPort(app.id, port.id, -1), { disabled: idx <= 0 }),
        item("down", "Move down", () => s().shiftPort(app.id, port.id, 1), { disabled: idx === side.length - 1 }),
        sep,
        item("delete-port", "Delete port", () => s().deletePort(app.id, port.id), { danger: true, disabled: port.id === "net" }),
      ],
    };
  }

  if (target.kind === "edge") {
    const c = a.spec.connections.find((x) => x.id === target.id);
    if (!c) return { title: "", items: [] };
    const sa = a.spec.appliances.find((x) => x.id === c.source.applianceId);
    const ta = a.spec.appliances.find((x) => x.id === c.target.applianceId);
    const sp = sa?.ports.find((p) => p.id === c.source.portId);
    const tp = ta?.ports.find((p) => p.id === c.target.portId);
    return {
      title: `${sa?.name}.${sp?.name} → ${ta?.name}.${tp?.name}`,
      items: [
        { type: "header", label: `${sp?.type ?? ""} cable` },
        item("properties", "Properties…", () => s().openProps({ kind: "cable", id: c.id })),
        item("cable-colour", "Cable colour", () => undefined, {
          children: [
            item("cable-colour-protocol", "Protocol colour", () => s().setCableColorAction(c.id, null), { checked: !cableColorOf(a, c.id) }),
            ...SHAPE_PALETTE.map((p) => item(`cable-colour-${p.name.toLowerCase()}`, p.name, () => s().setCableColorAction(c.id, p.color), { checked: cableColorOf(a, c.id) === p.color })),
          ],
        }),
        item("to-tag", "Convert to link tag", () => s().cableToTagAction(c.id)),
        item("select-source", `Select ${sa?.name ?? "source"}`, () => s().selectNode(c.source.applianceId)),
        item("select-target", `Select ${ta?.name ?? "target"}`, () => s().selectNode(c.target.applianceId)),
        sep,
        item("delete", "Unplug cable", () => s().deleteElements([], [c.id]), { shortcut: "Del", danger: true }),
      ],
    };
  }

  if (target.kind === "shape") {
    const sh = (a.editor.shapes ?? []).find((x) => x.id === target.id);
    if (!sh) return { title: "", items: [] };
    const inside = shapeContents(a, sh.id).length;
    const quick = (key: "fill" | "stroke") => SHAPE_PALETTE.map((p) => item(`${key}-${p.name.toLowerCase()}`, p.name, () => s().editShape(sh.id, key === "fill" ? { fill: p.color, stroke: p.color } : { stroke: p.color }), { checked: sh[key].toLowerCase() === p.color }));
    return {
      title: `${sh.label || "Zone"} · ${sh.kind}`,
      items: [
        item("properties", "Edit shape…", () => s().openProps({ kind: "shape", id: sh.id }), { shortcut: "Enter" }),
        item("rename", "Edit label…", () => { s().openProps({ kind: "shape", id: sh.id, tab: "label" }); focusField("[data-testid=shape-label-input]"); }),
        item("shape-kind", "Shape", () => undefined, { children: SHAPE_KINDS_MENU.map((k) => item(`kind-${k.kind}`, k.label, () => s().editShape(sh.id, { kind: k.kind, ...(k.kind === "rounded" && sh.radius === 0 ? { radius: 18 } : {}) }), { checked: sh.kind === k.kind })) }),
        item("fill", "Colour (fill and border)", () => undefined, { children: quick("fill") }),
        item("border-colour", "Border colour", () => undefined, { children: quick("stroke") }),
        item("border-style", "Border style", () => undefined, { children: STROKE_STYLES.map((st) => item(`style-${st}`, st[0]!.toUpperCase() + st.slice(1), () => s().editShape(sh.id, { strokeStyle: st }), { checked: sh.strokeStyle === st })) }),
        sep,
        item("select-contents", inside ? `Select appliances inside (${inside})` : "Select appliances inside", () => s().setSelection({ nodes: shapeContents(a, sh.id), edges: [], shapes: [] }), { disabled: inside === 0 }),
        item("front", "Bring forward", () => s().orderShapeAction(sh.id, "front")),
        item("back", "Send back", () => s().orderShapeAction(sh.id, "back")),
        item("duplicate", "Duplicate", () => s().duplicateShapeAction(sh.id)),
        item("lock", sh.locked ? "Unlock" : "Lock position and size", () => s().editShape(sh.id, { locked: !sh.locked }), { checked: sh.locked }),
        sep,
        item("delete", "Delete", () => s().deleteShapes([sh.id]), { shortcut: "Del", danger: true }),
      ],
    };
  }

  // pane
  const flow = target.flow;
  const size = DENSITY_SIZE[a.editor.density];
  const classes = allClasses();
  const add: MenuItem[] = CATEGORIES.flatMap((cat) => {
    const inCat = classes.filter((c) => c.category === cat);
    return inCat.length ? [{ type: "header" as const, label: cat }, ...inCat.map((c) => item(`add-${c.id}`, c.displayName, () => void s().addFromCatalog(c.id, { x: flow.x - size.width / 2, y: flow.y - size.height / 2 })))] : [];
  });
  return {
    title: "Canvas",
    items: [
      item("add", "Add appliance", () => undefined, { children: add }),
      item("add-shape", "Add zone", () => undefined, { children: SHAPE_KINDS_MENU.map((k) => item(`shape-${k.kind}`, k.label, () => void s().addShapeAt(k.kind, flow))) }),
      item("paste", "Paste", () => s().pasteAt(flow), { shortcut: "Ctrl+V", disabled: !s().clipboard }),
      item("select-all", "Select all", () => s().selectAll(), { shortcut: "Ctrl+A", disabled: a.spec.appliances.length === 0 }),
      sep,
      item("undo", "Undo", () => s().undo(), { shortcut: "Ctrl+Z", disabled: s().past.length === 0 }),
      item("redo", "Redo", () => s().redo(), { shortcut: "Ctrl+Shift+Z", disabled: s().future.length === 0 }),
      sep,
      item("assembly-props", "Assembly properties…", () => s().openProps({ kind: "assembly" })),
      item("fit", "Fit to view", () => rf.fitView({ padding: 0.2, maxZoom: 1.2, duration: ms(250) }), { shortcut: "F" }),
      item("grid", "Dotted grid", () => s().setShowGrid(!s().showGrid), { checked: s().showGrid }),
      item("labels", "Port and link labels", () => s().setShowLabels(!s().showLabels), { checked: s().showLabels }),
      item("routing", "Right-angle cables", () => s().setRouting(s().routing === "angled" ? "curved" : "angled"), { checked: s().routing === "angled" }),
      item("category", "Category colours", () => s().setShowCategory(!s().showCategory), { checked: s().showCategory }),
      item("cable", "Thick patch-cable look", () => s().setCableStyle(s().cableStyle === "line" ? "cable" : "line"), { checked: s().cableStyle === "cable" }),
    ],
  };
}

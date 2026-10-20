import type { InstanceAccess } from "@apelora/api-client";

import { KIND_ACTION, instancesOf, kindOf, useSessions, type Kind } from "./sessions";

/** Menu entries (as the canvas menu models them) for sessions into one module's instances. */
export interface SessionMenuItem {
  type: "item";
  id: string;
  label: string;
  hint?: string;
  disabled?: boolean;
  run?: () => void;
  children?: SessionMenuItem[];
}

/**
 * Right-click entries for a module of the application on screen. Empty unless the control
 * plane knows deployed instances of it, so the design canvas of an undeployed draft never offers
 * a session. Each entry says why it is unavailable instead of silently disappearing.
 */
export function sessionMenuItems(moduleId: string): SessionMenuItem[] {
  const { access, app, open } = useSessions.getState();
  const instances = instancesOf(access, moduleId);
  if (!app || instances.length === 0) return [];
  const kinds = [...new Set(instances.flatMap((i) => i.kinds.map((k) => k.kind)))] as Kind[];
  return kinds.map((kind) => {
    const entry = (i: InstanceAccess): SessionMenuItem => {
      const k = kindOf(i, kind)!;
      const suffix = k.allowed && k.approvalRequired ? " · needs approval" : "";
      return {
        type: "item",
        id: `session-${kind}-${i.instance}`,
        label: instances.length > 1 ? `${i.instance}${suffix}` : `${KIND_ACTION[kind]}… (${i.instance})${suffix}`,
        hint: k.allowed ? undefined : (k.reason ?? "Not available"),
        disabled: !k.allowed,
        run: () => open({ app, instance: i.instance, module: moduleId, kind, approvalRequired: k.approvalRequired }),
      };
    };
    if (instances.length === 1) return entry(instances[0]!);
    return { type: "item", id: `session-${kind}`, label: `${KIND_ACTION[kind]}…`, children: instances.map(entry) } as SessionMenuItem;
  });
}

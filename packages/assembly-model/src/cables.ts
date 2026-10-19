import type { ApplicationAssembly, CableStyle } from "./schema";

const HEX = /^#[0-9a-fA-F]{6}$/;

const stylesOf = (a: ApplicationAssembly): CableStyle[] => a.editor.cables ?? [];

function withStyles(a: ApplicationAssembly, cables: CableStyle[]): ApplicationAssembly {
  const { cables: _old, ...editor } = a.editor;
  return { ...a, editor: cables.length ? { ...editor, cables } : editor };
}

/** The user-chosen colour of a cable, or undefined when it uses its protocol colour. */
export function cableColorOf(a: ApplicationAssembly, connectionId: string): string | undefined {
  return stylesOf(a).find((c) => c.id === connectionId)?.color;
}

/** Sets (or with `null` clears) a cable's colour. Layout only: the application model is untouched. */
export function setCableColor(a: ApplicationAssembly, connectionId: string, color: string | null): ApplicationAssembly {
  if (!a.spec.connections.some((c) => c.id === connectionId)) return a;
  if (color !== null && !HEX.test(color)) return a;
  const rest = stylesOf(a).filter((c) => c.id !== connectionId);
  if (color === null) return rest.length === stylesOf(a).length ? a : withStyles(a, rest);
  if (cableColorOf(a, connectionId) === color) return a;
  return withStyles(a, [...rest, { id: connectionId, color }]);
}

/** Drops colours of cables that no longer exist. Returns the same object when nothing changes. */
export function pruneCableStyles(a: ApplicationAssembly): ApplicationAssembly {
  const styles = stylesOf(a);
  if (!styles.length) return a;
  const ids = new Set(a.spec.connections.map((c) => c.id));
  const kept = styles.filter((c) => ids.has(c.id));
  return kept.length === styles.length ? a : withStyles(a, kept);
}

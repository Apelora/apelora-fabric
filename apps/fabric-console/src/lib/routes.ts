/** Detail pages under a view: `#/instances/{project}/{name}` and `#/operations/{jobId}`. */

export type InstanceRoute = { project: string; name: string };

const INSTANCE = /^#\/instances\/([^/]+)\/([^/]+)$/;
const JOB = /^#\/operations\/([0-9a-fA-F-]{36})$/;

const decode = (s: string | undefined): string | null => { if (s === undefined) return null; try { return decodeURIComponent(s); } catch { return null; } };

export function instanceRoute(hash: string): InstanceRoute | null {
  const m = INSTANCE.exec(hash);
  if (!m) return null;
  const project = decode(m[1]);
  const name = decode(m[2]);
  return project && name ? { project, name } : null;
}

export const jobRoute = (hash: string): string | null => JOB.exec(hash)?.[1]?.toLowerCase() ?? null;

export const instanceHref = (project: string, name: string) => `#/instances/${encodeURIComponent(project)}/${encodeURIComponent(name)}`;
export const jobHref = (id: string) => `#/operations/${id}`;

/** The view a detail page belongs to, for the navigation; `null` when the hash is not a detail page. */
export function detailView(hash: string): "instances" | "operations" | null {
  if (instanceRoute(hash)) return "instances";
  if (jobRoute(hash)) return "operations";
  return null;
}

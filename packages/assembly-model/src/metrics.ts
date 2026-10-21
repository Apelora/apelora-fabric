import { effectiveValue } from "./config";
import type { Metrics, ModuleClass, ModuleInstance } from "./schema";

/**
 * Whether a collector can read the class's metrics over the network as metrics text: built in
 * or added by an exporter, on a port. Such a class offers a `mon` output; one without (only
 * counters inside the instance, its own protocol, JSON, or nothing) does not.
 */
export function readableMetrics(m: Metrics | undefined): m is Metrics & { port: number } {
  return !!m && (m.source === "native" || m.source === "exporter") && m.format === "text" && m.port !== undefined;
}

/** The port and transport a module's metrics are read on, given its configuration (`portBy`). */
export function metricsEndpoint(cls: ModuleClass, config: ModuleInstance["config"]): { transport: "tcp" | "udp"; port: number } | undefined {
  const m = cls.metrics;
  if (!m || m.port === undefined) return undefined;
  let port = m.port;
  if (m.portBy) {
    const prop = cls.properties?.find((p) => p.id === m.portBy?.property);
    const value = prop ? effectiveValue(prop, config) : undefined;
    port = (value === undefined ? undefined : m.portBy.ports[String(value)]) ?? port;
  }
  return { transport: m.transport ?? "tcp", port };
}

/** Problems with a class's metrics description (empty when it is sound or absent). */
export function metricsProblems(cls: ModuleClass): string[] {
  const m = cls.metrics;
  if (!m) return [];
  const out: string[] = [];
  if (m.portBy && m.port === undefined) out.push(`${cls.id}: metrics.portBy needs a default port`);
  if (m.portChosen && m.port === undefined) out.push(`${cls.id}: metrics.portChosen without a port`);
  if ((m.source === "platform" || m.source === "none") && m.port !== undefined) out.push(`${cls.id}: a ${m.source} source has no port of its own`);
  if (m.format === "counters" && m.port !== undefined) out.push(`${cls.id}: counters are read inside the instance, not on a port`);
  if (m.portBy) {
    const prop = cls.properties?.find((p) => p.id === m.portBy?.property);
    if (!prop || prop.type !== "enum" || !prop.options?.length) out.push(`${cls.id}: metrics.portBy.property ${m.portBy.property} must name an enum property`);
    else out.push(...prop.options.filter((o) => !(o in m.portBy!.ports)).map((o) => `${cls.id}: no metrics port for ${prop.id} = ${o}`));
  }
  return out;
}

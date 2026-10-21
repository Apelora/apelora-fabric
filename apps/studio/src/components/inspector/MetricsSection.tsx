import { metricsEndpoint, type Metrics, type ModuleClass, type ModuleInstance } from "@apelora/assembly-model";

const SOURCE: Record<Metrics["source"], string> = {
  native: "Built into the software",
  exporter: "Separate exporter (add-on)",
  agent: "Optional agent in the guest",
  platform: "Incus (no process in the module)",
  none: "None: the application must serve them",
};
const FORMAT: Record<Metrics["format"], string> = {
  text: "metrics text",
  json: "JSON",
  page: "status page",
  protocol: "its own protocol",
  counters: "counters inside the instance",
  none: "",
};
const AUTH: Record<Metrics["auth"], string> = {
  none: "None",
  basic: "User name and password",
  token: "Token",
  certificate: "Client certificate",
  configurable: "Configurable",
};

/**
 * Where a module's metrics come from, read-only, from its class's researched description: the
 * port a drawn `mon` connection opens, whether they are on by default, how to turn them on, and
 * what to watch. Unverified descriptions are marked.
 */
export function MetricsSection({ cls, app }: { cls: ModuleClass; app: ModuleInstance }) {
  const m = cls.metrics;
  if (!m) return null;
  const ep = metricsEndpoint(cls, app.config);
  const where = [
    ep ? `${ep.transport === "udp" ? "UDP " : ""}port ${ep.port}` : "",
    m.path ?? "",
    FORMAT[m.format],
  ].filter(Boolean).join(" · ");
  const confidence = m.confidence === "unverified"
    ? <span className="badge badge-warning badge-xs" data-testid="metrics-confidence" data-confidence="unverified" title="Not confirmed in the project's documentation: try it before relying on it">Unverified</span>
    : <span className="text-[10px] text-base-content/60" data-testid="metrics-confidence" data-confidence={m.confidence}>{m.confidence === "checked" ? "Checked" : "Partly checked"}</span>;
  return (
    <section data-testid="metrics-section" aria-label="Metrics">
      <div className="mb-1.5 flex items-center justify-between">
        <h4 className="text-[11px] font-semibold uppercase tracking-wide text-base-content/60">Metrics</h4>
        {confidence}
      </div>
      <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-xs">
        <dt className="text-base-content/60">Source</dt><dd data-testid="metrics-source" data-source={m.source}>{SOURCE[m.source]}</dd>
        {where && <><dt className="text-base-content/60">Endpoint</dt><dd className="break-all" data-testid="metrics-endpoint"><span className="font-mono">{where}</span>{m.portChosen && <span className="text-base-content/60"> (port set by the class)</span>}</dd></>}
        {m.source !== "none" && <><dt className="text-base-content/60">By default</dt><dd data-testid="metrics-default">{m.onByDefault ? (m.localOnly ? "On, inside the instance only" : "On") : "Off"}</dd></>}
        {m.source !== "none" && <><dt className="text-base-content/60">Access</dt><dd data-testid="metrics-auth">{AUTH[m.auth]}</dd></>}
        {m.health && <><dt className="text-base-content/60">Health</dt><dd className="break-all font-mono" data-testid="metrics-health">{m.health.port ? `port ${m.health.port} · ` : ""}{m.health.path}</dd></>}
      </dl>
      {m.enable && <p className="mt-1.5 text-xs text-base-content/70" data-testid="metrics-enable">{m.enable}</p>}
      {m.signals.length > 0 && (
        <div className="mt-1.5 text-xs">
          <span className="text-base-content/60">Worth watching</span>
          <ul className="mt-0.5 list-disc pl-4" data-testid="metrics-signals">{m.signals.map((s) => <li key={s}>{s}</li>)}</ul>
        </div>
      )}
      {m.note && <p className="mt-1.5 text-[11px] text-base-content/60" data-testid="metrics-note">{m.note}</p>}
    </section>
  );
}

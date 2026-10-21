import type { MetricsEndpoint, MonitoringInfo } from "@apelora/api-client";

import { Badge, Section, SectionHeader, TableFrame } from "../components/ui";
import { client, useLoad } from "../lib/api";

const COMPONENTS: Record<string, string> = {
  "control-plane": "Control plane",
  "control-plane-listener": "Control plane (separate listener)",
  worker: "Deployment worker",
};

const where = (e: MetricsEndpoint) => (e.address ? `${e.address}${e.path}` : `${e.path} on this API's address`);

const ROW = "flex flex-col gap-1 px-4 py-3 sm:flex-row sm:justify-between sm:gap-6";

/**
 * How this installation's own collector reaches Apelora's metrics: the endpoints, whether the
 * signed-in caller could scrape, the region/site/sector labels on every metric, and the steps to
 * connect. Read-only; everything shown comes from `GET /api/v1/monitoring`.
 */
export function MonitoringSection({ onAuth }: { onAuth: (m: string) => void }) {
  const [info, reload] = useLoad<MonitoringInfo>(() => client.get("monitoring"), onAuth);
  return (
    <Section title="Monitoring" state={info} testId="monitoring" onRetry={reload}>
      {(m) => (
        <>
          <SectionHeader title="Monitoring" />
          {/* Three groups with one gap between them; each heading sits close to its own content. */}
          <div className="flex flex-col gap-6">
            <dl className="divide-y divide-base-300 rounded-box border border-base-300 text-sm">
              <div className={ROW} data-testid="monitoring-row">
                <dt className="font-medium">Labels on every metric</dt>
                <dd className="font-mono text-xs text-base-content/70 sm:text-right" data-testid="monitoring-labels">
                  region={m.region} · site={m.site} · sector={m.sector}
                </dd>
              </div>
              <div className={ROW} data-testid="monitoring-row">
                <dt className="font-medium">Inventory source</dt>
                <dd className="text-base-content/70 sm:text-right">
                  {m.provider === "mock" ? <Badge tone="warn">Mock · not a real cluster</Badge> : "Incus"}
                </dd>
              </div>
              <div className={ROW} data-testid="monitoring-row">
                <dt className="font-medium">Your sign-in</dt>
                <dd className="text-base-content/70 sm:max-w-[65%] sm:text-right" data-testid="monitoring-access">
                  {m.mayScrape ? (
                    <Badge tone="ok">Can read metrics</Badge>
                  ) : (
                    <span className="flex items-start gap-2 sm:justify-end">
                      <span className="status status-neutral mt-1.5 shrink-0" aria-hidden="true" />
                      <span>Cannot read metrics. A collector needs <code className="text-xs">{m.permission}</code>.</span>
                    </span>
                  )}
                </dd>
              </div>
            </dl>

            <div className="flex flex-col gap-2">
              <h3 className="text-sm font-semibold">Where to scrape</h3>
              <TableFrame>
                <thead><tr><th>Component</th><th>Address</th><th className="max-sm:hidden">Sign-in</th></tr></thead>
                <tbody>
                  {m.endpoints.map((e) => {
                    const signIn = e.authentication === "bearer" ? `Token with ${m.permission}` : "None (loopback or management network only)";
                    return (
                      <tr key={e.component} data-testid="metrics-endpoint">
                        <td>
                          <span className="font-medium">{COMPONENTS[e.component] ?? e.component}</span>
                          {/* On phones the sign-in column moves under the name, so the table never scrolls sideways. */}
                          <span className="mt-1 block text-xs text-base-content/60 sm:hidden">{signIn}</span>
                        </td>
                        <td className="font-mono text-xs [overflow-wrap:anywhere]">{where(e)}</td>
                        <td className="max-sm:hidden">{signIn}</td>
                      </tr>
                    );
                  })}
                </tbody>
              </TableFrame>
            </div>

            <div className="flex flex-col gap-2">
              <h3 className="text-sm font-semibold">Connect your collector</h3>
              <ol className="flex list-decimal flex-col gap-2 rounded-box border border-base-300 py-3 pr-4 pl-8 text-sm" data-testid="monitoring-steps">
                <li>Give the collector a service identity with the role <strong>Metrics Reader</strong> (<code className="text-xs">{m.role}</code>) on the platform. It reads metrics only.</li>
                <li>Scrape <code className="text-xs">/metrics</code> here with that identity's token, every 30 seconds or slower.</li>
                <li>Scrape the worker from its host or management network. It has no sign-in.</li>
                <li>
                  Instance CPU, memory, disk and network come from Incus: scrape <code className="text-xs">{m.incusMetricsPath}</code> on every server. Trust the collector's certificate for metrics only:{" "}
                  <code className="text-xs break-all">incus config trust add-certificate metrics.crt --type=metrics</code>
                </li>
              </ol>
              <p className="text-xs text-base-content/60">The collector runs inside this installation. Alerts and runbooks per layer: docs/architecture/MONITORING.md</p>
            </div>
          </div>
        </>
      )}
    </Section>
  );
}

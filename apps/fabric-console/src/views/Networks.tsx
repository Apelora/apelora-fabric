import { Fragment, useState } from "react";

import type { NetApplication, NetApplications, NetModule, NetRule, NetSettings, NetworkList, WhoAmI } from "@apelora/api-client";

import { Badge, Notice, Section, SectionHeader, TableFrame, sentence, type Tone } from "../components/ui";
import { client, useLoad } from "../lib/api";
import { can } from "../lib/permissions";

const INTERNET: Record<string, string> = { none: "None", web: "Web and DNS", any: "Any port" };

/** `$shop-web-01` → `web-01`; `@external` → outside; empty → anyone. */
function peer(value: string, modules: Map<string, string>): string {
  if (!value) return "Anyone";
  if (value === "@external") return "Outside the application";
  if (value.startsWith("$")) return modules.get(value.slice(1)) ?? value.slice(1);
  return value;
}

const port = (protocol: string, p: string) => (p ? `${protocol} ${p}` : `any ${protocol} port`);

const RULE_STATE: Record<string, { label: string; tone: Tone }> = {
  applied: { label: "In Incus", tone: "ok" },
  missing: { label: "Missing in Incus", tone: "error" },
  unexpected: { label: "Not in the design", tone: "warn" },
  unknown: { label: "Not checked", tone: "neutral" },
};

/** The application's state in words: in step, how many differences, or why it was not compared. */
function appState(a: NetApplication, incusError: string | null | undefined): { label: string; tone: Tone } {
  if (!a.enforced) return { label: "Not enforced", tone: "neutral" };
  if (incusError) return { label: "Not checked", tone: "neutral" };
  const n = a.drift.length + a.modules.reduce((s, m) => s + m.drift.length, 0) + a.services.reduce((s, x) => s + x.drift.length, 0);
  return n === 0 ? { label: "Matches Incus", tone: "ok" } : { label: `${n} ${n === 1 ? "difference" : "differences"}`, tone: "warn" };
}

// ---- settings in effect -------------------------------------------------------------------------

function Settings({ onAuth }: { onAuth: (m: string) => void }) {
  const [state, reload] = useLoad<NetSettings>(() => client.get("network_settings"), onAuth);
  return (
    <Section title="Settings in effect" state={state} testId="network-settings" onRetry={reload}>
      {(s) => (
        <>
          <SectionHeader title="Settings in effect" source={s.source} />
          <dl className="grid grid-cols-1 gap-x-6 gap-y-3 rounded-box border border-base-300 p-4 text-sm sm:grid-cols-[max-content_1fr]" data-testid="network-settings-list">
            <dt className="font-medium">Uplink</dt>
            <dd>{s.uplink ? <span className="font-mono">{s.uplink}</span> : "None: connections are not enforced"}</dd>
            <dt className="font-medium">Service addresses</dt>
            <dd>{s.serviceRange ? <><span className="font-mono">{s.serviceRange}</span><span className="block text-base-content/70">{s.serviceAddressesUsed} of {s.serviceAddressesTotal ?? "?"} in use</span></> : "None"}</dd>
            <dt className="font-medium">Uplink in Incus</dt>
            <dd data-testid="network-uplink">
              {s.uplinkObserved ? (
                <>
                  {s.uplinkObserved.kind}, {s.uplinkObserved.status.toLowerCase()}{s.uplinkObserved.ipv4Address ? <>, address <span className="font-mono">{s.uplinkObserved.ipv4Address}</span></> : null}
                  <span className="block text-base-content/70">Routed ranges: {([...s.uplinkObserved.ipv4Routes, ...s.uplinkObserved.ipv6Routes].join(", ")) || "none"}</span>
                  <span className="block text-base-content/70">Ranges for application networks: {s.uplinkObserved.ovnRanges.length ? <span className="font-mono">{s.uplinkObserved.ovnRanges.join(", ")}</span> : "none"}</span>
                </>
              ) : <span className="text-base-content/70">{s.uplinkNote ?? "—"}</span>}
            </dd>
          </dl>
          {s.findings.filter((f) => f.severity === "warn").map((f) => <Notice key={f.message} tone="warn" testId="network-finding">{f.message}</Notice>)}
          {s.findings.filter((f) => f.severity !== "warn").map((f) => <p key={f.message} className="text-sm text-base-content/70" data-testid="network-note">{f.message}</p>)}
          <TableFrame>
            <thead><tr><th>Incus project</th><th className="max-sm:hidden">Own networks</th><th className="max-sm:hidden">Uplinks allowed</th><th>Address ranges allowed</th></tr></thead>
            <tbody>
              {s.projects.map((p) => (
                <tr key={p.project} data-testid="network-project-row">
                  <td className="font-medium">
                    {p.project}
                    {!p.restricted && p.found && <span className="block text-xs font-normal text-base-content/70">Not restricted</span>}
                    <span className="block text-xs font-normal text-base-content/70 sm:hidden">Own networks: {!p.found ? "—" : p.ownNetworks ? "Yes" : "No"}</span>
                  </td>
                  <td className="max-sm:hidden">{!p.found ? "—" : p.ownNetworks ? "Yes" : "No"}</td>
                  <td className="font-mono text-xs max-sm:hidden">{p.uplinks.join(", ") || "—"}</td>
                  <td className="font-mono text-xs break-all">{p.subnets.join(", ") || "—"}</td>
                </tr>
              ))}
            </tbody>
          </TableFrame>
        </>
      )}
    </Section>
  );
}

// ---- per application ----------------------------------------------------------------------------

function Rules({ title, rules, modules, direction }: { title: string; rules: NetRule[]; modules: Map<string, string>; direction: "in" | "out" }) {
  return (
    <div className="flex flex-col gap-1">
      <h5 className="text-xs font-semibold text-base-content/70">{title}</h5>
      {rules.length === 0 ? <p className="text-sm text-base-content/70">{direction === "in" ? "Nothing may reach it" : "It may reach nothing"}</p> : (
        <ul className="flex flex-col gap-1">
          {rules.map((r, i) => {
            const st = RULE_STATE[r.state] ?? RULE_STATE.unknown!;
            return (
              <li key={`${r.source}-${r.destination}-${r.protocol}-${r.port}-${i}`} className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1 text-sm" data-testid="network-rule">
                <span className="min-w-0">{direction === "in" ? "From " : "To "}{peer(direction === "in" ? r.source : r.destination, modules)} on <span className="font-mono">{port(r.protocol, r.port)}</span>{r.action !== "allow" && ` (${r.action})`}</span>
                <Badge tone={st.tone}>{st.label}</Badge>
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}

function Module({ m, modules }: { m: NetModule; modules: Map<string, string> }) {
  return (
    <li className="flex flex-col gap-2 rounded-box border border-base-300 p-3" data-testid="network-module">
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <h4 className="font-medium">{m.module}</h4>
        <span className="text-xs text-base-content/70">Internet: {INTERNET[m.internet] ?? m.internet}, {m.addresses == null ? "addresses not read" : `${m.addresses.length} ${m.addresses.length === 1 ? "address" : "addresses"}`}</span>
      </div>
      <div className="grid gap-3 md:grid-cols-2">
        <Rules title="May be reached" rules={m.ingress} modules={modules} direction="in" />
        <Rules title="May reach" rules={m.egress} modules={modules} direction="out" />
      </div>
      {m.drift.length > 0 && <ul className="list-disc pl-5 text-sm text-error" data-testid="network-module-drift">{m.drift.map((d) => <li key={d}>{sentence(d)}</li>)}</ul>}
    </li>
  );
}

function Details({ a }: { a: NetApplication }) {
  const modules = new Map(a.modules.map((m) => [m.acl, m.module]));
  return (
    <div className="flex flex-col gap-4 py-1" data-testid="network-details">
      {a.drift.length > 0 && (
        <Notice tone="warn" title="Differences from the design" testId="network-drift">
          <ul className="list-disc pl-5">{a.drift.map((d) => <li key={d}>{sentence(d)}</li>)}</ul>
        </Notice>
      )}
      <div className="flex flex-col gap-2">
        <h3 className="text-sm font-semibold">Connections in the design</h3>
        {a.connections.length === 0 ? <p className="text-sm text-base-content/70">None: no module may reach another.</p> : (
          <ul className="flex flex-col gap-1 text-sm" data-testid="network-connections">
            {a.connections.map((c) => (
              <li key={`${c.from}-${c.to}-${c.transport}-${c.port ?? "any"}`}>{c.from} may reach {c.to} on <span className="font-mono">{port(c.transport, c.port == null ? "" : String(c.port))}</span></li>
            ))}
          </ul>
        )}
      </div>
      {!a.enforced ? (
        <p className="text-sm text-base-content/70">The control plane had no uplink when this was deployed, so the instances share their project's network and nothing above is enforced.</p>
      ) : (
        <div className="flex flex-col gap-2">
          <h3 className="text-sm font-semibold">Rules per module</h3>
          <ul className="flex flex-col gap-2">{a.modules.map((m) => <Module key={m.module} m={m} modules={modules} />)}</ul>
        </div>
      )}
      {a.services.length > 0 && (
        <div className="flex flex-col gap-2">
          <h3 className="text-sm font-semibold">Service and published addresses</h3>
          <TableFrame>
            <thead><tr><th>Module</th><th>Address</th><th className="max-sm:hidden">Replicas behind it</th></tr></thead>
            <tbody>
              {a.services.map((s) => (
                <tr key={s.module} data-testid="network-service">
                  <td className="align-top">{s.module}<span className="block text-xs text-base-content/70">{s.published ? "Published outside the application" : "Inside the application"}</span></td>
                  <td className="align-top">
                    <span className="font-mono">{s.address ?? "None yet"}</span>
                    {s.address && <span className="block font-mono text-xs text-base-content/70" data-testid="network-service-name">{s.module}.{a.application}.internal</span>}
                    <span className="block font-mono text-xs text-base-content/70">{s.ports.map((p) => `${p.transport} ${p.port}`).join(", ")}</span>
                    {s.drift.map((d) => <span key={d} className="block text-xs text-error">{sentence(d)}</span>)}
                  </td>
                  <td className="align-top text-sm max-sm:hidden">{s.backends == null ? "—" : s.backends.length === 0 ? "None" : s.backends.map((b) => <span key={b.name} className="block">{b.instance || b.name} <span className="font-mono text-xs text-base-content/70">{b.address}</span></span>)}</td>
                </tr>
              ))}
            </tbody>
          </TableFrame>
          {a.published.length > 0 && a.networkObserved?.routerAddress && (
            <p className="text-xs text-base-content/70">Outside traffic to published addresses must be routed to <span className="font-mono">{a.networkObserved.routerAddress}</span>, the network's address on the uplink.</p>
          )}
        </div>
      )}
    </div>
  );
}

function Applications({ onAuth }: { onAuth: (m: string) => void }) {
  const [state, reload] = useLoad<NetApplications>(() => client.get("application_networks"), onAuth);
  const [open, setOpen] = useState<string | null>(null);
  return (
    <Section title="Application networks" state={state} testId="network-applications" onRetry={reload}>
      {(l) => (
        <>
          <SectionHeader title="Application networks" count={l.items.length} source={l.source}>
            <button type="button" className="btn btn-sm btn-ghost" onClick={reload}>Refresh</button>
          </SectionHeader>
          {l.incusError && <Notice tone="warn" testId="network-incus-error">Incus could not be read, so only the design's side is shown: {l.incusError}</Notice>}
          {l.items.length === 0 ? <p className="text-sm text-base-content/70">No deployed application you may see. Deploy a design from the Studio.</p> : (
            <TableFrame>
              <thead><tr><th>Application</th><th>State</th><th className="max-sm:hidden">Network</th><th><span className="sr-only">Details</span></th></tr></thead>
              <tbody>
                {l.items.map((a) => {
                  const key = `${a.application}/${a.project}`;
                  const st = appState(a, l.incusError);
                  const expanded = open === key;
                  const id = `network-${a.application}-${a.project}`;
                  return (
                    <Fragment key={key}>
                      <tr data-testid="network-application-row">
                        <td className="align-top font-medium">{a.application}<span className="block text-xs font-normal text-base-content/70">Revision {a.revision} in {a.project}</span></td>
                        <td className="align-top"><Badge tone={st.tone} testId="network-application-state">{st.label}</Badge></td>
                        <td className="align-top text-sm max-sm:hidden">
                          {a.enforced ? <span className="font-mono">{a.network}</span> : "Project network"}
                          <span className="block text-xs text-base-content/70">{a.enforced ? (a.networkObserved ? `${a.networkObserved.kind}, ${a.networkObserved.status.toLowerCase()}` : l.incusError ? "Not read" : "Not in Incus") : "Connections not enforced"}</span>
                        </td>
                        <td className="align-top text-right">
                          <button type="button" className="btn btn-xs btn-ghost" aria-expanded={expanded} aria-controls={id} onClick={() => setOpen(expanded ? null : key)} data-testid="network-application-toggle">
                            {expanded ? "Hide details" : "Show details"}
                          </button>
                        </td>
                      </tr>
                      {expanded && <tr id={id}><td colSpan={4} className="bg-base-200/40"><Details a={a} /></td></tr>}
                    </Fragment>
                  );
                })}
              </tbody>
            </TableFrame>
          )}
        </>
      )}
    </Section>
  );
}

// ---- Incus networks -----------------------------------------------------------------------------

function IncusNetworks({ onAuth }: { onAuth: (m: string) => void }) {
  const [networks, reload] = useLoad<NetworkList>(() => client.get("networks"), onAuth);
  return (
    <Section title="Incus networks" state={networks} testId="networks" onRetry={reload}>
      {(l) => {
        const managed = l.items.filter((n) => n.managed);
        return (
          <>
            <SectionHeader title="Incus networks" count={managed.length} source={l.source} />
            {managed.length === 0 ? <p className="text-sm text-base-content/70">No managed networks.</p> : (
              <TableFrame>
                <thead><tr><th>Network</th><th>Type</th><th>Status</th><th className="max-sm:hidden">Project</th></tr></thead>
                <tbody>
                  {managed.map((n) => (
                    <tr key={`${n.project}/${n.name}`} data-testid="network-row">
                      <td className="font-medium">{n.name}<span className="block text-xs font-normal text-base-content/70 sm:hidden">{n.project}</span></td>
                      <td>{n.kind}</td>
                      <td>{n.status || "—"}</td>
                      <td className="max-sm:hidden">{n.project}</td>
                    </tr>
                  ))}
                </tbody>
              </TableFrame>
            )}
            {l.items.length > managed.length && (
              <p className="text-xs text-base-content/70">{l.items.length - managed.length} host interfaces not managed by Incus are not shown.</p>
            )}
          </>
        );
      }}
    </Section>
  );
}

/**
 * Networks (`#/networks`), read-only: the settings in effect and the Incus networks (needs
 * `cluster.view`), and per application what Apelora intends next to what Incus holds (listed
 * for the applications the caller may see).
 */
export function NetworksView({ me, onAuth }: { me: WhoAmI | null; onAuth: (m: string) => void }) {
  const infra = can(me, "cluster.view", "read-inventory");
  const apps = can(me, "application.view", "read-inventory");
  return (
    <div className="flex flex-col gap-8">
      {apps && <Applications onAuth={onAuth} />}
      {infra && <Settings onAuth={onAuth} />}
      {infra && <IncusNetworks onAuth={onAuth} />}
      {!infra && !apps && <p className="text-sm text-base-content/70">Nothing here is visible to you: networks need permission to see the cluster or an application.</p>}
    </div>
  );
}

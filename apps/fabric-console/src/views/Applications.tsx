import { Fragment, useEffect, useState } from "react";

import type { ApplianceStatus, ApplicationList, ApplicationStatus } from "@apelora/api-client";

import { ErrorPanel, Section, SectionHeader, TableFrame, sentence } from "../components/ui";
import { client, messageOf, useLoad } from "../lib/api";

type Statuses = Record<string, ApplicationStatus | { error: string }>;

/** How an application's instances are doing, in a few words and a status colour. */
export function summary(items: readonly ApplianceStatus[]): { label: string; dot: string } {
  if (!items.length) return { label: "No instances", dot: "status-neutral" };
  const running = items.filter((i) => i.status === "Running");
  const failing = running.filter((i) => i.health?.health === "unhealthy").length;
  const stopped = items.length - running.length;
  if (!failing && !stopped) return { label: "All running", dot: "status-success" };
  const parts = [
    failing ? `${failing} failing ${failing === 1 ? "its" : "their"} check` : "",
    stopped ? `${stopped} not running` : "",
  ].filter(Boolean);
  return { label: sentence(parts.join(", ")), dot: failing === items.length || stopped === items.length ? "status-error" : "status-warning" };
}

/** An instance's health, in words. */
const health = (i: ApplianceStatus): string => {
  if (i.status !== "Running") return "—";
  if (!i.health) return "Not checked";
  const since = new Date(i.health.since).toLocaleTimeString();
  if (i.health.health === "unhealthy") return `Failing since ${since}${i.health.restartedAt ? `; restarted ${new Date(i.health.restartedAt).toLocaleTimeString()}` : ""}`;
  return i.health.health === "healthy" ? "Passing" : "Not decided yet";
};

const dot = (i: ApplianceStatus) => (i.status !== "Running" ? "status-error" : i.health?.health === "unhealthy" ? "status-warning" : "status-success");

function Instances({ status }: { status: ApplicationStatus }) {
  return (
    <div className="flex flex-col gap-2 py-1">
      <TableFrame>
        <thead><tr><th>Instance</th><th className="max-sm:hidden">Status</th><th className="max-sm:hidden">Member</th><th>Port check</th></tr></thead>
        <tbody>
          {status.items.map((i) => (
            <tr key={`${i.project}/${i.instance}`} data-testid="application-instance">
              <td className="font-medium break-words">{i.instance}<span className="block text-xs font-normal text-base-content/60">{i.appliance} · {i.project}</span></td>
              <td className="max-sm:hidden"><span className={`status mr-2 ${dot(i)}`} aria-hidden="true" />{i.status}</td>
              <td className="max-sm:hidden">{i.location}</td>
              <td className="text-sm"><span className={`status mr-2 sm:hidden ${dot(i)}`} aria-hidden="true" /><span className="sm:hidden">{i.status === "Running" ? "" : `${i.status}. `}</span>{health(i)}</td>
            </tr>
          ))}
        </tbody>
      </TableFrame>
    </div>
  );
}

/**
 * Applications deployed from the Studio: how many of their instances run, whether the ones
 * with a service address pass their port check, and the addresses. Read-only; changes are made
 * by deploying from the Studio.
 */
export function ApplicationsView({ onAuth }: { onAuth: (m: string) => void }) {
  const [list, reload] = useLoad<ApplicationList>(() => client.get("applications"), onAuth);
  const [statuses, setStatuses] = useState<Statuses>({});
  const [open, setOpen] = useState<string | null>(null);
  useEffect(() => {
    if (list.kind !== "ok") return;
    let current = true;
    void Promise.all(
      list.data.items.map(async (a) => {
        try {
          return [a.name, await client.request("application_status", { params: { name: a.name } })] as const;
        } catch (e) {
          return [a.name, { error: messageOf(e) }] as const;
        }
      }),
    ).then((entries) => { if (current) setStatuses(Object.fromEntries(entries)); });
    return () => { current = false; };
  }, [list]);

  return (
    <Section title="Applications" level={1} state={list} testId="applications" onRetry={reload}>
      {(l) => (
        <>
          <SectionHeader title="Applications" level={1} count={l.items.length}>
            <button type="button" className="btn btn-sm btn-ghost" onClick={reload}>Refresh</button>
          </SectionHeader>
          {l.items.length === 0 ? <p className="text-sm text-base-content/70">Nothing deployed yet. Deploy a design from the Studio.</p> : (
            <TableFrame>
              <thead><tr><th>Application</th><th>Instances</th><th className="max-sm:hidden">Service addresses</th><th><span className="sr-only">Details</span></th></tr></thead>
              <tbody>
                {l.items.map((a) => {
                  const s = statuses[a.name];
                  const ok = s && !("error" in s) ? s : null;
                  const sum = ok ? summary(ok.items) : null;
                  const expanded = open === a.name;
                  return (
                    <Fragment key={a.name}>
                      <tr data-testid="application-row">
                        <td className="font-medium">{a.name}<span className="block text-xs font-normal text-base-content/60">{a.latestRevision ? `Revision ${a.latestRevision}` : "No revision"}</span></td>
                        <td data-testid="application-summary">
                          {!s ? <span className="text-base-content/60">Checking…</span>
                            : "error" in s ? <span className="text-error">{s.error}</span>
                            : <><span className={`status mr-2 ${sum!.dot}`} aria-hidden="true" />{sum!.label}<span className="block text-xs text-base-content/60">{ok!.items.filter((i) => i.status === "Running").length} of {ok!.items.length} running</span></>}
                        </td>
                        <td className="font-mono text-xs max-sm:hidden">{ok?.services.map((v) => <span key={`${v.project}/${v.appliance}`} className="block">{v.address} <span className="font-sans text-base-content/60">{v.appliance}</span></span>) ?? null}{ok && !ok.services.length && <span className="font-sans text-base-content/60">None</span>}</td>
                        <td className="text-right">
                          <button type="button" className="btn btn-xs btn-ghost" disabled={!ok?.items.length} aria-expanded={expanded} aria-controls={`instances-${a.name}`} onClick={() => setOpen(expanded ? null : a.name)} data-testid="application-toggle">
                            {expanded ? "Hide instances" : "Show instances"}
                          </button>
                        </td>
                      </tr>
                      {expanded && ok && (
                        <tr id={`instances-${a.name}`}><td colSpan={4} className="bg-base-200/40"><Instances status={ok} /></td></tr>
                      )}
                    </Fragment>
                  );
                })}
              </tbody>
            </TableFrame>
          )}
          {list.kind === "ok" && Object.values(statuses).some((s) => "error" in s) && <ErrorPanel message="Some applications' state could not be read." onRetry={reload} />}
        </>
      )}
    </Section>
  );
}

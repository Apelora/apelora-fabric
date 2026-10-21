import type { NetworkList, ProjectList, StoragePoolList } from "@apelora/api-client";

import { Section, SectionHeader, TableFrame } from "../components/ui";
import { client, useLoad } from "../lib/api";

/** Incus projects (Administration): which are restricted, and what they are for. */
export function ProjectsSection({ onAuth }: { onAuth: (m: string) => void }) {
  const [projects, reload] = useLoad<ProjectList>(() => client.get("projects"), onAuth);
  return (
    <Section title="Incus projects" state={projects} testId="projects" onRetry={reload}>
      {(l) => (
        <>
          <SectionHeader title="Incus projects" count={l.items.length} source={l.source} />
          <TableFrame>
            <thead><tr><th>Project</th><th>Restricted</th><th>Description</th></tr></thead>
            <tbody>
              {l.items.map((p) => (
                <tr key={p.name} data-testid="project-row">
                  <td className="font-medium">{p.name}</td>
                  <td>{p.restricted ? "Yes" : "No"}</td>
                  <td>{p.description || "—"}</td>
                </tr>
              ))}
            </tbody>
          </TableFrame>
        </>
      )}
    </Section>
  );
}

/** Networks: the managed Incus networks (read-only). */
export function NetworksView({ onAuth }: { onAuth: (m: string) => void }) {
  const [networks, reload] = useLoad<NetworkList>(() => client.get("networks"), onAuth);
  return (
    <Section title="Networks" state={networks} testId="networks" onRetry={reload} level={1}>
      {(l) => {
        const managed = l.items.filter((n) => n.managed);
        return (
          <>
            <SectionHeader title="Networks" level={1} count={managed.length} source={l.source} />
            {managed.length === 0 ? <p className="text-sm text-base-content/70">No managed networks.</p> : (
              <TableFrame>
                <thead><tr><th>Network</th><th>Type</th><th>Status</th><th>Project</th></tr></thead>
                <tbody>
                  {managed.map((n) => (
                    <tr key={`${n.project}/${n.name}`} data-testid="network-row">
                      <td className="font-medium">{n.name}</td>
                      <td>{n.kind}</td>
                      <td>{n.status || "—"}</td>
                      <td>{n.project}</td>
                    </tr>
                  ))}
                </tbody>
              </TableFrame>
            )}
            {l.items.length > managed.length && (
              <p className="text-xs text-base-content/60">{l.items.length - managed.length} host interfaces not managed by Incus are not shown.</p>
            )}
          </>
        );
      }}
    </Section>
  );
}

/** Storage: the Incus storage pools (read-only). */
export function StorageView({ onAuth }: { onAuth: (m: string) => void }) {
  const [pools, reload] = useLoad<StoragePoolList>(() => client.get("storage_pools"), onAuth);
  return (
    <Section title="Storage pools" state={pools} testId="storage" onRetry={reload} level={1}>
      {(l) => (
        <>
          <SectionHeader title="Storage pools" level={1} count={l.items.length} source={l.source} />
          <TableFrame>
            <thead><tr><th>Pool</th><th>Driver</th><th>Status</th><th>Description</th></tr></thead>
            <tbody>
              {l.items.map((p) => (
                <tr key={p.name} data-testid="pool-row">
                  <td className="font-medium">{p.name}</td>
                  <td>{p.driver}</td>
                  <td>{p.status}</td>
                  <td>{p.description || "—"}</td>
                </tr>
              ))}
            </tbody>
          </TableFrame>
        </>
      )}
    </Section>
  );
}

import type { NetworkList, ProjectList, StoragePoolList } from "@apelora/api-client";

import { Section, SectionHeader, TableFrame } from "../components/ui";
import { client, useLoad } from "../lib/api";

export function InfrastructureView({ onAuth }: { onAuth: (m: string) => void }) {
  const [projects, reloadProjects] = useLoad<ProjectList>(() => client.get("projects"), onAuth);
  const [networks, reloadNetworks] = useLoad<NetworkList>(() => client.get("networks"), onAuth);
  const [pools, reloadPools] = useLoad<StoragePoolList>(() => client.get("storage_pools"), onAuth);
  return (
    <div className="flex flex-col gap-8">
      <Section title="Projects" state={projects} testId="projects" onRetry={reloadProjects}>
        {(l) => (
          <>
            <SectionHeader title="Projects" count={l.items.length} source={l.source} />
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
      <Section title="Networks" state={networks} testId="networks" onRetry={reloadNetworks}>
        {(l) => {
          const managed = l.items.filter((n) => n.managed);
          return (
            <>
              <SectionHeader title="Networks" count={managed.length} source={l.source} />
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
      <Section title="Storage pools" state={pools} testId="storage" onRetry={reloadPools}>
        {(l) => (
          <>
            <SectionHeader title="Storage pools" count={l.items.length} source={l.source} />
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
    </div>
  );
}

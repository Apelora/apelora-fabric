import type { ProjectList } from "@apelora/api-client";

import { Section, SectionHeader, TableFrame } from "../components/ui";
import { client, useLoad } from "../lib/api";

/** Incus projects (Administration): which are restricted, and what they are for. */
export function ProjectsSection({ onAuth }: { onAuth: (m: string) => void }) {
  const [projects, reload] = useLoad<ProjectList>(() => client.get("incus_projects"), onAuth);
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

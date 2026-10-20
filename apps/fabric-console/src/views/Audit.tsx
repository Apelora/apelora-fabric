import type { AuditList } from "@apelora/api-client";

import { Section, SectionHeader, TableFrame } from "../components/ui";
import { client, useLoad } from "../lib/api";

/** "instance.create.requested" → "Instance create requested". */
const describe = (action: string) => {
  const s = action.replace(/[.-]/g, " ");
  return s.charAt(0).toUpperCase() + s.slice(1);
};

export function AuditView({ onAuth }: { onAuth: (m: string) => void }) {
  const [log, reload] = useLoad<AuditList>(() => client.get("audit"), onAuth);
  return (
    <Section title="Audit log" level={1} state={log} testId="audit" onRetry={reload}>
      {(l) => (
        <>
          <SectionHeader title="Audit log" level={1} count={l.items.length}>
            <button type="button" className="btn btn-sm btn-ghost" onClick={reload}>Refresh</button>
          </SectionHeader>
          <p className="text-sm text-base-content/70">Who asked for what, newest first. Secrets such as tokens are never recorded.</p>
          {l.items.length === 0 ? <p className="text-sm text-base-content/70">Nothing recorded yet.</p> : (
            <TableFrame>
              <thead><tr><th>When</th><th>Who</th><th>What</th><th>Target</th></tr></thead>
              <tbody>
                {l.items.map((a) => (
                  <tr key={a.id} data-testid="audit-row">
                    <td className="whitespace-nowrap">{new Date(a.at).toLocaleString()}</td>
                    <td>{a.actor}</td>
                    <td>{describe(a.action)}</td>
                    <td className="font-mono text-xs">{a.target}</td>
                  </tr>
                ))}
              </tbody>
            </TableFrame>
          )}
        </>
      )}
    </Section>
  );
}

// SPDX-License-Identifier: AGPL-3.0-only
import { useId, useState } from "react";

import type { EffectiveSetting, EffectiveSettings } from "@apelora/api-client";

import { Heading, Section, SectionHeader, TableFrame } from "../components/ui";
import { client, useLoad } from "../lib/api";
import { fmtTime } from "../lib/identity";

const SOURCE: Record<string, string> = { environment: "Environment", default: "Default", "not set": "Not set", console: "Saved in the Console" };
const READ_BY: Record<string, string> = { "control-plane": "control plane", worker: "worker", "web-dev-server": "web development server" };
const HIDDEN: Record<string, string> = { secret: "Set, not shown: a secret", "looks like a credential": "Set, not shown: looks like a credential", "unknown setting": "Set, not shown" };

/** The value as people should read it: defaults muted, changed values at full strength, secrets never. */
function Value({ s }: { s: EffectiveSetting }) {
  if (s.hidden) return <span className="text-base-content/70" data-testid="setting-hidden">{HIDDEN[s.hidden] ?? "Set, not shown"}</span>;
  if (s.value == null || s.value === "") return <span className="text-base-content/60">Off</span>;
  return <code className={`break-all text-xs ${s.isDefault ? "text-base-content/60" : "font-semibold"}`}>{s.value}</code>;
}

function Rows({ items }: { items: EffectiveSetting[] }) {
  return (
    <TableFrame>
      <thead><tr><th>Setting</th><th>Value</th><th className="max-sm:hidden">From</th></tr></thead>
      <tbody>
        {items.map((s) => (
          <tr key={s.name} data-testid="setting-row" data-name={s.name} data-default={s.isDefault ? "yes" : "no"}>
            <td className="align-top">
              <span className="break-all font-mono text-xs">{s.name}</span>
              <span className="block text-xs text-base-content/60">
                {s.description}
                {s.readBy.length > 0 && <>; read by the {s.readBy.map((r) => READ_BY[r] ?? r).join(" and ")}</>}
              </span>
              <span className="block text-xs text-base-content/60 sm:hidden">{SOURCE[s.source] ?? s.source}</span>
            </td>
            <td className="align-top">
              <Value s={s} />
              {!s.known && <span className="mt-1 flex items-start gap-1.5 text-xs text-base-content/70"><span className="status status-warning mt-1 shrink-0" aria-hidden="true" />Not a setting this version reads: check the name</span>}
            </td>
            <td className="align-top whitespace-nowrap max-sm:hidden">{SOURCE[s.source] ?? s.source}</td>
          </tr>
        ))}
      </tbody>
    </TableFrame>
  );
}

/**
 * Administration → Settings: what this control plane runs with (`GET /settings`, `cluster.view`).
 * Read-only. Values come from the server's allow-list: secrets and key or certificate paths say
 * only that they are set.
 */
export function EffectiveSettingsSection({ onAuth }: { onAuth: (m: string) => void }) {
  const [state, reload] = useLoad<EffectiveSettings>(() => client.get("effective_settings"), onAuth);
  const [filter, setFilter] = useState("");
  const [changedOnly, setChangedOnly] = useState(false);
  const id = useId();
  return (
    <Section title="Settings" state={state} testId="settings" onRetry={reload}>
      {(s) => {
        const q = filter.trim().toLowerCase();
        const items = s.items.filter((x) => (!changedOnly || !x.isDefault) && (!q || x.name.toLowerCase().includes(q) || x.description.toLowerCase().includes(q)));
        const changed = s.items.filter((x) => !x.isDefault).length;
        return (
          <>
            <SectionHeader title="Settings">
              <span className="text-xs text-base-content/60">Read when it started, {fmtTime(s.readAt)}</span>
            </SectionHeader>
            <p className="max-w-2xl text-sm text-base-content/70">What this control plane started with; {changed} changed from the default. Secrets are never shown.</p>
            <div className="flex flex-wrap items-end gap-3">
              <label htmlFor={`${id}-q`} className="flex flex-col gap-1 text-sm">
                <span>Find a setting</span>
                <input id={`${id}-q`} type="search" className="input input-sm w-64 max-w-full" value={filter} onChange={(e) => setFilter(e.target.value)} placeholder="CONSOLE, OIDC, rate…" autoComplete="off" data-testid="settings-filter" />
              </label>
              <label className="flex items-center gap-2 pb-1 text-sm">
                <input type="checkbox" className="checkbox checkbox-sm" checked={changedOnly} onChange={(e) => setChangedOnly(e.target.checked)} data-testid="settings-changed" />
                Only those not at their default
              </label>
            </div>
            {items.length === 0 ? <p className="text-sm text-base-content/70" data-testid="settings-none">No setting matches.</p> : <Rows items={items} />}
            <div className="flex flex-col gap-2">
              <Heading>Saved in the Console</Heading>
              <Rows items={s.saved} />
            </div>
            <p className="max-w-2xl text-sm text-base-content/70" data-testid="settings-workers">
              {s.workers.length === 0
                ? "No worker has reported in the last day."
                : `Workers do not report their settings. Seen in the last day: ${s.workers.map((w) => `${w.id} on ${w.host}, version ${w.version}`).join("; ")}.`}
            </p>
          </>
        );
      }}
    </Section>
  );
}

// SPDX-License-Identifier: AGPL-3.0-only
import type { BackupState, DatabaseBackupReport } from "@apelora/api-client";

import { Badge, Notice, Section, SectionHeader, type Tone } from "../components/ui";
import { client, useLoad } from "../lib/api";
import { fmtTime } from "../lib/identity";

const STATE: Record<BackupState, { label: string; tone: Tone }> = {
  ok: { label: "Backed up and verified", tone: "ok" },
  warning: { label: "Needs attention", tone: "warn" },
  failing: { label: "Not protected", tone: "error" },
};

const size = (bytes: number) => (bytes >= 1 << 30 ? `${(bytes / (1 << 30)).toFixed(1)} GiB` : bytes >= 1 << 20 ? `${(bytes / (1 << 20)).toFixed(1)} MiB` : `${Math.max(1, Math.round(bytes / 1024))} KiB`);
const ROW = "flex flex-col gap-1 px-4 py-3 sm:flex-row sm:justify-between sm:gap-6";

/**
 * Administration → Apelora's database backups: whether the scripts' backups are recent and a
 * restore has been proven, from the records they leave in the backup directory. Read-only: the
 * scripts run on the host (runbooks/apelora-database-backup.md), never from here.
 */
export function DatabaseBackupsSection({ onAuth }: { onAuth: (m: string) => void }) {
  const [report, reload] = useLoad<DatabaseBackupReport>(() => client.get("database_backups"), onAuth);
  return (
    <Section title="Apelora's database backups" state={report} testId="database-backups" onRetry={reload}>
      {(r) => {
        if (!r.configured || !r.assessment) {
          return (
            <>
              <SectionHeader title="Apelora's database backups" />
              <Notice tone="neutral" testId="backups-not-configured">
                Not set up here. Set <code className="rounded bg-base-200 px-1 py-0.5 text-xs">APELORA_DB_BACKUP_DIR</code> on the control plane to the directory the backup script writes to, and this shows whether it runs and whether a restore has been proven. Without backups, losing the database loses every application, role and audit record.
              </Notice>
            </>
          );
        }
        const a = r.assessment;
        const s = STATE[a.state];
        return (
          <>
            <SectionHeader title="Apelora's database backups" />
            <div className="flex flex-col gap-3">
              <p className="flex flex-wrap items-center gap-2 text-sm" data-testid="backups-state" data-state={a.state}>
                <Badge tone={s.tone}>{s.label}</Badge>
                <span className="text-base-content/70">{r.backups} backup{r.backups === 1 ? "" : "s"} in <code className="text-xs">{r.directory}</code></span>
              </p>
              <ul className="flex flex-col gap-2" data-testid="backups-findings">
                {a.findings.map((f) => <li key={f.text}><Notice tone={f.level === "ok" ? "ok" : f.level === "warning" ? "warn" : "error"}>{f.text}</Notice></li>)}
              </ul>
              <dl className="divide-y divide-base-300 rounded-box border border-base-300 text-sm">
                <div className={ROW} data-testid="backups-row">
                  <dt className="font-medium">Newest backup</dt>
                  <dd className="text-base-content/70 sm:text-right">
                    {r.newestBackup
                      ? <>{fmtTime(r.newestBackup.createdAt)}{a.backupAgeHours != null ? ` · ${a.backupAgeHours} h ago` : ""}<span className="block text-xs">{size(r.newestBackup.sizeBytes)} · {r.newestBackup.encryption === "none" ? "not encrypted" : `encrypted (${r.newestBackup.encryption})`}{r.newestBackup.migrations != null ? ` · ${r.newestBackup.migrations} migrations` : ""}</span></>
                      : "none found"}
                  </dd>
                </div>
                <div className={ROW} data-testid="backups-row">
                  <dt className="font-medium">Last verified restore</dt>
                  <dd className="text-base-content/70 sm:text-right">
                    {r.lastVerified ? <>{fmtTime(r.lastVerified.drilledAt)}{a.drillAgeDays != null ? ` · ${a.drillAgeDays} days ago` : ""}<span className="block text-xs">took {r.lastVerified.durationSeconds} s</span></> : "never recorded"}
                  </dd>
                </div>
                {r.lastDrill && !r.lastDrill.passed && (
                  <div className={ROW} data-testid="backups-row">
                    <dt className="font-medium">Latest drill</dt>
                    <dd className="text-error sm:text-right">failed {fmtTime(r.lastDrill.drilledAt)}{r.lastDrill.reason ? `: ${r.lastDrill.reason}` : ""}</dd>
                  </div>
                )}
              </dl>
              <p className="text-xs text-base-content/60">Read from the records the backup and drill scripts write. Nothing here runs or restores anything: see the runbook <code>apelora-database-backup.md</code>.</p>
            </div>
          </>
        );
      }}
    </Section>
  );
}

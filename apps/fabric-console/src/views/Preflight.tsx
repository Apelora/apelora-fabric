import type { CheckStatus, PreflightReport } from "@apelora/api-client";

import { Section, SectionHeader } from "../components/ui";
import { client, useLoad } from "../lib/api";

const STATUS: Record<CheckStatus, { label: string; dot: string }> = {
  pass: { label: "Passed", dot: "status-success" },
  warn: { label: "Warning", dot: "status-warning" },
  fail: { label: "Failed", dot: "status-error" },
  "not-checked": { label: "Not checked", dot: "status-neutral" },
};

const OVERALL: Record<CheckStatus, string> = {
  pass: "Every check passed.",
  "not-checked": "Every check that can be made through the Incus API passed; some must be checked on the servers themselves.",
  warn: "The cluster works, but some checks need attention.",
  fail: "Some checks failed: fix them before deploying.",
};

export function PreflightView({ onAuth }: { onAuth: (m: string) => void }) {
  const [report, reload] = useLoad<PreflightReport>(() => client.get("preflight"), onAuth);
  return (
    <Section title="Preflight" level={1} state={report} testId="preflight" onRetry={reload}>
      {(r) => (
        <>
          <SectionHeader title="Preflight" level={1} source={r.source}>
            <button type="button" className="btn btn-sm btn-ghost" onClick={reload}>Check again</button>
          </SectionHeader>
          <p className="text-sm text-base-content/70" data-testid="preflight-overall">
            <span className={`status ${STATUS[r.overall].dot} mr-2`} aria-hidden="true" />
            {OVERALL[r.overall]} Nothing is changed by these checks.
          </p>
          <ul className="divide-y divide-base-300 rounded-box border border-base-300">
            {r.checks.map((c) => (
              <li key={c.id} className="flex flex-col gap-1 px-4 py-3 sm:flex-row sm:items-start sm:justify-between sm:gap-6" data-testid="preflight-check">
                <span className="text-sm font-medium">{c.title}</span>
                <span className="flex items-start gap-2 text-sm text-base-content/70 sm:max-w-[60%] sm:text-right">
                  <span className={`status ${STATUS[c.status].dot} mt-1.5 shrink-0`} aria-hidden="true" />
                  <span><span className="font-medium text-base-content">{STATUS[c.status].label}</span> · {c.detail}</span>
                </span>
              </li>
            ))}
          </ul>
        </>
      )}
    </Section>
  );
}

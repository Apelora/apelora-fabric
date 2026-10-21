// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";

import { ErrorPanel, Heading, Notice } from "../components/ui";
import { client } from "../lib/api";
import { useRun } from "../lib/identity";

const CONTENTS = [
  "Versions",
  "Settings, without secrets",
  "Health and preflight checks",
  "Jobs that failed in the last day, with their attempts",
  "A metrics snapshot",
];

const stamp = () => new Date().toISOString().replace(/[-:]/g, "").replace(/\..*$/, "").replace("T", "-");

/**
 * Administration → Support bundle (`support.bundle`): one tar file to hand to whoever helps.
 * Downloaded through the API and saved from a Blob; nothing is uploaded anywhere.
 */
export function SupportBundleSection({ onAuth }: { onAuth: (m: string) => void }) {
  const { busy, error, run } = useRun(onAuth);
  const [saved, setSaved] = useState<string | null>(null);
  const download = () => {
    setSaved(null);
    void run(async () => {
      const blob = await client.get("support_bundle");
      const name = `apelora-support-${stamp()}.tar`;
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = name;
      document.body.append(a);
      a.click();
      a.remove();
      setTimeout(() => URL.revokeObjectURL(url), 0);
      setSaved(`${name} (${Math.max(1, Math.round(blob.size / 1024))} KiB)`);
    });
  };
  return (
    <section className="flex flex-col gap-2" aria-label="Support bundle" data-testid="support-bundle">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <Heading>Support bundle</Heading>
        <button type="button" className="btn btn-sm btn-outline" onClick={download} disabled={busy} aria-busy={busy || undefined} data-testid="support-bundle-download">
          {busy && <span className="loading loading-spinner loading-xs" aria-hidden="true" />}
          {busy ? "Preparing…" : "Download support bundle"}
        </button>
      </div>
      <p className="max-w-2xl text-sm text-base-content/70">One file for whoever helps you with a problem. It holds:</p>
      <ul className="list-disc pl-5 text-sm text-base-content/70">
        {CONTENTS.map((c) => <li key={c}>{c}</li>)}
      </ul>
      <p className="max-w-2xl text-sm text-base-content/70">Never tokens, passwords, keys or log lines. Each download is audited; nothing is uploaded.</p>
      {saved && <Notice tone="ok" testId="support-bundle-saved">Saved {saved}. Check it before you share it.</Notice>}
      {error && <ErrorPanel message={error} testId="support-bundle-error" />}
    </section>
  );
}

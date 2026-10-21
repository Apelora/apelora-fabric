import { useState } from "react";

import { useBackend } from "../../features/applications/backend";
import { useLifecycle } from "../../features/applications/lifecycle";
import type { AppDetailView, AppView } from "../../features/applications/model";
import { useAppToasts } from "../../features/applications/toast";
import { IconSpinner, IconUpload } from "../icons";
import { DeployVersionDialog } from "./DeployVersionDialog";

/** Why this application cannot be deployed right now, or null when it can. */
export function deployReason(app: AppView, kind: "api" | "local"): string | null {
  if (kind !== "api") return "Deploying needs a control plane. Connect to one from the header.";
  if (app.archived) return "This application is archived. Restore it in Settings first.";
  if (app.latestRevision == null) return "Nothing is published yet. Open the designer and publish a version first.";
  return null;
}

/** Deploy…: asks which published version, shows what would change, and only then deploys. */
export function DeployAction({ app, size = "xs" }: { app: AppView; size?: "xs" | "sm" }) {
  const backend = useBackend((s) => s.backend);
  const toast = useAppToasts((s) => s.push);
  const [detail, setDetail] = useState<AppDetailView | null>(null);
  const [busy, setBusy] = useState(false);
  const reason = deployReason(app, backend.kind);

  const open = () => {
    if (busy || reason) return;
    setBusy(true);
    backend.get(app.name).then(setDetail, (e: unknown) => toast("error", e instanceof Error ? e.message : String(e))).finally(() => setBusy(false));
  };

  return (
    <>
      <span title={reason ?? `Deploy a published version of ${app.name}`}>
        <button type="button" className={`btn btn-${size} btn-outline`} disabled={!!reason || busy} aria-label={`Deploy ${app.name}`} data-testid="action-deploy" data-allowed={!reason} onClick={open}>
          {busy ? <span className="animate-spin"><IconSpinner size={13} /></span> : <IconUpload size={13} />}
          <span>Deploy…</span>
        </button>
      </span>
      {detail && <DeployVersionDialog app={detail.application} versions={detail.versions} onClose={() => setDetail(null)} onDone={() => useLifecycle.setState((s) => ({ bump: s.bump + 1 }))} />}
    </>
  );
}

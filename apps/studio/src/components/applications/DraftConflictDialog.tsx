import { useState } from "react";

import type { DraftSync } from "../../features/applications/draftSync";

/** Shown when the draft changed elsewhere: the person chooses; nothing is overwritten silently. */
export function DraftConflictDialog({ sync }: { sync: DraftSync }) {
  const [busy, setBusy] = useState(false);
  if (sync.kind !== "conflict") return null;
  const act = async (f: () => Promise<void>) => { setBusy(true); try { await f(); } finally { setBusy(false); } };
  return (
    <dialog className="modal modal-open" aria-labelledby="conflict-title" data-testid="draft-conflict">
      <div className="modal-box max-w-md">
        <h3 id="conflict-title" className="text-base font-semibold">This draft changed elsewhere</h3>
        <p className="mt-2 text-sm text-base-content/80">{sync.message} Your changes since then are not saved yet, and saving is paused so nothing is overwritten by accident.</p>
        <div className="modal-action flex-wrap">
          <button type="button" className="btn btn-sm" disabled={busy} onClick={() => void act(sync.loadLatest)} data-testid="conflict-load-latest">Load the newest draft (discard my changes)</button>
          <button type="button" className="btn btn-sm btn-error" disabled={busy} onClick={() => void act(sync.overwrite)} data-testid="conflict-overwrite">Keep mine and overwrite</button>
        </div>
      </div>
    </dialog>
  );
}

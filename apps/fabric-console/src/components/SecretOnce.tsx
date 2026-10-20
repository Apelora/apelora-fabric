import { useState } from "react";

import { FormDialog, Notice } from "./ui";

export type OneTimeSecret = { title: string; what: string; value: string };

/**
 * Shows a secret the API returns exactly once. The value exists only in the parent's state while
 * the dialog is open: closing it (button or Escape) clears that state, so nothing stays in the page.
 * It is never put in storage, the URL or a log.
 */
export function SecretOnceDialog({ secret, onClose }: { secret: OneTimeSecret | null; onClose: () => void }) {
  const [copied, setCopied] = useState<"yes" | "no" | null>(null);
  const close = () => { setCopied(null); onClose(); };
  return (
    <FormDialog open={!!secret} title={secret?.title ?? "Copy this now"} onClose={close} testId="secret-dialog">
      {secret && (
        <div className="flex flex-col gap-3">
          <Notice tone="warn" title="Shown only once">
            Copy the {secret.what} now and keep it in your secret store. Apelora cannot show it again; if it is lost, create a new one.
          </Notice>
          <label className="flex flex-col gap-1 text-sm">
            <span>{secret.what.charAt(0).toUpperCase() + secret.what.slice(1)}</span>
            <input readOnly className="input input-sm w-full font-mono text-xs" value={secret.value} autoComplete="off" spellCheck={false} onFocus={(e) => e.currentTarget.select()} data-testid="secret-value" />
          </label>
          {copied === "no" && <Notice tone="error">Your browser did not allow copying. Select the text above and copy it yourself.</Notice>}
          <div className="modal-action mt-0">
            <button type="button" className="btn btn-sm btn-outline" onClick={() => void navigator.clipboard.writeText(secret.value).then(() => setCopied("yes"), () => setCopied("no"))} data-testid="secret-copy">{copied === "yes" ? "Copied" : `Copy ${secret.what}`}</button>
            <button type="button" className="btn btn-sm btn-primary" onClick={close} data-testid="secret-close">I have stored it</button>
          </div>
        </div>
      )}
    </FormDialog>
  );
}

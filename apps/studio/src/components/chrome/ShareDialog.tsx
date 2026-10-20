import { useEffect, useState } from "react";
import { useStudio, useStudioApi } from "../../features/editor/context";
import { encodeShare } from "../../features/share/link";
import { Modal } from "./Modal";

/** A link that carries a small diagram in its address; nothing is uploaded anywhere. */
export function ShareDialog() {
  const api = useStudioApi();
  const open = useStudio((s) => s.dialog === "share");
  const assembly = useStudio((s) => s.assembly);
  const [state, setState] = useState<{ url: string } | { error: string } | null>(null);
  useEffect(() => {
    if (!open) { setState(null); return; }
    let live = true;
    void encodeShare(assembly).then((r) => { if (live) setState(r.ok ? { url: `${location.origin}${location.pathname}${r.fragment}` } : { error: r.error }); }).catch((e: Error) => { if (live) setState({ error: e.message }); });
    return () => { live = false; };
  }, [open, assembly]);
  if (!open) return null;
  const close = () => api.getState().setDialog(null);
  return (
    <Modal testId="share-dialog" title="Share as a link" hint="The diagram is packed into the address after the # sign, so it never leaves the browser until you send the link. Whoever opens it gets a copy." onClose={close} size="max-w-xl">
      {!state && <p className="text-sm text-base-content/60">Preparing the link…</p>}
      {state && "error" in state && <p className="text-sm text-error" data-testid="share-error">{state.error}</p>}
      {state && "url" in state && (
        <div className="flex flex-col gap-2">
          <textarea readOnly className="textarea textarea-sm h-24 w-full font-mono text-[11px]" data-testid="share-url" aria-label="Share link" value={state.url} onFocus={(e) => e.currentTarget.select()} />
          <button type="button" className="btn btn-sm btn-primary self-start" data-testid="share-copy" onClick={() => { void navigator.clipboard.writeText(state.url).then(() => api.getState().toast("success", "Link copied."), () => api.getState().toast("warning", "Copy failed; select the link and copy it by hand.")); }}>Copy link</button>
          <p className="text-xs text-base-content/60">{state.url.length.toLocaleString()} characters. Secrets are never stored in a diagram, only references to them.</p>
        </div>
      )}
    </Modal>
  );
}

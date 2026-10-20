import { useEffect, useState } from "react";
import { markTourSeen, tourSeen } from "../../features/editor/persistence";

const STEPS = [
  { title: "Add modules", body: "Drag a tile from the catalog onto the canvas, or click it to place one. Search finds any of them." },
  { title: "Plug cables", body: "Drag from a right-hand jack to a left-hand jack. Compatible sockets light up while you drag. Right-click a jack to add something that fits." },
  { title: "Right-click anything", body: "Modules, jacks, cables, zones and the canvas all have a menu. Properties open in a floating window." },
  { title: "Keep it tidy", body: "Use Tidy to lay the flow out left to right, add zones and notes, and press ? any time for the shortcuts." },
] as const;

/** A short, non-blocking walkthrough shown once on a first visit (and from the help sheet). */
export function Tour() {
  const [step, setStep] = useState<number | null>(() => (tourSeen() ? null : 0));
  useEffect(() => {
    const start = () => setStep(0);
    window.addEventListener("apelora:tour", start);
    return () => window.removeEventListener("apelora:tour", start);
  }, []);
  if (step === null) return null;
  const done = () => { markTourSeen(); setStep(null); };
  const s = STEPS[step]!;
  return (
    <aside className="fixed bottom-4 right-4 z-40 w-72 rounded-box border border-base-300 bg-base-100 p-3 text-xs shadow-lg" role="region" aria-label="Quick tour" data-testid="tour">
      <div className="mb-1 flex items-center justify-between"><span className="text-[10px] uppercase tracking-wide text-base-content/50">Quick tour · {step + 1} of {STEPS.length}</span><button type="button" className="btn btn-ghost btn-xs" data-testid="tour-skip" onClick={done}>Skip</button></div>
      <h4 className="text-sm font-semibold">{s.title}</h4>
      <p className="mt-1 text-base-content/70">{s.body}</p>
      <div className="mt-2 flex justify-end gap-2">
        {step > 0 && <button type="button" className="btn btn-xs" onClick={() => setStep(step - 1)}>Back</button>}
        <button type="button" className="btn btn-xs btn-primary" data-testid="tour-next" onClick={() => (step === STEPS.length - 1 ? done() : setStep(step + 1))}>{step === STEPS.length - 1 ? "Done" : "Next"}</button>
      </div>
    </aside>
  );
}

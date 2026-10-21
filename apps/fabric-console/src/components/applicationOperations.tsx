import { useEffect, useRef, useState } from "react";

import { ApiRequestError, type ActionState, type OperationDto, type OperationKind } from "@apelora/api-client";

import { client, idempotencyKey, messageOf } from "../lib/api";
import { ConfirmDialog } from "./ui";

const PRESENT: Record<OperationKind, string> = { start: "Starting", stop: "Stopping", restart: "Restarting" };
const PAST: Record<OperationKind, string> = { start: "Started", stop: "Stopped", restart: "Restarted" };
const final = (o: OperationDto) => o.state === "succeeded" || o.state === "failed" || o.state === "cancelled";

/** What the row says about the operation it started: in progress, done, or why it failed. */
export type OperationNote = { kind: OperationKind; text: string; tone: "info" | "ok" | "error" };

const note = (o: OperationDto): OperationNote => ({
  kind: o.kind,
  tone: o.state === "succeeded" ? "ok" : final(o) ? "error" : "info",
  text: o.state === "succeeded" ? `${PAST[o.kind]}.`
    : o.state === "failed" ? `${PRESENT[o.kind]} failed: ${o.error ?? "see Operations"}`
    : o.state === "cancelled" ? `${PRESENT[o.kind]} was cancelled.`
    : `${PRESENT[o.kind]}… ${o.stepsDone} of ${o.stepsTotal} steps`,
});

/**
 * Start, stop and restart of an application's instances, as the server allows them: a start goes
 * at once; a stop or restart asks first, and when the server says it will interrupt clients of a
 * single-instance or stateful module it says so in the same dialog and asks again. The row then
 * follows the operation until it ends. `onDone` runs when one ends, to read the state again.
 */
export function useApplicationOperations(onDone: () => void) {
  const [notes, setNotes] = useState<Record<string, OperationNote>>({});
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState<string | null>(null);
  const [ask, setAsk] = useState<{ name: string; kind: OperationKind; disruption: string | null } | null>(null);
  const following = useRef<Record<string, string>>({});
  const [tick, setTick] = useState(0);
  const done = useRef(onDone);
  done.current = onDone;

  // Every running operation is read again every couple of seconds until it ends.
  useEffect(() => {
    const names = Object.keys(following.current);
    if (names.length === 0) return;
    const t = setTimeout(() => {
      void Promise.all(names.map(async (name) => {
        try {
          const op = await client.request("application_operation", { params: { name, id: following.current[name]! } });
          setNotes((n) => ({ ...n, [name]: note(op) }));
          if (final(op)) { delete following.current[name]; done.current(); }
        } catch (e) {
          delete following.current[name];
          setErrors((x) => ({ ...x, [name]: messageOf(e) }));
        }
      })).then(() => setTick((n) => n + 1));
    }, 1500);
    return () => clearTimeout(t);
  }, [tick, notes]);

  const send = async (name: string, kind: OperationKind, confirmDisruption: boolean) => {
    setBusy(name);
    setErrors((e) => { const { [name]: _, ...rest } = e; return rest; });
    try {
      const res = await client.request("operate_application", { params: { name }, body: { action: kind, confirmDisruption }, idempotencyKey: idempotencyKey() });
      following.current[name] = res.operation.id;
      setNotes((n) => ({ ...n, [name]: note(res.operation) }));
      setAsk(null);
      setTick((n) => n + 1);
    } catch (e) {
      if (e instanceof ApiRequestError && e.code === "confirmation_required") {
        setAsk({ name, kind, disruption: e.message });
      } else {
        setAsk(null);
        setErrors((x) => ({ ...x, [name]: messageOf(e) }));
      }
    } finally {
      setBusy(null);
    }
  };

  const request = (name: string, kind: OperationKind) => (kind === "start" ? void send(name, kind, false) : setAsk({ name, kind, disruption: null }));

  const dialog = (
    <ConfirmDialog
      open={!!ask}
      title={ask ? `${ask.kind === "stop" ? "Stop" : "Restart"} ${ask.name}?` : ""}
      body={ask && (
        <>
          <p>{ask.disruption ?? (ask.kind === "stop"
            ? "Its instances are stopped in dependency order, so what depends on a module stops first. Data and volumes are kept."
            : "Its instances are stopped and started again in dependency order. Data and volumes are kept.")}</p>
        </>
      )}
      action={ask?.disruption ? (ask.kind === "stop" ? "Stop and interrupt service" : "Restart and interrupt service") : ask?.kind === "stop" ? "Stop" : "Restart"}
      dismiss="Keep it as it is"
      busy={busy === ask?.name}
      onCancel={() => setAsk(null)}
      onConfirm={() => { if (ask) void send(ask.name, ask.kind, !!ask.disruption); }}
    />
  );

  return { notes, errors, busy, request, dialog };
}

/** The Start, Restart and Stop buttons the server allows (with its reason when none is), and what the last one did. */
export function ApplicationActions({ name, actions, control }: {
  name: string;
  actions: { start: ActionState; stop: ActionState; restart: ActionState };
  control: ReturnType<typeof useApplicationOperations>;
}) {
  const running = !!control.notes[name] && control.notes[name]!.tone === "info";
  const allowed = (["start", "restart", "stop"] as const).filter((k) => actions[k].allowed);
  const why = allowed.length === 0 ? actions.start.reason : null;
  const n = control.notes[name];
  const error = control.errors[name];
  return (
    <div className="flex flex-col items-end gap-1" data-testid="application-actions">
      <span className="inline-flex flex-wrap justify-end gap-1">
        {allowed.map((k) => (
          <button key={k} type="button" className={`btn btn-xs btn-ghost ${k === "stop" ? "text-error" : ""}`} disabled={running || control.busy === name}
            aria-label={`${k === "start" ? "Start" : k === "stop" ? "Stop" : "Restart"} ${name}`} onClick={() => control.request(name, k)} data-testid={`application-${k}`}>
            {k === "start" ? "Start" : k === "stop" ? "Stop…" : "Restart…"}
          </button>
        ))}
      </span>
      {why && <span className="max-w-48 text-right text-xs text-base-content/60" data-testid="application-actions-why">{why}</span>}
      {n && <span role="status" className={`max-w-56 text-right text-xs ${n.tone === "error" ? "text-error" : n.tone === "ok" ? "text-base-content" : "text-base-content/70"}`} data-testid="application-operation">{n.text}</span>}
      {error && <span role="alert" className="max-w-56 text-right text-xs text-error" data-testid="application-action-error">{error}</span>}
    </div>
  );
}

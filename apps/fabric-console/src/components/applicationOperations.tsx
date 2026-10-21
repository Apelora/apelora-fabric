import { useEffect, useRef, useState } from "react";

import { ApiRequestError, type ActionState, type OperationDto, type OperationKind } from "@apelora/api-client";

import { client, idempotencyKey, messageOf } from "../lib/api";
import { ConfirmDialog } from "./ui";

const PRESENT: Record<OperationKind, string> = { start: "Starting", stop: "Stopping", shutdown: "Shutting down", restart: "Restarting", migrate: "Migrating" };
const PAST: Record<OperationKind, string> = { start: "Started", stop: "Stopped", shutdown: "Shut down", restart: "Restarted", migrate: "Migrated" };
/** What each button is called, and what its dialog says. An older control plane has no `shutdown`: its `stop` is the graceful one. */
const LABEL: Record<string, string> = { start: "Start", restart: "Restart", shutdown: "Shut down", stop: "Stop" };
const WHAT: Partial<Record<OperationKind, string>> = {
  shutdown: "Its instances are asked to shut down in dependency order, so what depends on a module stops first. Each gets time to finish. Data and volumes are kept.",
  stop: "Every instance is powered off at once, in dependency order, without waiting for it to shut down. Anything held only in memory is lost; data on disks and volumes is kept.",
  restart: "Its instances are stopped and started again in dependency order. Data and volumes are kept.",
};
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
 * Start, restart, shut down and stop of an application's instances, as the server allows them: a
 * start goes at once; the others ask first, and when the server says it will interrupt clients of a
 * single-instance or stateful module it says so in the same dialog and asks again. The row then
 * follows the operation until it ends. `onDone` runs when one ends, to read the state again.
 */
export function useApplicationOperations(onDone: () => void) {
  const [notes, setNotes] = useState<Record<string, OperationNote>>({});
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState<string | null>(null);
  const [ask, setAsk] = useState<{ name: string; kind: OperationKind; disruption: string | null; immediate: boolean } | null>(null);
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

  const send = async (name: string, kind: OperationKind, confirmDisruption: boolean, immediate = true) => {
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
        setAsk({ name, kind, disruption: e.message, immediate });
      } else {
        setAsk(null);
        setErrors((x) => ({ ...x, [name]: messageOf(e) }));
      }
    } finally {
      setBusy(null);
    }
  };

  /** `immediate`: false for the `stop` of a control plane that has no `shutdown`, where `stop` is the graceful one. */
  const request = (name: string, kind: OperationKind, immediate = true) => (kind === "start" ? void send(name, kind, false) : setAsk({ name, kind, disruption: null, immediate }));

  const dialog = (
    <ConfirmDialog
      open={!!ask}
      title={ask ? `${ask.kind === "stop" && !ask.immediate ? "Stop" : ask.kind === "stop" ? "Stop now" : LABEL[ask.kind] ?? ask.kind} ${ask.name}?` : ""}
      body={ask && (
        <>
          <p>{ask.disruption ?? (ask.kind === "stop" && !ask.immediate
            ? "Its instances are stopped in dependency order, so what depends on a module stops first. Data and volumes are kept."
            : WHAT[ask.kind] ?? "")}</p>
        </>
      )}
      action={`${LABEL[ask?.kind ?? "stop"] ?? "Continue"}${ask?.disruption ? " and interrupt service" : ""}`}
      dismiss="Keep it as it is"
      busy={busy === ask?.name}
      onCancel={() => setAsk(null)}
      onConfirm={() => { if (ask) void send(ask.name, ask.kind, !!ask.disruption, ask.immediate); }}
    />
  );

  return { notes, errors, busy, request, dialog };
}

/** The Start, Restart, Shut down and Stop buttons the server allows (with its reason when none is), and what the last one did. */
export function ApplicationActions({ name, actions, control }: {
  name: string;
  actions: { start: ActionState; stop: ActionState; restart: ActionState; shutdown?: ActionState };
  control: ReturnType<typeof useApplicationOperations>;
}) {
  const running = !!control.notes[name] && control.notes[name]!.tone === "info";
  // An older control plane has no `shutdown`: its `stop` is the graceful one, so it is offered as it was.
  const legacy = actions.shutdown === undefined;
  const kinds = (legacy ? ["start", "restart", "stop"] : ["start", "restart", "shutdown", "stop"]) as ("start" | "restart" | "shutdown" | "stop")[];
  const allowed = kinds.filter((k) => (actions[k] ?? actions.stop).allowed);
  const why = allowed.length === 0 ? actions.start.reason : null;
  const n = control.notes[name];
  const error = control.errors[name];
  const label = (k: string) => (k === "start" ? "Start" : k === "restart" ? "Restart…" : k === "shutdown" ? "Shut down…" : "Stop…");
  return (
    <div className="flex flex-col items-end gap-1" data-testid="application-actions">
      <span className="inline-flex flex-wrap justify-end gap-1">
        {allowed.map((k) => (
          <button key={k} type="button" className={`btn btn-xs btn-ghost ${k === "stop" ? "text-error" : ""}`} disabled={running || control.busy === name}
            aria-label={`${LABEL[k]} ${name}`} onClick={() => control.request(name, k, !(legacy && k === "stop"))} data-testid={`application-${k}`}>
            {label(k)}
          </button>
        ))}
      </span>
      {why && <span className="max-w-48 text-right text-xs text-base-content/60" data-testid="application-actions-why">{why}</span>}
      {n && <span role="status" className={`max-w-56 text-right text-xs ${n.tone === "error" ? "text-error" : n.tone === "ok" ? "text-base-content" : "text-base-content/70"}`} data-testid="application-operation">{n.text}</span>}
      {error && <span role="alert" className="max-w-56 text-right text-xs text-error" data-testid="application-action-error">{error}</span>}
    </div>
  );
}

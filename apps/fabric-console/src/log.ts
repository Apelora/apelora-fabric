// SPDX-License-Identifier: AGPL-3.0-only
/** Structured browser logs. Never send them off the origin; never print secrets. */

export type LogLevel = "debug" | "info" | "warn" | "error";

const SECRET = /pass(word)?|secret|token|authorization|cookie|apikey|api_key/i;

function scrub(fields: Record<string, unknown>): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(fields)) {
    out[key] = SECRET.test(key) ? "[redacted]" : value;
  }
  return out;
}

/** One JSON line on the console, with a UTC timestamp and a level. */
export function log(level: LogLevel, message: string, fields: Record<string, unknown> = {}): void {
  const line = JSON.stringify({ ts: new Date().toISOString(), level, msg: message, ...scrub(fields) });
  if (level === "error") console.error(line);
  else if (level === "warn") console.warn(line);
  else console.info(line);
}

/** Catch uncaught errors so a blank screen still leaves a line in the browser console. */
export function installWindowLogging(): void {
  window.addEventListener("error", (event) => {
    log("error", "uncaught error", { message: event.message });
  });
  window.addEventListener("unhandledrejection", (event) => {
    log("error", "unhandled rejection", { reason: String(event.reason) });
  });
}

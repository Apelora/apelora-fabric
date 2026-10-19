// SPDX-License-Identifier: AGPL-3.0-only
// Typed client for the Apelora control plane (`/api/v1`). Types and routes come from
// generated.ts, which is generated from the control plane's OpenAPI document.
import { operations, type ErrorBody, type Responses } from "./generated";

export * from "./generated";

export type Operation = keyof Responses;

/** A non-2xx response, carrying the API's stable error code. */
export class ApiRequestError extends Error {
  readonly status: number;
  readonly code: string;
  constructor(status: number, code: string, message: string) {
    super(message);
    this.name = "ApiRequestError";
    this.status = status;
    this.code = code;
  }
  /** The caller has no valid token (401). */
  get unauthenticated() {
    return this.status === 401;
  }
}

export type ClientOptions = {
  /** Prefix for API paths; "" uses the page's origin (the dev servers proxy `/api`). */
  baseUrl?: string;
  /** Returns the API token to send, if any. Read on every request. */
  token?: () => string | null;
  fetch?: typeof globalThis.fetch;
};

export function createClient({ baseUrl = "", token, fetch: doFetch = globalThis.fetch.bind(globalThis) }: ClientOptions = {}) {
  return {
    async get<K extends Operation>(op: K): Promise<Responses[K]> {
      const { path, public: open } = operations[op];
      const t = open ? null : (token?.() ?? null);
      let res: Response;
      try {
        res = await doFetch(baseUrl + path, { headers: { accept: "application/json", ...(t ? { authorization: `Bearer ${t}` } : {}) } });
      } catch (e) {
        throw new ApiRequestError(0, "network_error", e instanceof Error ? e.message : String(e));
      }
      const body = (await res.json().catch(() => null)) as Responses[K] | ErrorBody | null;
      if (!res.ok || body === null) {
        const err = (body as ErrorBody | null)?.error;
        throw new ApiRequestError(res.status, err?.code ?? "http_error", err?.message ?? `HTTP ${res.status}`);
      }
      return body as Responses[K];
    },
  };
}

export type ApiClient = ReturnType<typeof createClient>;

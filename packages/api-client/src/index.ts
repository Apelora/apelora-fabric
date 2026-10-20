// SPDX-License-Identifier: AGPL-3.0-only
// Typed client for the Apelora control plane (`/api/v1`). Types and routes come from
// generated.ts, which is generated from the control plane's OpenAPI document.
import { operations, type Bodies, type ErrorBody, type Responses } from "./generated";

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

type Params<K extends Operation> = { [P in (typeof operations)[K]["params"][number]]: string };

/** Options for one request. `params` fills the path template; `body` is sent as JSON. */
export type RequestOptions<K extends Operation> = ([keyof Params<K>] extends [never] ? { params?: undefined } : { params: Params<K> }) &
  ([Bodies[K]] extends [never] ? { body?: undefined } : { body: Bodies[K] }) & {
    /** Sent as `Idempotency-Key`: repeating a request with the same key returns the first result. */
    idempotencyKey?: string;
  };

/** Operations that need no path parameters and no body. */
type Simple = { [K in Operation]: [keyof Params<K>] extends [never] ? ([Bodies[K]] extends [never] ? K : never) : never }[Operation];

export function createClient({ baseUrl = "", token, fetch: doFetch = globalThis.fetch.bind(globalThis) }: ClientOptions = {}) {
  async function request<K extends Operation>(op: K, options: RequestOptions<K>): Promise<Responses[K]> {
    const { method, path, public: open } = operations[op];
    const params = (options.params ?? {}) as Record<string, string>;
    const url = path.replace(/\{(\w+)\}/g, (_, name: string) => {
      const value = params[name];
      if (value === undefined) throw new Error(`${op}: missing path parameter ${name}`);
      return encodeURIComponent(value);
    });
    const t = open ? null : (token?.() ?? null);
    const headers: Record<string, string> = { accept: "application/json" };
    if (t) headers.authorization = `Bearer ${t}`;
    if (options.idempotencyKey) headers["idempotency-key"] = options.idempotencyKey;
    if (options.body !== undefined) headers["content-type"] = "application/json";
    let res: Response;
    try {
      res = await doFetch(baseUrl + url, { method, headers, body: options.body === undefined ? undefined : JSON.stringify(options.body) });
    } catch (e) {
      throw new ApiRequestError(0, "network_error", e instanceof Error ? e.message : String(e));
    }
    const body = (await res.json().catch(() => null)) as Responses[K] | ErrorBody | null;
    if (!res.ok || body === null) {
      const err = (body as ErrorBody | null)?.error;
      throw new ApiRequestError(res.status, err?.code ?? "http_error", err?.message ?? `HTTP ${res.status}`);
    }
    return body as Responses[K];
  }

  return {
    request,
    /** Shorthand for operations without path parameters or a body. */
    get<K extends Simple>(op: K): Promise<Responses[K]> {
      return request(op, {} as RequestOptions<K>);
    },
  };
}

export type ApiClient = ReturnType<typeof createClient>;

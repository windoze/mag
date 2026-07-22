import type { Command, Event } from "@mag/protocol";

/** Function signature used by transports that run in browser or test environments. */
export type FetchLike = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;

/** Callback invoked for each decoded protocol event. */
export type EventHandler = (event: Event) => void;

/** Optional controls for a live event subscription. */
export interface SubscribeOptions {
  /** External cancellation signal for the subscription. */
  readonly signal?: AbortSignal;
  /** Receives stream, HTTP, and decoding failures. */
  readonly onError?: (error: unknown) => void;
}

/** Transport abstraction shared by web and future desktop shells. */
export interface ITransport {
  /** Implementation family for capability probing. */
  readonly kind: "web" | "tauri";

  /** Sends one protocol command and resolves with the route-specific response body. */
  send(command: Command): Promise<unknown>;

  /** Subscribes to the live event stream and returns an unsubscribe callback. */
  subscribe(handler: EventHandler, options?: SubscribeOptions): () => void;
}

/** Options used to construct the HTTP + SSE transport. */
export interface HttpSseTransportOptions {
  /** Base URL for the mag-web server; empty means same-origin relative `/api` paths. */
  readonly baseUrl?: string;
  /** Bearer token or token supplier. Undefined disables header injection. */
  readonly token?: string | (() => string | undefined);
  /** Fetch implementation, mainly for tests. Defaults to `globalThis.fetch`. */
  readonly fetch?: FetchLike;
}

/** Error body projected by mag-web REST handlers. */
export interface RestErrorBody {
  /** Stable machine-readable error kind, for example `not_pivotable`. */
  readonly kind: string;
  /** Human-readable error message from the service projection. */
  readonly message: string;
}

/** Typed transport failure preserving HTTP status and projected service error kind. */
export class TransportError extends Error {
  /** HTTP status when the failure came from a response. */
  readonly status?: number;
  /** Stable error kind, preserving mag-web's `{kind,message}` body when present. */
  readonly kind: string;
  /** Parsed response body, when it was valid JSON. */
  readonly body?: unknown;

  /** Creates a typed transport error. */
  constructor(message: string, options: { kind: string; status?: number; body?: unknown }) {
    super(message);
    this.name = "TransportError";
    this.kind = options.kind;
    this.status = options.status;
    this.body = options.body;
    Object.setPrototypeOf(this, new.target.prototype);
  }
}

interface RequestPlan {
  readonly method: "DELETE" | "GET" | "POST" | "PUT";
  readonly path: string;
  readonly body?: unknown;
}

/** Maps a transport-neutral command to the REST route specified by docs/WEB.md §2.1. */
export function commandToRequest(command: Command): RequestPlan {
  switch (command.type) {
    case "list_sessions":
      return { method: "GET", path: "/api/sessions" };
    case "create_session":
      return {
        method: "POST",
        path: "/api/sessions",
        body: {
          ...(command.cwd === undefined || command.cwd === null ? {} : { cwd: command.cwd }),
          ...(command.agent === undefined || command.agent === null
            ? {}
            : { agent: command.agent })
        }
      };
    case "resume_session":
      return { method: "POST", path: `/api/sessions/${encodePathSegment(command.id)}/resume` };
    case "get_session_history":
      return { method: "GET", path: `/api/sessions/${encodePathSegment(command.id)}/history` };
    case "delete_session":
      return { method: "DELETE", path: `/api/sessions/${encodePathSegment(command.id)}` };
    case "send_message":
      return {
        method: "POST",
        path: `/api/sessions/${encodePathSegment(command.session_id)}/messages`,
        body: { text: command.text, attachments: command.attachments }
      };
    case "pivot_message":
      return {
        method: "POST",
        path: `/api/sessions/${encodePathSegment(command.session_id)}/pivot`,
        body: { text: command.text }
      };
    case "cancel_run":
      return {
        method: "POST",
        path: `/api/sessions/${encodePathSegment(command.session_id)}/cancel`
      };
    case "respond_interaction":
      return {
        method: "POST",
        path: `/api/sessions/${encodePathSegment(command.session_id)}/interactions/${encodePathSegment(command.request_id)}`,
        body: command.response
      };
    case "list_sources":
      return { method: "GET", path: "/api/sources" };
    case "probe_local_agents":
      return { method: "POST", path: "/api/sources/probe" };
    case "get_config":
      return { method: "GET", path: "/api/config" };
    case "update_config":
      return { method: "PUT", path: "/api/config", body: command.config };
    case "reload_config":
      return { method: "POST", path: "/api/config/reload" };
    case "apply_config":
      return { method: "POST", path: "/api/config/apply" };
  }
}

/** Web transport that sends commands through REST and receives events through fetch-based SSE. */
export class HttpSseTransport implements ITransport {
  readonly kind = "web" as const;

  private readonly baseUrl: string;
  private readonly fetchFn: FetchLike;
  private readonly token?: string | (() => string | undefined);

  /** Creates a transport for mag-web's REST + SSE API. */
  constructor(options: HttpSseTransportOptions = {}) {
    const fetchFn = options.fetch ?? globalThis.fetch?.bind(globalThis);
    if (fetchFn === undefined) {
      throw new TransportError("fetch is not available", { kind: "fetch_unavailable" });
    }

    this.baseUrl = trimTrailingSlashes(options.baseUrl ?? "");
    this.fetchFn = fetchFn;
    this.token = options.token;
  }

  /** Sends one Command by applying the REST route mapping table. */
  async send(command: Command): Promise<unknown> {
    const request = commandToRequest(command);
    const hasBody = request.body !== undefined;
    const response = await this.fetchFn(this.urlFor(request.path), {
      method: request.method,
      headers: this.headers(hasBody ? "application/json" : undefined),
      body: hasBody ? JSON.stringify(request.body) : undefined
    });

    if (!response.ok) {
      throw await transportErrorFromResponse(response);
    }

    if (response.status === 204) {
      return undefined;
    }

    const text = await response.text();
    return text.length === 0 ? undefined : (JSON.parse(text) as unknown);
  }

  /** Opens `/api/events` using fetch so Authorization headers can be sent. */
  subscribe(handler: EventHandler, options: SubscribeOptions = {}): () => void {
    const controller = new AbortController();
    let closed = false;
    const abortFromCaller = (): void => controller.abort(options.signal?.reason);

    if (options.signal?.aborted === true) {
      abortFromCaller();
    } else {
      options.signal?.addEventListener("abort", abortFromCaller, { once: true });
    }

    this.readEventStream(handler, controller.signal)
      .then(() => {
        if (!closed && !controller.signal.aborted) {
          options.onError?.(new TransportError("event stream closed", { kind: "stream_closed" }));
        }
      })
      .catch((error: unknown) => {
        if (!closed && !controller.signal.aborted && !isAbortError(error)) {
          options.onError?.(error);
        }
      });

    return () => {
      closed = true;
      options.signal?.removeEventListener("abort", abortFromCaller);
      controller.abort();
    };
  }

  private async readEventStream(handler: EventHandler, signal: AbortSignal): Promise<void> {
    const response = await this.fetchFn(this.urlFor("/api/events"), {
      method: "GET",
      headers: this.headers(undefined, "text/event-stream"),
      signal
    });

    if (!response.ok) {
      throw await transportErrorFromResponse(response);
    }

    if (response.body === null) {
      throw new TransportError("event stream response had no body", {
        kind: "stream_body_missing",
        status: response.status
      });
    }

    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    let buffer = "";
    let eventName: string | undefined;
    let dataLines: string[] = [];

    const dispatch = (): void => {
      if (dataLines.length === 0) {
        eventName = undefined;
        return;
      }

      const event = JSON.parse(dataLines.join("\n")) as Event;
      if (eventName !== undefined && event.type !== eventName) {
        throw new TransportError(
          `SSE event name ${eventName} did not match payload type ${event.type}`,
          {
            kind: "event_type_mismatch"
          }
        );
      }
      handler(event);
      eventName = undefined;
      dataLines = [];
    };

    const processLine = (line: string): void => {
      if (line.length === 0) {
        dispatch();
        return;
      }
      if (line.startsWith(":")) {
        return;
      }

      const separator = line.indexOf(":");
      const field = separator === -1 ? line : line.slice(0, separator);
      const value = separator === -1 ? "" : stripSingleLeadingSpace(line.slice(separator + 1));

      if (field === "event") {
        eventName = value;
      } else if (field === "data") {
        dataLines.push(value);
      }
    };

    for (;;) {
      const { done, value } = await reader.read();
      if (done) {
        break;
      }

      buffer += decoder.decode(value, { stream: true });
      buffer = processBufferedLines(buffer, processLine);
    }

    buffer += decoder.decode();
    if (buffer.length > 0) {
      processLine(stripTrailingCarriageReturn(buffer));
    }
    dispatch();
  }

  private headers(contentType?: string, accept?: string): Record<string, string> {
    const headers: Record<string, string> = {};
    const token = typeof this.token === "function" ? this.token() : this.token;
    if (token !== undefined && token.length > 0) {
      headers.Authorization = `Bearer ${token}`;
    }
    if (contentType !== undefined) {
      headers["Content-Type"] = contentType;
    }
    if (accept !== undefined) {
      headers.Accept = accept;
    }
    return headers;
  }

  private urlFor(path: string): string {
    return `${this.baseUrl}${path}`;
  }
}

async function transportErrorFromResponse(response: Response): Promise<TransportError> {
  const text = await response.text();
  const body = parseJsonOrUndefined(text);
  if (isRestErrorBody(body)) {
    return new TransportError(body.message, { kind: body.kind, status: response.status, body });
  }

  const message = response.statusText.length > 0 ? response.statusText : `HTTP ${response.status}`;
  return new TransportError(message, {
    kind: `http_${response.status}`,
    status: response.status,
    body
  });
}

function parseJsonOrUndefined(text: string): unknown {
  if (text.length === 0) {
    return undefined;
  }

  try {
    return JSON.parse(text) as unknown;
  } catch {
    return undefined;
  }
}

function isRestErrorBody(value: unknown): value is RestErrorBody {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const candidate = value as Partial<RestErrorBody>;
  return typeof candidate.kind === "string" && typeof candidate.message === "string";
}

function processBufferedLines(buffer: string, processLine: (line: string) => void): string {
  let remaining = buffer;
  for (;;) {
    const newline = remaining.indexOf("\n");
    if (newline === -1) {
      return remaining;
    }
    processLine(stripTrailingCarriageReturn(remaining.slice(0, newline)));
    remaining = remaining.slice(newline + 1);
  }
}

function stripTrailingCarriageReturn(value: string): string {
  return value.endsWith("\r") ? value.slice(0, -1) : value;
}

function stripSingleLeadingSpace(value: string): string {
  return value.startsWith(" ") ? value.slice(1) : value;
}

function trimTrailingSlashes(value: string): string {
  return value.replace(/\/+$/, "");
}

function encodePathSegment(value: string): string {
  return encodeURIComponent(value);
}

function isAbortError(error: unknown): boolean {
  return (
    (typeof DOMException !== "undefined" &&
      error instanceof DOMException &&
      error.name === "AbortError") ||
    (typeof error === "object" && error !== null && "name" in error && error.name === "AbortError")
  );
}

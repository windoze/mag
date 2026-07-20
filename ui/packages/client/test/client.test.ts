import type {
  Command,
  Event,
  HistoryEntry,
  InteractionResponseWire,
  SessionConfig,
  SessionId,
  SessionInfo
} from "@mag/protocol";
import { describe, expect, it, vi } from "vitest";

import historyJson from "./fixtures/session-history.json";
import eventsJson from "./fixtures/session-events.json";
import {
  HttpSseTransport,
  SessionStore,
  TransportError,
  clientPackageName,
  commandToRequest,
  type FetchLike,
  type ITransport,
  type SubscribeOptions
} from "../src/index";

const sessionId = "session-a";
const config = {
  provider: "anthropic",
  model: "claude-sonnet-4-5",
  routing: "model_routed"
} satisfies SessionConfig;
const sessionInfo = {
  id: sessionId,
  config,
  title: "Inspect README",
  last_active_at: 1721234567890,
  status: "idle"
} satisfies SessionInfo;
const historyFixture = historyJson as HistoryEntry[];
const eventFixture = eventsJson as Event[];

describe("@mag/client", () => {
  it("keeps the workspace package marker", () => {
    expect(clientPackageName).toBe("@mag/client");
  });

  it("maps every Command variant to the documented REST route and injects bearer auth", async () => {
    const requests: Array<{ url: string; init: RequestInit }> = [];
    const fetchMock: FetchLike = vi.fn(async (input, init) => {
      requests.push({ url: String(input), init: init ?? {} });
      return new Response(null, { status: 204 });
    });
    const transport = new HttpSseTransport({
      baseUrl: "http://127.0.0.1:3000/",
      token: "secret-token",
      fetch: fetchMock
    });
    const response = { kind: "answer", text: "ok" } satisfies InteractionResponseWire;
    const commands: Array<{ command: Command; method: string; path: string; body?: unknown }> = [
      { command: { type: "list_sessions" }, method: "GET", path: "/api/sessions" },
      {
        command: { type: "create_session", config },
        method: "POST",
        path: "/api/sessions",
        body: config
      },
      {
        command: { type: "resume_session", id: sessionId },
        method: "POST",
        path: `/api/sessions/${sessionId}/resume`
      },
      {
        command: { type: "get_session_history", id: sessionId },
        method: "GET",
        path: `/api/sessions/${sessionId}/history`
      },
      {
        command: { type: "delete_session", id: sessionId },
        method: "DELETE",
        path: `/api/sessions/${sessionId}`
      },
      {
        command: { type: "send_message", session_id: sessionId, text: "hello" },
        method: "POST",
        path: `/api/sessions/${sessionId}/messages`,
        body: { text: "hello" }
      },
      {
        command: { type: "pivot_message", session_id: sessionId, text: "pivot" },
        method: "POST",
        path: `/api/sessions/${sessionId}/pivot`,
        body: { text: "pivot" }
      },
      {
        command: { type: "cancel_run", session_id: sessionId },
        method: "POST",
        path: `/api/sessions/${sessionId}/cancel`
      },
      {
        command: {
          type: "respond_interaction",
          session_id: sessionId,
          request_id: "request-live",
          response
        },
        method: "POST",
        path: `/api/sessions/${sessionId}/interactions/request-live`,
        body: response
      },
      { command: { type: "list_sources" }, method: "GET", path: "/api/sources" },
      { command: { type: "probe_local_agents" }, method: "POST", path: "/api/sources/probe" },
      { command: { type: "get_config" }, method: "GET", path: "/api/config" },
      {
        command: {
          type: "update_config",
          config: { providers: {}, agents: {}, session: {}, approval: {} }
        },
        method: "PUT",
        path: "/api/config",
        body: { providers: {}, agents: {}, session: {}, approval: {} }
      },
      { command: { type: "reload_config" }, method: "POST", path: "/api/config/reload" },
      { command: { type: "apply_config" }, method: "POST", path: "/api/config/apply" }
    ];

    for (const item of commands) {
      expect(commandToRequest(item.command)).toMatchObject({
        method: item.method,
        path: item.path
      });
      await transport.send(item.command);
    }

    expect(requests).toHaveLength(commands.length);
    commands.forEach((item, index) => {
      const request = requests[index];
      expect(request.url).toBe(`http://127.0.0.1:3000${item.path}`);
      expect(request.init.method).toBe(item.method);
      expect(request.init.headers).toMatchObject({ Authorization: "Bearer secret-token" });
      if (item.body === undefined) {
        expect(request.init.body).toBeUndefined();
      } else {
        expect(request.init.headers).toMatchObject({ "Content-Type": "application/json" });
        expect(JSON.parse(request.init.body as string)).toEqual(item.body);
      }
    });
  });

  it("preserves typed REST errors for pivot fallback decisions", async () => {
    const fetchMock: FetchLike = vi.fn(
      async () =>
        new Response(JSON.stringify({ kind: "not_pivotable", message: "no active run" }), {
          status: 409,
          headers: { "Content-Type": "application/json" }
        })
    );
    const transport = new HttpSseTransport({ fetch: fetchMock });

    await expect(
      transport.send({ type: "pivot_message", session_id: sessionId, text: "try pivot" })
    ).rejects.toMatchObject({
      name: "TransportError",
      kind: "not_pivotable",
      status: 409,
      message: "no active run"
    });
  });

  it("parses fetch-based SSE frames and ignores heartbeat comments", async () => {
    const textDelta = eventFixture.find((event) => event.type === "text_delta")!;
    const body = new ReadableStream<Uint8Array>({
      start(controller) {
        controller.enqueue(
          new TextEncoder().encode(
            `: ping\n\nid: 1\nevent: text_delta\ndata: ${JSON.stringify(textDelta)}\n\n`
          )
        );
        controller.close();
      }
    });
    const fetchMock: FetchLike = vi.fn(async (input, init) => {
      expect(String(input)).toBe("/api/events");
      expect(init?.headers).toMatchObject({
        Authorization: "Bearer sse-token",
        Accept: "text/event-stream"
      });
      return new Response(body, { status: 200, headers: { "Content-Type": "text/event-stream" } });
    });
    const transport = new HttpSseTransport({ token: "sse-token", fetch: fetchMock });
    const received: Event[] = [];

    transport.subscribe((event) => received.push(event));

    await waitFor(() => received.length === 1);
    expect(received[0]).toEqual(textDelta);
  });

  it("merges history, event deltas, tool states, delegations, pending interactions, and pivot notices", async () => {
    const transport = new ScriptedTransport();
    const store = new SessionStore(transport);

    await store.openSession(sessionId);
    eventFixture.forEach((event) => store.applyEvent(event));

    let session = store.selectSession(sessionId)!;
    expect(session.messages.map((message) => [message.role, message.text])).toEqual([
      ["user", "Inspect README"],
      ["assistant", "The README is short."],
      ["assistant", "New answer"]
    ]);
    expect(session.toolCalls.map((tool) => [tool.id, tool.trace.status])).toEqual([
      ["tool-history", "finished"],
      ["tool-live", "finished"]
    ]);
    expect(
      session.delegations.find((delegation) => delegation.trace.delegate === "researcher")
        ?.messages[0]?.text
    ).toBe("context found");
    expect(session.pendingInteractions.map((interaction) => interaction.requestId)).toEqual([
      "request-live"
    ]);
    expect(session.run.state).toBe("awaiting_interaction");
    expect(session.pivotNotices.map((notice) => notice.status)).toEqual(["queued", "applied"]);

    await store.respondInteraction(sessionId, "request-live", { kind: "answer", text: "yes" });

    session = store.selectSession(sessionId)!;
    expect(session.pendingInteractions).toHaveLength(0);
    const interaction = session.thread.find((item) => item.type === "interaction")?.interaction;
    expect(interaction).toMatchObject({
      requestId: "request-live",
      status: "responded",
      response: { kind: "answer", text: "yes" }
    });
  });

  it("deduplicates history and incremental terminal tool traces by call id", () => {
    const store = new SessionStore(new ScriptedTransport());
    store.replaceHistory(sessionId, historyFixture);
    store.applyEvent({
      type: "tool_finished",
      id: sessionId,
      trace: {
        run_id: "run-history",
        call_id: "tool-history",
        name: "read_file",
        input: { path: "README.md" },
        output: { bytes: 10 },
        status: "finished"
      }
    });
    store.applyEvent({ type: "text_delta", id: sessionId, text: "Fresh delta" });

    const session = store.selectSession(sessionId)!;
    expect(session.toolCalls).toHaveLength(1);
    expect(session.messages.at(-1)).toMatchObject({
      role: "assistant",
      text: "Fresh delta",
      streaming: true
    });
  });

  it("automatically reconnects and aligns open sessions with list_sessions plus history", async () => {
    const transport = new ScriptedTransport();
    const store = new SessionStore(transport, { reconnectDelayMs: 0 });
    await store.openSession(sessionId);
    store.start();

    transport.sessions = [{ ...sessionInfo, title: "Aligned", status: "running" }];
    transport.histories.set(sessionId, [{ type: "user_message", text: "Aligned history" }]);
    transport.fail(new TransportError("lost stream", { kind: "stream_closed" }));

    await waitFor(
      () =>
        transport.subscribeCalls >= 2 &&
        store.selectSession(sessionId)?.messages[0]?.text === "Aligned history"
    );

    const snapshot = store.getSnapshot();
    expect(snapshot.connectionStatus).toBe("connected");
    expect(store.selectSession(sessionId)).toMatchObject({
      status: "running",
      info: { title: "Aligned" }
    });
    store.stop();
  });
});

class ScriptedTransport implements ITransport {
  readonly kind = "web" as const;
  readonly sent: Command[] = [];
  readonly histories = new Map<SessionId, HistoryEntry[]>([[sessionId, historyFixture]]);
  sessions: SessionInfo[] = [sessionInfo];
  subscribeCalls = 0;
  private handler: ((event: Event) => void) | undefined;
  private subscribeOptions: SubscribeOptions | undefined;

  async send(command: Command): Promise<unknown> {
    this.sent.push(command);
    switch (command.type) {
      case "list_sessions":
        return this.sessions;
      case "get_session_history":
        return this.histories.get(command.id) ?? [];
      case "create_session":
        return { id: sessionId, config: command.config };
      case "send_message":
        return { run_id: "run-local" };
      default:
        return undefined;
    }
  }

  subscribe(handler: (event: Event) => void, options?: SubscribeOptions): () => void {
    this.subscribeCalls += 1;
    this.handler = handler;
    this.subscribeOptions = options;
    return () => {
      if (this.handler === handler) {
        this.handler = undefined;
      }
    };
  }

  emit(event: Event): void {
    this.handler?.(event);
  }

  fail(error: unknown): void {
    this.subscribeOptions?.onError?.(error);
  }
}

async function waitFor(predicate: () => boolean): Promise<void> {
  const started = Date.now();
  for (;;) {
    if (predicate()) {
      return;
    }
    if (Date.now() - started > 1_000) {
      throw new Error("condition was not met before timeout");
    }
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
}

import type {
  Command,
  Event,
  HistoryEntry,
  InteractionResponseWire,
  SessionConfig,
  SessionId,
  SessionInfo,
  ToolStatusWire,
  ToolTrace
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
        ?.messages[0]?.message.text
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

  it("groups two-level delegation messages and origin-attributed interactions per delegate", () => {
    const store = new SessionStore(new ScriptedTransport());
    const events = [
      { type: "run_started", id: sessionId, run_id: "run-live" },
      {
        type: "delegation_started",
        id: sessionId,
        trace: { run_id: "run-live", delegate: "researcher", status: "started", task: "research" }
      },
      {
        type: "delegation_message",
        id: sessionId,
        message: { run_id: "run-live", delegate: "researcher", text: "scanning repo" }
      },
      {
        type: "interaction_requested",
        id: sessionId,
        request_id: "req-research",
        kind: { kind: "question", prompt: "Continue research?" },
        origin: { delegate: "researcher", depth: 1 }
      },
      {
        type: "delegation_started",
        id: sessionId,
        trace: { run_id: "run-live", delegate: "reviewer", status: "started", task: "review" }
      },
      {
        type: "delegation_message",
        id: sessionId,
        message: { run_id: "run-live", delegate: "reviewer", text: "reviewing diff" }
      },
      {
        type: "interaction_requested",
        id: sessionId,
        request_id: "req-review",
        kind: {
          kind: "approval",
          call_id: "tool-review",
          requirement: { type: "require_approval", reason: "nested approval" }
        },
        origin: { delegate: "reviewer", depth: 2 }
      },
      {
        type: "interaction_requested",
        id: sessionId,
        request_id: "req-root",
        kind: { kind: "question", prompt: "Root question?" },
        origin: { depth: 0 }
      },
      {
        type: "delegation_finished",
        id: sessionId,
        trace: {
          run_id: "run-live",
          delegate: "researcher",
          status: "finished",
          task: "research",
          output: "done",
          usage: { input_tokens: 10, output_tokens: 5, total_tokens: 15 }
        }
      }
    ] satisfies Event[];
    events.forEach((event) => store.applyEvent(event));

    const groups = store.selectDelegationGroups(sessionId);
    expect(groups.map((group) => group.delegate)).toEqual(["researcher", "reviewer"]);

    const researcher = groups[0];
    expect(researcher.depth).toBe(1);
    expect(researcher.delegations.map((delegation) => delegation.trace.status)).toEqual([
      "finished"
    ]);
    expect(researcher.delegations[0]?.trace.usage).toEqual({
      input_tokens: 10,
      output_tokens: 5,
      total_tokens: 15
    });
    expect(
      researcher.items.map((item) =>
        item.type === "message"
          ? `message:${item.text}`
          : `interaction:${item.interaction.requestId}`
      )
    ).toEqual(["message:scanning repo", "interaction:req-research"]);

    const reviewer = groups[1];
    expect(reviewer.depth).toBe(2);
    expect(
      reviewer.items.map((item) =>
        item.type === "message"
          ? `message:${item.text}`
          : `interaction:${item.interaction.requestId}`
      )
    ).toEqual(["message:reviewing diff", "interaction:req-review"]);

    // Root-originated interactions are never attributed to a delegate group.
    expect(
      groups.flatMap((group) =>
        group.items.flatMap((item) =>
          item.type === "interaction" ? [item.interaction.requestId] : []
        )
      )
    ).not.toContain("req-root");
  });

  it("synthesizes a group for a delegate that only surfaced through an interaction origin", () => {
    const store = new SessionStore(new ScriptedTransport());
    store.applyEvent({
      type: "interaction_requested",
      id: sessionId,
      request_id: "req-ghost",
      kind: { kind: "question", prompt: "Ghost?" },
      origin: { delegate: "ghost", depth: 3 }
    });

    const groups = store.selectDelegationGroups(sessionId);
    expect(groups).toHaveLength(1);
    expect(groups[0]).toMatchObject({ delegate: "ghost", depth: 3, delegations: [] });
    expect(groups[0]?.items).toHaveLength(1);
  });

  it("returns no delegation groups for unknown or delegation-free sessions", () => {
    const store = new SessionStore(new ScriptedTransport());
    expect(store.selectDelegationGroups("missing")).toEqual([]);
    store.applyEvent({ type: "run_started", id: sessionId, run_id: "run-live" });
    expect(store.selectDelegationGroups(sessionId)).toEqual([]);
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

  it("parses CRLF, multi-line data, and chunk-split SSE frames", async () => {
    const textDelta = eventFixture.find((event) => event.type === "text_delta")!;
    const payload = JSON.stringify(textDelta);
    // Split at a comma so the "\n" inserted between data lines stays valid
    // JSON whitespace.
    const splitAt = payload.indexOf(",");
    const frame = `id: 7\r\nevent: text_delta\r\ndata: ${payload.slice(0, splitAt)}\r\ndata: ${payload.slice(splitAt)}\r\n\r\n`;
    const bytes = new TextEncoder().encode(frame);
    const body = new ReadableStream<Uint8Array>({
      start(controller) {
        // Deliver the frame split across two chunks, mid-line.
        controller.enqueue(bytes.slice(0, 7));
        controller.enqueue(bytes.slice(7));
        controller.close();
      }
    });
    const fetchMock: FetchLike = vi.fn(
      async () =>
        new Response(body, { status: 200, headers: { "Content-Type": "text/event-stream" } })
    );
    const transport = new HttpSseTransport({ fetch: fetchMock });
    const received: Event[] = [];

    transport.subscribe((event) => received.push(event));

    await waitFor(() => received.length === 1);
    expect(received[0]).toEqual(textDelta);
  });

  it("buffers live events during a history refresh and replays them after the replace", async () => {
    const transport = new DeferredHistoryTransport();
    let releaseHistory!: () => void;
    transport.historyGate = new Promise((resolve) => {
      releaseHistory = resolve;
    });
    const store = new SessionStore(transport);
    store.replaceHistory(sessionId, historyFixture);

    const refresh = store.refreshHistory(sessionId);
    store.applyEvent({ type: "text_delta", id: sessionId, text: "In-flight delta" });
    store.applyEvent({ type: "config_changed", revision: 7 });

    // Session events stay buffered while the refresh is in flight (the
    // pending replace must not wipe them); store-global events still apply
    // immediately.
    expect(store.selectSession(sessionId)!.messages.map((message) => message.text)).not.toContain(
      "In-flight delta"
    );
    expect(store.getSnapshot().configRevision).toBe(7);

    releaseHistory();
    await refresh;

    const texts = store.selectSession(sessionId)!.messages.map((message) => message.text);
    expect(texts).toContain("Inspect README");
    expect(texts).toContain("In-flight delta");
  });

  it("queues pending interactions in arrival order and resolves them independently", async () => {
    const store = new SessionStore(new ScriptedTransport());
    await store.openSession(sessionId);
    store.applyEvent({ type: "run_started", id: sessionId, run_id: "run-queue" });
    store.applyEvent({
      type: "interaction_requested",
      id: sessionId,
      request_id: "req-first",
      kind: { kind: "question", prompt: "First?" },
      origin: { depth: 0 }
    });
    store.applyEvent({
      type: "interaction_requested",
      id: sessionId,
      request_id: "req-second",
      kind: { kind: "question", prompt: "Second?" },
      origin: { depth: 0 }
    });

    let session = store.selectSession(sessionId)!;
    expect(session.pendingInteractions.map((interaction) => interaction.requestId)).toEqual([
      "req-first",
      "req-second"
    ]);

    await store.respondInteraction(sessionId, "req-first", { kind: "answer", text: "one" });

    session = store.selectSession(sessionId)!;
    expect(session.pendingInteractions.map((interaction) => interaction.requestId)).toEqual([
      "req-second"
    ]);
    expect(session.run.state).toBe("awaiting_interaction");

    await store.respondInteraction(sessionId, "req-second", { kind: "answer", text: "two" });

    session = store.selectSession(sessionId)!;
    expect(session.pendingInteractions).toHaveLength(0);
    expect(session.run).toMatchObject({ state: "running", runId: "run-queue" });
    const interactions = session.thread.flatMap((item) =>
      item.type === "interaction" ? [[item.interaction.requestId, item.interaction.status]] : []
    );
    expect(interactions).toEqual([
      ["req-first", "responded"],
      ["req-second", "responded"]
    ]);
  });

  it("keeps streaming text deltas isolated per session", () => {
    const store = new SessionStore(new ScriptedTransport());

    store.applyEvent({ type: "text_delta", id: "session-a", text: "A1" });
    store.applyEvent({ type: "text_delta", id: "session-b", text: "B1" });
    store.applyEvent({ type: "text_delta", id: "session-a", text: "A2" });

    expect(store.selectSession("session-a")?.messages.map((message) => message.text)).toEqual([
      "A1A2"
    ]);
    expect(store.selectSession("session-b")?.messages.map((message) => message.text)).toEqual([
      "B1"
    ]);
  });

  it("tracks every terminal tool state and never downgrades a terminal card", () => {
    const store = new SessionStore(new ScriptedTransport());
    const trace = (callId: string, status: ToolStatusWire): ToolTrace => ({
      run_id: "run-tools",
      call_id: callId,
      name: "shell",
      status
    });

    store.applyEvent({
      type: "tool_finished",
      id: sessionId,
      trace: trace("t-finished", "finished")
    });
    store.applyEvent({ type: "tool_finished", id: sessionId, trace: trace("t-denied", "denied") });
    store.applyEvent({
      type: "tool_finished",
      id: sessionId,
      trace: trace("t-cancelled", "cancelled")
    });
    store.applyEvent({ type: "tool_finished", id: sessionId, trace: trace("t-failed", "failed") });
    // A late started echo must not downgrade the terminal card.
    store.applyEvent({ type: "tool_started", id: sessionId, trace: trace("t-failed", "started") });

    const statuses = store
      .selectSession(sessionId)!
      .toolCalls.map((tool) => [tool.id, tool.trace.status]);
    expect(statuses).toEqual([
      ["t-finished", "finished"],
      ["t-denied", "denied"],
      ["t-cancelled", "cancelled"],
      ["t-failed", "failed"]
    ]);
  });

  it("never lists a session twice after events race with list_sessions", async () => {
    const transport = new ScriptedTransport();
    const store = new SessionStore(transport);

    store.applyEvent({ type: "session_created", id: "session-new", config });
    store.applyEvent({ type: "text_delta", id: "session-new", text: "hello" });
    transport.sessions = [sessionInfo, { ...sessionInfo, id: "session-new" }];
    await store.refreshSessions();

    const ids = store.selectSessions().map((session) => session.id);
    expect(ids.filter((id) => id === "session-new")).toHaveLength(1);
    expect(new Set(ids).size).toBe(ids.length);
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

class DeferredHistoryTransport extends ScriptedTransport {
  historyGate: Promise<void> = Promise.resolve();

  override async send(command: Command): Promise<unknown> {
    if (command.type === "get_session_history") {
      await this.historyGate;
    }
    return super.send(command);
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

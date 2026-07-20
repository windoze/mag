import type { ITransport } from "@mag/client";
import { SessionStore, TransportError } from "@mag/client";
import * as React from "react";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import { App } from "./App";
import { captureFragmentToken } from "./token";

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

const sessionId = "session-a";
const config = {
  provider: "openai",
  model: "gpt-5-codex",
  routing: "model_routed"
};

type TransportCommand = Parameters<ITransport["send"]>[0];
type TransportEvent = Parameters<Parameters<ITransport["subscribe"]>[0]>[0];
type TransportSubscribeOptions = Parameters<ITransport["subscribe"]>[1];

describe("App", () => {
  afterEach(() => {
    vi.restoreAllMocks();
    document.body.replaceChildren();
  });

  it("captures fragment tokens into session storage and removes them from the URL", () => {
    const storage = new MemoryStorage();
    const replaceState = vi.fn();

    const token = captureFragmentToken(
      storage,
      { hash: "#t=secret-token&pane=config", pathname: "/", search: "?debug=1" },
      { replaceState }
    );

    expect(token).toBe("secret-token");
    expect(storage.getItem("mag.web.token")).toBe("secret-token");
    expect(replaceState).toHaveBeenCalledWith(null, "", "/?debug=1#pane=config");
  });

  it("wires sessions, history, interaction responses, pivot fallback, cancel, create, and delete", async () => {
    const transport = new ScriptedTransport();
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(true);
    const rendered = await renderApp(<App transport={transport} storage={new MemoryStorage()} />);

    await waitFor(() => rendered.container.textContent?.includes("Inspect README") === true);
    expect(transport.subscribeCalls).toBe(1);
    expect(transport.sent.map((command) => command.type)).toContain("list_sessions");

    await click(getButton(rendered.container, "Inspect README"));
    await waitFor(() => rendered.container.textContent?.includes("The README is short.") === true);
    expect(transport.sent.map((command) => command.type)).toEqual(
      expect.arrayContaining(["resume_session", "get_session_history"])
    );

    await act(async () => {
      transport.emit({ type: "run_started", id: sessionId, run_id: "run-live" });
      transport.emit({ type: "text_delta", id: sessionId, text: "Working" });
      transport.emit({
        type: "tool_started",
        id: sessionId,
        trace: {
          run_id: "run-live",
          call_id: "tool-live",
          name: "shell",
          input: { cmd: "pwd" },
          status: "started"
        }
      });
      transport.emit({
        type: "interaction_requested",
        id: sessionId,
        request_id: "request-live",
        kind: {
          kind: "approval",
          call_id: "tool-live",
          requirement: { type: "require_approval", reason: "needs approval" }
        },
        origin: { delegate: "researcher", depth: 1 }
      });
    });

    await waitFor(() => rendered.container.textContent?.includes("Approve tool call") === true);
    expect(rendered.container.textContent).toContain("shell");

    await click(getButton(rendered.container, "Approve"));
    expect(transport.sent.at(-1)).toMatchObject({
      type: "respond_interaction",
      session_id: sessionId,
      request_id: "request-live",
      response: {
        kind: "approval",
        step_id: "tool-live",
        call_id: "tool-live",
        decision: "approve"
      }
    });

    await click(getButton(rendered.container, "Cancel"));
    expect(transport.sent.at(-1)).toMatchObject({ type: "cancel_run", session_id: sessionId });

    transport.failPivot = true;
    await change(getComposer(rendered.container), "fall back to a new message");
    await click(getButton(rendered.container, "Insert pivot..."));
    expect(transport.sent.slice(-3).map((command) => command.type)).toEqual([
      "pivot_message",
      "send_message",
      "list_sessions"
    ]);

    await click(getButton(rendered.container, "New chat"));
    await waitFor(() => transport.sent.some((command) => command.type === "create_session"));
    expect(transport.sent.find((command) => command.type === "create_session")).toMatchObject({
      config: { provider: "openai", model: "gpt-5-codex", routing: "model_routed" }
    });

    await click(getByLabel(rendered.container, "Delete New shell session"));
    expect(confirm).toHaveBeenCalled();
    expect(transport.sent.at(-1)).toMatchObject({ type: "delete_session", id: "session-new" });

    await act(async () => rendered.root.unmount());
  });

  it("reconnects after a stream failure and realigns without duplicate bubbles", async () => {
    const transport = new ScriptedTransport();
    // Zero reconnect delay keeps the store's reconnect timer on real timers.
    const store = new SessionStore(transport, { reconnectDelayMs: 0 });
    const rendered = await renderApp(<App storage={new MemoryStorage()} store={store} />);

    await waitFor(() => rendered.container.textContent?.includes("Inspect README") === true);
    await click(getButton(rendered.container, "Inspect README"));
    await waitFor(() => rendered.container.textContent?.includes("The README is short.") === true);

    // A partial streaming turn arrives before the SSE stream drops.
    await act(async () => {
      transport.emit({ type: "run_started", id: sessionId, run_id: "run-live" });
      transport.emit({ type: "text_delta", id: sessionId, text: "Working" });
    });
    await waitFor(() => rendered.container.textContent?.includes("Working") === true);

    transport.sent.length = 0;
    await act(async () => {
      transport.fail(new TransportError("lost stream", { kind: "stream_closed" }));
    });

    // The store realigns with a full list_sessions + history refresh, then
    // resubscribes to the event stream.
    await waitFor(() => transport.subscribeCalls >= 2);
    expect(transport.sent.map((command) => command.type)).toEqual(
      expect.arrayContaining(["list_sessions", "get_session_history"])
    );
    await waitFor(() => rendered.container.textContent?.includes("The README is short.") === true);

    // Authoritative history replaces the optimistic stream: no bubble renders
    // twice and the lost delta does not linger.
    const bubbles = [...rendered.container.querySelectorAll("article")].map(
      (bubble) => bubble.textContent ?? ""
    );
    expect(bubbles.filter((text) => text.includes("Inspect README"))).toHaveLength(1);
    expect(bubbles.filter((text) => text.includes("The README is short."))).toHaveLength(1);
    expect(bubbles.filter((text) => text.includes("Working"))).toHaveLength(0);

    await act(async () => rendered.root.unmount());
  });

  it("opens the delegate sub-thread in the right rail and collapses the rail", async () => {
    const transport = new ScriptedTransport();
    const rendered = await renderApp(<App transport={transport} storage={new MemoryStorage()} />);

    await waitFor(() => rendered.container.textContent?.includes("Inspect README") === true);
    await click(getButton(rendered.container, "Inspect README"));
    await waitFor(() => rendered.container.textContent?.includes("The README is short.") === true);

    await act(async () => {
      transport.emit({ type: "run_started", id: sessionId, run_id: "run-live" });
      transport.emit({
        type: "delegation_started",
        id: sessionId,
        trace: { run_id: "run-live", delegate: "researcher", status: "started", task: "research" }
      });
      transport.emit({
        type: "delegation_message",
        id: sessionId,
        message: { run_id: "run-live", delegate: "researcher", text: "scanning repo" }
      });
      transport.emit({
        type: "interaction_requested",
        id: sessionId,
        request_id: "req-research",
        kind: { kind: "question", prompt: "Continue research?" },
        origin: { delegate: "researcher", depth: 1 }
      });
    });

    // The inline delegation card opens the drill-down panel in the right rail.
    await click(getButton(rendered.container, "researcher"));
    await waitFor(
      () =>
        rendered.container.querySelector('[aria-label="Delegate researcher sub-thread"]') !== null
    );
    const panel = rendered.container.querySelector(
      '[aria-label="Delegate researcher sub-thread"]'
    ) as HTMLElement;
    expect(panel.textContent).toContain("scanning repo");
    expect(panel.textContent).toContain("Continue research?");
    expect(panel.textContent).toContain("[from researcher@depth1]");

    // Responding inside the panel goes through the same interaction channel.
    await change(getComposer(panel), "go ahead");
    await click(getButton(panel, "Submit answer"));
    expect(transport.sent.at(-1)).toMatchObject({
      type: "respond_interaction",
      session_id: sessionId,
      request_id: "req-research",
      response: { kind: "answer", text: "go ahead" }
    });

    // The panel close button returns to the delegates list.
    await click(getByLabel(rendered.container, "Close researcher sub-thread"));
    expect(
      rendered.container.querySelector('[aria-label="Delegate researcher sub-thread"]')
    ).toBeNull();

    // The header toggle collapses and re-expands the whole right rail.
    const rail = getByLabel(rendered.container, "Session progress rail");
    expect(rail.className).toContain("xl:flex");
    await click(getByLabel(rendered.container, "Collapse right rail"));
    expect(rail.className).not.toContain("xl:flex");
    await click(getByLabel(rendered.container, "Expand right rail"));
    expect(rail.className).toContain("xl:flex");

    await act(async () => rendered.root.unmount());
  });

  it("lists globally running sessions in the right rail and jumps across sessions", async () => {
    const transport = new ScriptedTransport();
    const rendered = await renderApp(<App transport={transport} storage={new MemoryStorage()} />);

    await waitFor(() => rendered.container.textContent?.includes("Inspect README") === true);
    await click(getButton(rendered.container, "Inspect README"));
    await waitFor(() => rendered.container.textContent?.includes("The README is short.") === true);

    await act(async () => {
      transport.emit({ type: "run_started", id: sessionId, run_id: "run-a" });
      transport.emit({ type: "run_started", id: "session-new", run_id: "run-b" });
    });

    const rail = getByLabel(rendered.container, "Session progress rail");
    await waitFor(() => rail.textContent?.includes("Running run-b") === true);
    expect(rail.textContent).toContain("Running sessions");
    expect(rail.textContent).toContain("New shell session");

    // Clicking a running session in the rail jumps to it (resume + history).
    await click(getButton(rail, "New shell session"));
    await waitFor(() =>
      transport.sent.some(
        (command) => command.type === "resume_session" && command.id === "session-new"
      )
    );
    expect(
      transport.sent.some(
        (command) => command.type === "get_session_history" && command.id === "session-new"
      )
    ).toBe(true);

    // A finished run leaves the global running list.
    transport.sent.length = 0;
    await act(async () => {
      transport.emit({ type: "run_finished", id: "session-new", output: { text: "done" } });
    });
    await waitFor(() => rail.textContent?.includes("Running run-b") === false);
    expect(rail.textContent).toContain("Running run-a");

    await act(async () => rendered.root.unmount());
  });

  it("routes Sources and Config pages from the sidebar", async () => {
    const transport = new ScriptedTransport();
    const rendered = await renderApp(<App transport={transport} storage={new MemoryStorage()} />);

    await waitFor(() => rendered.container.textContent?.includes("Inspect README") === true);

    await click(getButton(rendered.container, "Sources"));
    await waitFor(() => transport.sent.some((command) => command.type === "list_sources"));
    await waitFor(() => rendered.container.textContent?.includes("Claude Code") === true);

    await click(getButton(rendered.container, "Config"));
    await waitFor(() => transport.sent.some((command) => command.type === "get_config"));
    await waitFor(
      () =>
        rendered.container.querySelector("textarea")?.value.includes("[providers.anthropic]") ===
        true
    );

    await act(async () => rendered.root.unmount());
  });

  it("loads, edits, saves, reloads, and applies the config from the Config page", async () => {
    const transport = new ScriptedTransport();
    const rendered = await renderApp(<App transport={transport} storage={new MemoryStorage()} />);

    await waitFor(() => rendered.container.textContent?.includes("Inspect README") === true);
    await click(getButton(rendered.container, "Config"));
    await waitFor(
      () =>
        rendered.container.querySelector("textarea")?.value.includes("[providers.anthropic]") ===
        true
    );
    expect(transport.sent.map((command) => command.type)).toContain("get_config");

    const textarea = rendered.container.querySelector("textarea") as HTMLTextAreaElement;
    await change(textarea, '[session]\nrouting = "model_routed"\n');
    await click(getButton(rendered.container, "Save"));
    await waitFor(() => transport.sent.some((command) => command.type === "update_config"));
    expect(transport.sent.find((command) => command.type === "update_config")).toMatchObject({
      type: "update_config",
      config: { session: { routing: "model_routed" } }
    });

    transport.sent.length = 0;
    await click(getButton(rendered.container, "Reload"));
    await waitFor(() => transport.sent.some((command) => command.type === "get_config"));
    expect(transport.sent.map((command) => command.type)).toEqual(["reload_config", "get_config"]);

    await click(getButton(rendered.container, "Apply"));
    expect(transport.sent.at(-1)).toMatchObject({ type: "apply_config" });

    await act(async () => rendered.root.unmount());
  });

  it("renders the sources table and probes local agents", async () => {
    const transport = new ScriptedTransport();
    const rendered = await renderApp(<App transport={transport} storage={new MemoryStorage()} />);

    await waitFor(() => rendered.container.textContent?.includes("Inspect README") === true);
    await click(getButton(rendered.container, "Sources"));
    await waitFor(() => rendered.container.textContent?.includes("Claude Code") === true);
    expect(rendered.container.textContent).toContain("unavailable");

    await click(getButton(rendered.container, "Probe"));
    await waitFor(() => transport.sent.some((command) => command.type === "probe_local_agents"));
    await waitFor(() => rendered.container.textContent?.includes("Gemini CLI") === true);
    // Probing only covers local agents: provider rows must survive.
    expect(rendered.container.textContent).toContain("anthropic");
    expect(rendered.container.textContent).not.toContain("Claude Code");

    await act(async () => rendered.root.unmount());
  });

  it("surfaces pivot failures other than not_pivotable instead of falling back", async () => {
    const transport = new ScriptedTransport();
    const rendered = await renderApp(<App transport={transport} storage={new MemoryStorage()} />);

    await waitFor(() => rendered.container.textContent?.includes("Inspect README") === true);
    await click(getButton(rendered.container, "Inspect README"));
    await waitFor(() => rendered.container.textContent?.includes("The README is short.") === true);

    await act(async () => {
      transport.emit({ type: "run_started", id: sessionId, run_id: "run-live" });
    });
    transport.sent.length = 0;
    transport.pivotError = new TransportError("engine exploded", { kind: "backend", status: 500 });

    await change(getComposer(rendered.container), "try a pivot");
    await click(getButton(rendered.container, "Insert pivot..."));

    await waitFor(() => rendered.container.textContent?.includes("engine exploded") === true);
    // A non-409/not_pivotable failure must not silently fall back to a new run.
    expect(transport.sent.map((command) => command.type)).toEqual(["pivot_message"]);

    await act(async () => rendered.root.unmount());
  });

  it("closes the delegate sub-thread when switching sessions", async () => {
    const transport = new ScriptedTransport();
    const rendered = await renderApp(<App transport={transport} storage={new MemoryStorage()} />);

    await waitFor(() => rendered.container.textContent?.includes("Inspect README") === true);
    await click(getButton(rendered.container, "Inspect README"));
    await waitFor(() => rendered.container.textContent?.includes("The README is short.") === true);

    await act(async () => {
      transport.emit({
        type: "delegation_started",
        id: sessionId,
        trace: { run_id: "run-live", delegate: "researcher", status: "started", task: "research" }
      });
    });
    await click(getButton(rendered.container, "researcher"));
    await waitFor(
      () =>
        rendered.container.querySelector('[aria-label="Delegate researcher sub-thread"]') !== null
    );

    // Switching to another session clears the drill-down selection.
    await click(getButton(rendered.container, "New shell session"));
    await waitFor(
      () =>
        rendered.container.querySelector('[aria-label="Delegate researcher sub-thread"]') === null
    );

    await act(async () => rendered.root.unmount());
  });

  it("keeps composer drafts per session across navigation", async () => {
    const transport = new ScriptedTransport();
    const rendered = await renderApp(<App transport={transport} storage={new MemoryStorage()} />);

    await waitFor(() => rendered.container.textContent?.includes("Inspect README") === true);
    await click(getButton(rendered.container, "Inspect README"));
    await waitFor(() => rendered.container.textContent?.includes("The README is short.") === true);

    await change(getComposer(rendered.container), "draft for A");

    // A draft typed for session A must not leak into session B's composer.
    await click(getButton(rendered.container, "New shell session"));
    await waitFor(() => getComposer(rendered.container).value === "");
    await change(getComposer(rendered.container), "draft for B");

    await click(getButton(rendered.container, "Inspect README"));
    await waitFor(() => getComposer(rendered.container).value === "draft for A");

    await act(async () => rendered.root.unmount());
  });

  it("confirms before reloading the config with unsaved edits", async () => {
    const transport = new ScriptedTransport();
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(false);
    const rendered = await renderApp(<App transport={transport} storage={new MemoryStorage()} />);

    await waitFor(() => rendered.container.textContent?.includes("Inspect README") === true);
    await click(getButton(rendered.container, "Config"));
    await waitFor(
      () =>
        rendered.container.querySelector("textarea")?.value.includes("[providers.anthropic]") ===
        true
    );

    // Dirty editor + declined confirmation: no reload command is sent.
    const textarea = rendered.container.querySelector("textarea") as HTMLTextAreaElement;
    await change(textarea, '[session]\nrouting = "dispatcher"\n');
    transport.sent.length = 0;
    await click(getButton(rendered.container, "Reload"));
    expect(confirm).toHaveBeenCalled();
    expect(transport.sent.map((command) => command.type)).toEqual([]);

    // Accepted confirmation: reload proceeds and refetches the text.
    confirm.mockReturnValue(true);
    await click(getButton(rendered.container, "Reload"));
    await waitFor(() => transport.sent.some((command) => command.type === "get_config"));
    expect(transport.sent.map((command) => command.type)).toEqual(["reload_config", "get_config"]);

    await act(async () => rendered.root.unmount());
  });
});

class ScriptedTransport implements ITransport {
  readonly kind = "web" as const;
  readonly sent: TransportCommand[] = [];
  failPivot = false;
  pivotError: TransportError | undefined;
  subscribeCalls = 0;
  private handler: ((event: TransportEvent) => void) | undefined;
  private subscribeOptions: TransportSubscribeOptions;

  async send(command: TransportCommand): Promise<unknown> {
    this.sent.push(command);
    switch (command.type) {
      case "list_sessions":
        return [
          {
            id: sessionId,
            config,
            title: "Inspect README",
            last_active_at: 1721234567890,
            status: "idle"
          },
          {
            id: "session-new",
            config,
            title: "New shell session",
            last_active_at: 1721234567999,
            status: "idle"
          }
        ];
      case "get_session_history":
        return command.id === sessionId
          ? [
              { type: "user_message", text: "Inspect README", attachments: [] },
              { type: "assistant_message", text: "The README is short." }
            ]
          : [];
      case "create_session":
        return { id: "session-new", config: command.config };
      case "get_config":
        return {
          providers: {
            anthropic: { wire: "anthropic", api_key: { env: "ANTHROPIC_API_KEY" } }
          },
          session: { routing: "model_routed" }
        };
      case "list_sources":
        return [
          {
            id: "anthropic",
            name: "anthropic",
            kind: "llm_provider",
            available: true,
            capabilities: ["anthropic"]
          },
          {
            id: "claude-code",
            name: "Claude Code",
            kind: "local_agent",
            available: true,
            version: "1.2.3",
            capabilities: ["delegate"]
          },
          {
            id: "ollama",
            name: "Ollama",
            kind: "local_agent",
            available: false,
            capabilities: []
          }
        ];
      case "probe_local_agents":
        return [
          {
            id: "gemini-cli",
            name: "Gemini CLI",
            kind: "local_agent",
            available: true,
            version: "0.1.0",
            capabilities: ["delegate"]
          }
        ];
      case "send_message":
        return { run_id: "run-fallback" };
      case "pivot_message":
        if (this.pivotError !== undefined) {
          throw this.pivotError;
        }
        if (this.failPivot) {
          throw new TransportError("no active run", { kind: "not_pivotable", status: 409 });
        }
        return undefined;
      default:
        return undefined;
    }
  }

  subscribe(
    handler: (event: TransportEvent) => void,
    options?: TransportSubscribeOptions
  ): () => void {
    this.subscribeCalls += 1;
    this.handler = handler;
    this.subscribeOptions = options;
    return () => {
      if (this.handler === handler) {
        this.handler = undefined;
      }
    };
  }

  emit(event: TransportEvent): void {
    this.handler?.(event);
  }

  fail(error: unknown): void {
    this.subscribeOptions?.onError?.(error);
  }
}

class MemoryStorage {
  private readonly values = new Map<string, string>();

  getItem(key: string): string | null {
    return this.values.get(key) ?? null;
  }

  setItem(key: string, value: string): void {
    this.values.set(key, value);
  }

  removeItem(key: string): void {
    this.values.delete(key);
  }
}

async function renderApp(element: React.ReactElement): Promise<{
  readonly container: HTMLElement;
  readonly root: Root;
}> {
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  await act(async () => root.render(element));
  return { container, root };
}

function getButton(container: HTMLElement, text: string): HTMLButtonElement {
  const button = [...container.querySelectorAll("button")].find(
    (candidate) => candidate.textContent?.includes(text) === true
  );
  if (!(button instanceof HTMLButtonElement)) {
    throw new Error(`button ${text} not found`);
  }
  return button;
}

function getByLabel(container: HTMLElement, label: string): HTMLElement {
  const element = container.querySelector(`[aria-label="${label}"]`);
  if (!(element instanceof HTMLElement)) {
    throw new Error(`element ${label} not found`);
  }
  return element;
}

function getComposer(container: HTMLElement): HTMLTextAreaElement {
  const textareas = [...container.querySelectorAll("textarea")];
  const textarea = textareas.at(-1);
  if (!(textarea instanceof HTMLTextAreaElement)) {
    throw new Error("composer textarea not found");
  }
  return textarea;
}

async function click(element: HTMLElement): Promise<void> {
  await act(async () => {
    element.click();
  });
}

async function change(element: HTMLTextAreaElement, value: string): Promise<void> {
  await act(async () => {
    const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set;
    setter?.call(element, value);
    element.dispatchEvent(new Event("input", { bubbles: true }));
    element.dispatchEvent(new Event("change", { bubbles: true }));
  });
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

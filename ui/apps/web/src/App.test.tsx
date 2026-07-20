import type { ITransport } from "@mag/client";
import { TransportError } from "@mag/client";
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

  it("routes Sources and Config placeholders from the sidebar", async () => {
    const rendered = await renderApp(
      <App transport={new ScriptedTransport()} storage={new MemoryStorage()} />
    );

    await waitFor(() => rendered.container.textContent?.includes("Inspect README") === true);
    await click(getButton(rendered.container, "Sources"));
    expect(rendered.container.textContent).toContain("Sources management lands in W4");

    await click(getButton(rendered.container, "Config"));
    expect(rendered.container.textContent).toContain("The text ConfigEditor lands in W4");

    await act(async () => rendered.root.unmount());
  });
});

class ScriptedTransport implements ITransport {
  readonly kind = "web" as const;
  readonly sent: TransportCommand[] = [];
  failPivot = false;
  subscribeCalls = 0;
  private handler: ((event: TransportEvent) => void) | undefined;

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
      case "send_message":
        return { run_id: "run-fallback" };
      case "pivot_message":
        if (this.failPivot) {
          throw new TransportError("no active run", { kind: "not_pivotable", status: 409 });
        }
        return undefined;
      default:
        return undefined;
    }
  }

  subscribe(handler: (event: TransportEvent) => void): () => void {
    this.subscribeCalls += 1;
    this.handler = handler;
    return () => {
      if (this.handler === handler) {
        this.handler = undefined;
      }
    };
  }

  emit(event: TransportEvent): void {
    this.handler?.(event);
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

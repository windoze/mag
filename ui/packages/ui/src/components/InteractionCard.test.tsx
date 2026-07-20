import { act, type ReactElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import { InteractionCard } from "./InteractionCard";

(
  globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const mountedRoots: Array<{ readonly root: Root; readonly host: HTMLDivElement }> = [];

describe("InteractionCard", () => {
  afterEach(() => {
    mountedRoots.splice(0).forEach(({ host, root }) => {
      act(() => root.unmount());
      host.remove();
    });
  });

  it("submits approval responses with call_id as the placeholder step_id", () => {
    const onSubmit = vi.fn();
    const host = render(
      <InteractionCard
        kind={{
          kind: "approval",
          call_id: "11111111-1111-1111-1111-111111111111",
          requirement: { type: "require_approval", reason: "shell command" },
          tool_name: "shell"
        }}
        requestId="req-approval"
        onSubmit={onSubmit}
      />
    );

    click(buttonByText(host, "Approve"));

    expect(onSubmit).toHaveBeenCalledWith(
      {
        kind: "approval",
        step_id: "11111111-1111-1111-1111-111111111111",
        call_id: "11111111-1111-1111-1111-111111111111",
        decision: "approve",
        message: undefined
      },
      "req-approval"
    );
  });

  it("submits question answer text", () => {
    const onSubmit = vi.fn();
    const host = render(
      <InteractionCard
        kind={{ kind: "question", prompt: "Which branch should I use?" }}
        requestId="req-question"
        onSubmit={onSubmit}
      />
    );

    changeText(requiredElement(host, "textarea"), "main");
    click(buttonByText(host, "Submit answer"));

    expect(onSubmit).toHaveBeenCalledWith({ kind: "answer", text: "main" }, "req-question");
  });

  it("submits zero-based choice indexes", () => {
    const onSubmit = vi.fn();
    const host = render(
      <InteractionCard
        kind={{ kind: "choice", prompt: "Pick a target", options: ["web", "desktop", "cli"] }}
        requestId="req-choice"
        onSubmit={onSubmit}
      />
    );

    click(buttonByText(host, "desktop"));

    expect(onSubmit).toHaveBeenCalledWith({ kind: "choice", index: 1 }, "req-choice");
  });

  it("submits permission decisions with optional denial reasons", () => {
    const onSubmit = vi.fn();
    const host = render(
      <InteractionCard
        kind={{
          kind: "permission",
          action_id: "perm-1",
          actor: "mag",
          category: "network",
          risk: "high",
          summary: "Open network connection",
          subject: { host: "example.test" }
        }}
        requestId="req-permission"
        onSubmit={onSubmit}
      />
    );

    changeText(requiredElement(host, "input"), "offline test");
    click(buttonByText(host, "Deny"));

    expect(onSubmit).toHaveBeenCalledWith(
      {
        kind: "permission",
        action_id: "perm-1",
        decision: { type: "deny", reason: "offline test" }
      },
      "req-permission"
    );
  });
});

function render(element: ReactElement): HTMLDivElement {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  mountedRoots.push({ host, root });
  act(() => root.render(element));
  return host;
}

function click(element: HTMLElement): void {
  act(() => {
    element.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  });
}

function changeText(element: Element, value: string): void {
  if (!(element instanceof HTMLInputElement || element instanceof HTMLTextAreaElement)) {
    throw new Error("expected text input element");
  }
  act(() => {
    const valueSetter = Object.getOwnPropertyDescriptor(
      element.constructor.prototype,
      "value"
    )?.set;
    valueSetter?.call(element, value);
    element.dispatchEvent(new Event("input", { bubbles: true }));
    element.dispatchEvent(new Event("change", { bubbles: true }));
  });
}

function buttonByText(host: HTMLElement, text: string): HTMLButtonElement {
  const button = [...host.querySelectorAll("button")].find((candidate) =>
    candidate.textContent?.includes(text)
  );
  if (button === undefined) {
    throw new Error(`missing button: ${text}`);
  }
  return button;
}

function requiredElement(host: HTMLElement, selector: string): Element {
  const element = host.querySelector(selector);
  if (element === null) {
    throw new Error(`missing element: ${selector}`);
  }
  return element;
}

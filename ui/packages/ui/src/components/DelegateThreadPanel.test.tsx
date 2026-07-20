import { act, type ReactElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import { DelegateThreadPanel } from "./DelegateThreadPanel";

(
  globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const mountedRoots: Array<{ readonly root: Root; readonly host: HTMLDivElement }> = [];

describe("DelegateThreadPanel", () => {
  afterEach(() => {
    mountedRoots.splice(0).forEach(({ host, root }) => {
      act(() => root.unmount());
      host.remove();
    });
  });

  it("renders the delegate header with status, usage, and depth badge", () => {
    const host = render(
      <DelegateThreadPanel
        delegate="researcher"
        depth={1}
        items={[]}
        status="finished"
        usage={{ input_tokens: 10, output_tokens: 5, total_tokens: 15 }}
      />
    );

    expect(host.textContent).toContain("researcher");
    expect(host.textContent).toContain("finished");
    expect(host.textContent).toContain("15 tokens");
    expect(host.textContent).toContain("[from researcher@depth1]");
    expect(host.textContent).toContain("No delegated activity yet.");
  });

  it("renders messages and origin-tagged interaction cards in arrival order", () => {
    const host = render(
      <DelegateThreadPanel
        delegate="reviewer"
        depth={2}
        items={[
          { type: "message", message: { id: "m1", text: "Reviewing the diff." } },
          {
            type: "interaction",
            interaction: {
              requestId: "req-review",
              kind: { kind: "question", prompt: "Approve the plan?" },
              origin: { delegate: "reviewer", depth: 2 },
              status: "pending"
            }
          }
        ]}
      />
    );

    expect(host.textContent).toContain("Reviewing the diff.");
    expect(host.textContent).toContain("Approve the plan?");
    expect(host.textContent).toContain("[from reviewer@depth2]");
    const reviewText = host.textContent ?? "";
    expect(reviewText.indexOf("Reviewing the diff.")).toBeLessThan(
      reviewText.indexOf("Approve the plan?")
    );
  });

  it("forwards interaction submissions and close clicks", () => {
    const onClose = vi.fn();
    const onRespond = vi.fn();
    const host = render(
      <DelegateThreadPanel
        delegate="researcher"
        depth={1}
        items={[
          {
            type: "interaction",
            interaction: {
              requestId: "req-research",
              kind: { kind: "question", prompt: "Continue?" },
              origin: { delegate: "researcher", depth: 1 },
              status: "pending"
            }
          }
        ]}
        onClose={onClose}
        onRespondInteraction={onRespond}
      />
    );

    changeText(requiredElement(host, "textarea"), "go ahead");
    click(buttonByText(host, "Submit answer"));
    expect(onRespond).toHaveBeenCalledWith("req-research", { kind: "answer", text: "go ahead" });

    click(requiredElement(host, '[aria-label="Close researcher sub-thread"]') as HTMLElement);
    expect(onClose).toHaveBeenCalledTimes(1);
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

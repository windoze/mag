import { act, type ReactElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import { Composer } from "./Composer";

(
  globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const mountedRoots: Array<{ readonly root: Root; readonly host: HTMLDivElement }> = [];

describe("Composer", () => {
  afterEach(() => {
    mountedRoots.splice(0).forEach(({ host, root }) => {
      act(() => root.unmount());
      host.remove();
    });
  });

  it("shows send copy without a cancel button while idle", () => {
    const host = render(
      <Composer value="hello" onSend={() => undefined} onValueChange={() => undefined} />
    );

    expect(buttonByText(host, "Send")).toBeDefined();
    expect(hasButton(host, "Cancel")).toBe(false);
    expect(host.textContent).toContain("Press Ctrl/⌘ + Enter to send.");
    expect(host.textContent).not.toContain("pivot");
  });

  it("switches to pivot copy with a cancel button while running", () => {
    const onCancel = vi.fn();
    const onSend = vi.fn();
    const host = render(
      <Composer
        mode="running"
        value="steer the run"
        onCancel={onCancel}
        onSend={onSend}
        onValueChange={() => undefined}
      />
    );

    expect(buttonByText(host, "Insert pivot...")).toBeDefined();
    expect(host.textContent).toContain("A run is active. Sending will try pivot first.");

    click(buttonByText(host, "Cancel"));
    expect(onCancel).toHaveBeenCalledTimes(1);

    click(buttonByText(host, "Insert pivot..."));
    expect(onSend).toHaveBeenCalledWith("steer the run");
  });

  it("omits the cancel button while running when no cancel callback is provided", () => {
    const host = render(
      <Composer
        mode="running"
        value="steer"
        onSend={() => undefined}
        onValueChange={() => undefined}
      />
    );

    expect(buttonByText(host, "Insert pivot...")).toBeDefined();
    expect(hasButton(host, "Cancel")).toBe(false);
  });

  it("surfaces the pending-interaction count without blocking input", () => {
    const host = render(
      <Composer
        mode="running"
        pendingInteractionCount={2}
        value="continue"
        onSend={() => undefined}
        onValueChange={() => undefined}
      />
    );

    expect(host.textContent).toContain("2 pending interactions");
    const textarea = host.querySelector("textarea");
    expect(textarea?.disabled).toBe(false);
  });

  it("uses the singular pending-interaction label", () => {
    const host = render(
      <Composer
        pendingInteractionCount={1}
        value=""
        onSend={() => undefined}
        onValueChange={() => undefined}
      />
    );

    expect(host.textContent).toContain("1 pending interaction");
    expect(host.textContent).not.toContain("1 pending interactions");
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

function hasButton(host: HTMLElement, text: string): boolean {
  return [...host.querySelectorAll("button")].some((candidate) =>
    candidate.textContent?.includes(text)
  );
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

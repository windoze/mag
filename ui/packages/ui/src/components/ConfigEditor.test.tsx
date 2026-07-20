import { act, type ReactElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ConfigEditor } from "./ConfigEditor";

(
  globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const mountedRoots: Array<{ readonly root: Root; readonly host: HTMLDivElement }> = [];

describe("ConfigEditor", () => {
  afterEach(() => {
    mountedRoots.splice(0).forEach(({ host, root }) => {
      act(() => root.unmount());
      host.remove();
    });
  });

  it("renders the TOML text and fires onChange on edit", () => {
    const onChange = vi.fn();
    const host = render(
      <ConfigEditor
        text={'[session]\nrouting = "model_routed"\n'}
        onApply={() => undefined}
        onChange={onChange}
        onReload={() => undefined}
        onSave={() => undefined}
      />
    );

    const textarea = host.querySelector("textarea");
    expect(textarea).not.toBeNull();
    expect(textarea?.value).toContain("model_routed");
    expect(textarea?.getAttribute("spellcheck")).toBe("false");

    act(() => {
      const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set;
      setter?.call(textarea, "[session]\n");
      textarea?.dispatchEvent(new Event("input", { bubbles: true }));
    });
    expect(onChange).toHaveBeenCalledWith("[session]\n");
  });

  it("fires Save, Reload, and Apply callbacks and notes the turn-boundary semantics", () => {
    const onSave = vi.fn();
    const onReload = vi.fn();
    const onApply = vi.fn();
    const host = render(
      <ConfigEditor
        text="x = 1"
        onApply={onApply}
        onChange={() => undefined}
        onReload={onReload}
        onSave={onSave}
      />
    );

    expect(host.textContent).toContain("takes effect at each session's next turn boundary");

    click(buttonByText(host, "Save"));
    click(buttonByText(host, "Reload"));
    click(buttonByText(host, "Apply"));
    expect(onSave).toHaveBeenCalledTimes(1);
    expect(onReload).toHaveBeenCalledTimes(1);
    expect(onApply).toHaveBeenCalledTimes(1);
  });

  it("shows the graph placeholder without editing controls", () => {
    const host = render(
      <ConfigEditor
        text="x = 1"
        onApply={() => undefined}
        onChange={() => undefined}
        onReload={() => undefined}
        onSave={() => undefined}
      />
    );

    click(buttonByText(host, "Graph"));

    expect(host.textContent).toContain("Graph mode lands in a future version");
    expect(host.querySelector("textarea")).toBeNull();
    expect(buttonByText(host, "Save").disabled).toBe(true);

    click(buttonByText(host, "Text"));
    expect(host.querySelector("textarea")).not.toBeNull();
  });

  it("supports the graph mode as initial mode prop", () => {
    const host = render(
      <ConfigEditor
        mode="graph"
        text="x = 1"
        onApply={() => undefined}
        onChange={() => undefined}
        onReload={() => undefined}
        onSave={() => undefined}
      />
    );

    expect(host.textContent).toContain("Graph mode lands in a future version");
    expect(host.querySelector("textarea")).toBeNull();
  });

  it("disables action buttons while busy and shows the error line", () => {
    const host = render(
      <ConfigEditor
        busy
        error="Invalid TOML: boom"
        text="x = 1"
        onApply={() => undefined}
        onChange={() => undefined}
        onReload={() => undefined}
        onSave={() => undefined}
      />
    );

    expect(buttonByText(host, "Save").disabled).toBe(true);
    expect(buttonByText(host, "Reload").disabled).toBe(true);
    expect(buttonByText(host, "Apply").disabled).toBe(true);
    expect(host.querySelector('[role="alert"]')?.textContent).toContain("Invalid TOML: boom");
  });

  it("fires onBack when the back button is clicked", () => {
    const onBack = vi.fn();
    const host = render(
      <ConfigEditor
        text="x = 1"
        onApply={() => undefined}
        onBack={onBack}
        onChange={() => undefined}
        onReload={() => undefined}
        onSave={() => undefined}
      />
    );

    click(buttonByText(host, "Back"));
    expect(onBack).toHaveBeenCalledTimes(1);
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

function buttonByText(host: HTMLElement, text: string): HTMLButtonElement {
  const button = [...host.querySelectorAll("button")].find(
    (candidate) => candidate.textContent?.trim() === text || candidate.textContent === text
  );
  if (button === undefined) {
    throw new Error(`missing button: ${text}`);
  }
  return button;
}

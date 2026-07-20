import { act, type ReactElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import { SourcesView } from "./SourcesView";
import type { SourceView } from "./types";

(
  globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const mountedRoots: Array<{ readonly root: Root; readonly host: HTMLDivElement }> = [];

const sources: SourceView[] = [
  {
    id: "anthropic",
    name: "Anthropic",
    kind: "llm_provider",
    available: true,
    version: "claude-sonnet-4-5",
    capabilities: ["chat", "tools"]
  },
  {
    id: "ollama",
    name: "Ollama",
    kind: "local_agent",
    available: false,
    capabilities: []
  }
];

describe("SourcesView", () => {
  afterEach(() => {
    mountedRoots.splice(0).forEach(({ host, root }) => {
      act(() => root.unmount());
      host.remove();
    });
  });

  it("renders one row per source with availability badges", () => {
    const host = render(<SourcesView sources={sources} onProbe={() => undefined} />);

    const rows = host.querySelectorAll("tbody tr");
    expect(rows).toHaveLength(2);
    expect(rows[0].textContent).toContain("Anthropic");
    expect(rows[0].textContent).toContain("llm_provider");
    expect(rows[0].textContent).toContain("available");
    expect(rows[0].textContent).toContain("claude-sonnet-4-5");
    expect(rows[0].textContent).toContain("chat");
    expect(rows[1].textContent).toContain("Ollama");
    expect(rows[1].textContent).toContain("unavailable");
  });

  it("fires onProbe when Probe is clicked", () => {
    const onProbe = vi.fn();
    const host = render(<SourcesView sources={sources} onProbe={onProbe} />);

    click(buttonByText(host, "Probe"));
    expect(onProbe).toHaveBeenCalledTimes(1);
  });

  it("shows a busy probing state that disables the button", () => {
    const host = render(<SourcesView busy sources={sources} onProbe={() => undefined} />);

    const probe = buttonByText(host, "Probing...");
    expect(probe.disabled).toBe(true);
  });

  it("renders an empty state when no sources exist", () => {
    const host = render(<SourcesView sources={[]} onProbe={() => undefined} />);

    expect(host.querySelectorAll("tbody tr")).toHaveLength(0);
    expect(host.textContent).toContain("No sources yet");
  });

  it("renders a list/probe failure as an alert row", () => {
    const host = render(
      <SourcesView
        error="probe failed: backend unavailable"
        sources={sources}
        onProbe={() => undefined}
      />
    );

    const alert = host.querySelector('[role="alert"]');
    expect(alert?.textContent).toContain("probe failed: backend unavailable");
    expect(host.querySelectorAll("tbody tr")).toHaveLength(2);
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
  const button = [...host.querySelectorAll("button")].find((candidate) =>
    candidate.textContent?.includes(text)
  );
  if (button === undefined) {
    throw new Error(`missing button: ${text}`);
  }
  return button;
}

import { act, type ReactElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";

import { ThreadView } from "./ThreadView";
import type { RunErrorKindView, ThreadItemView } from "./types";

(
  globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const mountedRoots: Array<{ readonly root: Root; readonly host: HTMLDivElement }> = [];

describe("ThreadView", () => {
  afterEach(() => {
    mountedRoots.splice(0).forEach(({ host, root }) => {
      act(() => root.unmount());
      host.remove();
    });
  });

  it("renders pivot lifecycle notices as lightweight system messages", () => {
    const host = render(
      <ThreadView
        items={[
          { type: "pivot", notice: { id: "pivot-queued", status: "queued" } },
          { type: "pivot", notice: { id: "pivot-applied", status: "applied" } },
          {
            type: "pivot",
            notice: { id: "pivot-dropped", status: "dropped", reason: "run already completed" }
          },
          { type: "pivot", notice: { id: "pivot-dropped-bare", status: "dropped" } }
        ]}
      />
    );

    expect(host.textContent).toContain("Pivot queued for the running turn.");
    expect(host.textContent).toContain("Pivot applied to the running turn.");
    expect(host.textContent).toContain("Pivot dropped: run already completed");
    expect(host.textContent).toContain("Pivot dropped.");
  });

  it("labels run errors by kind and keeps cancelled/budget/loop off the error tone", () => {
    const items: ThreadItemView[] = (
      [
        ["cancelled", "Run cancelled by user."],
        ["budget_exhausted", "Token budget reached."],
        ["loop_limit_exceeded", "Loop limit hit."],
        ["other", "Unexpected backend error."]
      ] satisfies Array<readonly [RunErrorKindView, string]>
    ).map(([kind, message]) => ({
      type: "run_error",
      error: { id: `error-${kind}`, kind, message }
    }));
    const host = render(<ThreadView items={items} />);

    const cancelled = noticeByText(host, "cancelled: Run cancelled by user.");
    const budget = noticeByText(host, "budget exhausted: Token budget reached.");
    const loop = noticeByText(host, "loop limit exceeded: Loop limit hit.");
    const other = noticeByText(host, "other: Unexpected backend error.");

    for (const notice of [cancelled, budget, loop]) {
      expect(notice.className).toContain("bg-muted");
      expect(notice.className).not.toContain("destructive");
    }
    expect(other.className).toContain("text-destructive");
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

function noticeByText(host: HTMLElement, text: string): HTMLElement {
  const element = [...host.querySelectorAll("div")].find(
    (candidate) => candidate.textContent === text
  );
  if (element === undefined) {
    throw new Error(`missing system notice: ${text}`);
  }
  return element;
}

import { describe, expect, it } from "vitest";

import { App } from "./App";

describe("App", () => {
  it("renders the web shell element", () => {
    expect(App()).toBeTruthy();
  });
});

import { describe, expect, it } from "vitest";

import { clientPackageName } from "../src/index";

describe("@mag/client", () => {
  it("exports the client package marker", () => {
    expect(clientPackageName).toBe("@mag/client");
  });
});

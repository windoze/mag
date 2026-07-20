import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { Button } from "./Button";

describe("Button", () => {
  it("renders children and shadcn-style classes", () => {
    const html = renderToStaticMarkup(<Button>Send</Button>);

    expect(html).toContain("Send");
    expect(html).toContain("inline-flex");
  });
});

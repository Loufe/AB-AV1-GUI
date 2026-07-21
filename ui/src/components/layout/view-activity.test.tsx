import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { ViewActiveContext, useViewActive } from "./view-activity";

function ActivityState() {
  return <span>{useViewActive() ? "active" : "hidden"}</span>;
}

describe("view activity contract", () => {
  it.each([
    [true, "active"],
    [false, "hidden"],
  ])("reports active=%s to a retained view", (active, expected) => {
    const html = renderToStaticMarkup(
      <ViewActiveContext value={active}>
        <ActivityState />
      </ViewActiveContext>,
    );

    expect(html).toContain(expected);
  });

  it("rejects consumers outside a production view", () => {
    expect(() => renderToStaticMarkup(<ActivityState />)).toThrow(
      "useViewActive must be used inside a production view",
    );
  });
});

import { expect, test } from "vitest";
import { formatRoute, parseRoute } from "./route";

test("round-trips routes", () => {
  for (const r of [
    { prId: null, tab: "conversation", file: null },
    { prId: 3, tab: "conversation", file: null },
    { prId: 3, tab: "files", file: null },
    { prId: 3, tab: "files", file: "src/a b/c.rs" },
  ] as const) {
    expect(parseRoute(formatRoute(r))).toEqual(r);
  }
  expect(parseRoute("#/nonsense")).toEqual({ prId: null, tab: "conversation", file: null });
});

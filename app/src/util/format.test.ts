import { expect, test } from "vitest";
import { ago, imageType, plural, short, until } from "./format";

test("formats", () => {
  const now = Date.parse("2026-09-23T12:00:00Z");
  expect(ago("2026-09-23T11:59:50Z", now)).toBe("just now");
  expect(ago("2026-09-23T11:55:00Z", now)).toBe("5m ago");
  expect(ago("2026-09-23T09:00:00Z", now)).toBe("3h ago");
  expect(ago("2026-09-21T12:00:00Z", now)).toBe("2d ago");
  expect(ago(null, now)).toBe("never");
  expect(short("abcdef0123")).toBe("abcdef0");
  expect(plural(1, "file")).toBe("1 file");
  expect(plural(2, "file")).toBe("2 files");
  expect(imageType("a/B.PNG")).toBe("image/png");
  expect(imageType("a.rs")).toBeNull();
});

test("until", () => {
  const now = Date.parse("2026-09-23T12:00:00Z");
  expect(until("2026-09-23T12:00:30Z", now)).toBe("in 30s");
  expect(until("2026-09-23T12:05:00Z", now)).toBe("in 5m");
  expect(until("2026-09-23T11:00:00Z", now)).toBe("now");
});

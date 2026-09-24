import { describe, expect, it } from "vitest";
import { findMatches, splitAtMarks } from "./find";
import type { Row } from "./rows";

const unified = (text: string, kind: "add" | "del" | "context" = "context"): Row => ({
  type: "unified",
  key: text,
  kind,
  oldNo: 1,
  newNo: 1,
  text,
  noNewline: false,
  commentable: true,
  hunk: 0,
});

describe("findMatches", () => {
  it("finds every occurrence, ignoring case, in reading order", () => {
    const rows: Row[] = [
      unified("let limit = Limit::new(limit);", "del"),
      { type: "expander", key: "e", gap: 0, hidden: 3, canDown: true, canUp: true, expandable: true, header: "limit" },
      {
        type: "split",
        key: "s",
        left: { side: "LEFT", no: 2, text: "limit", kind: "del", noNewline: false, commentable: true, hunk: 0 },
        right: { side: "RIGHT", no: 2, text: "page_size", kind: "add", noNewline: false, commentable: true, hunk: 0 },
      },
    ];
    expect(findMatches(rows, "LIMIT")).toEqual([
      { row: 0, side: "LEFT", start: 4, end: 9 },
      { row: 0, side: "LEFT", start: 12, end: 17 },
      { row: 0, side: "LEFT", start: 23, end: 28 },
      { row: 2, side: "LEFT", start: 0, end: 5 },
    ]);
    expect(findMatches(rows, "")).toEqual([]);
  });

  it("doesn't count overlapping matches twice", () => {
    expect(findMatches([unified("aaaa")], "aa").map((m) => m.start)).toEqual([0, 2]);
  });
});

describe("splitAtMarks", () => {
  it("cuts highlighted tokens where a match starts and ends", () => {
    const parts = splitAtMarks(["let ", "limit", " = 1"], [{ start: 6, end: 9, current: true }]);
    expect(parts.map((p) => [p.piece, p.text, p.mark !== null])).toEqual([
      [0, "let ", false],
      [1, "li", false],
      [1, "mit", true],
      [2, " = 1", false],
    ]);
  });

  it("marks across token boundaries", () => {
    const parts = splitAtMarks(["ab", "cd"], [{ start: 1, end: 3, current: false }]);
    expect(parts.map((p) => [p.text, p.mark !== null])).toEqual([
      ["a", false],
      ["b", true],
      ["c", true],
      ["d", false],
    ]);
  });
});

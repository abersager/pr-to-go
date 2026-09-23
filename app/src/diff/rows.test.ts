import { describe, expect, test } from "vitest";
import type { FileDiff, Hunk, ThreadEntry } from "../types";
import { buildRows, computeGaps, expand, splitLines, type Row } from "./rows";

const numbered = (n: number, edit?: (i: number) => string | null) =>
  Array.from({ length: n }, (_, i) => edit?.(i + 1) ?? `line ${i + 1}`).join("\n") + "\n";

// base: 30 lines. head: line 5 edited, two lines added after line 20.
const base = numbered(30);
const head = numbered(30, (i) => (i === 5 ? "line five" : i === 20 ? "line 20\nnew a\nnew b" : null));

const hunks: Hunk[] = [
  {
    oldStart: 2, oldLen: 7, newStart: 2, newLen: 7, section: "",
    lines: [
      { kind: "context", oldNo: 2, newNo: 2, text: "line 2", noNewline: false },
      { kind: "context", oldNo: 3, newNo: 3, text: "line 3", noNewline: false },
      { kind: "context", oldNo: 4, newNo: 4, text: "line 4", noNewline: false },
      { kind: "del", oldNo: 5, newNo: null, text: "line 5", noNewline: false },
      { kind: "add", oldNo: null, newNo: 5, text: "line five", noNewline: false },
      { kind: "context", oldNo: 6, newNo: 6, text: "line 6", noNewline: false },
      { kind: "context", oldNo: 7, newNo: 7, text: "line 7", noNewline: false },
      { kind: "context", oldNo: 8, newNo: 8, text: "line 8", noNewline: false },
    ],
  },
  {
    oldStart: 18, oldLen: 6, newStart: 18, newLen: 8, section: "fn x()",
    lines: [
      { kind: "context", oldNo: 18, newNo: 18, text: "line 18", noNewline: false },
      { kind: "context", oldNo: 19, newNo: 19, text: "line 19", noNewline: false },
      { kind: "context", oldNo: 20, newNo: 20, text: "line 20", noNewline: false },
      { kind: "add", oldNo: null, newNo: 21, text: "new a", noNewline: false },
      { kind: "add", oldNo: null, newNo: 22, text: "new b", noNewline: false },
      { kind: "context", oldNo: 21, newNo: 23, text: "line 21", noNewline: false },
      { kind: "context", oldNo: 22, newNo: 24, text: "line 22", noNewline: false },
      { kind: "context", oldNo: 23, newNo: 25, text: "line 23", noNewline: false },
    ],
  },
];

const diff: FileDiff = {
  path: "src/lib.rs", prevPath: null, changeType: "modified", patchStatus: "ok", contentStatus: "ok",
  hunks, source: "github", commentable: true, baseText: base, headText: head,
  baseBlobOid: null, headBlobOid: null, baseBinary: false, headBinary: false,
};

const thread = (side: "LEFT" | "RIGHT", line: number, outdated = false): ThreadEntry => ({
  nodeId: `T${side}${line}`, path: "src/lib.rs", subjectType: "LINE", diffSide: side, line: outdated ? null : line,
  startLine: null, startDiffSide: null, originalLine: line, isOutdated: outdated, isResolved: false,
  viewerCanReply: true, comments: [],
});

const kinds = (rows: Row[]) => rows.map((r) => r.type);

describe("splitLines", () => {
  test("matches the core", () => {
    expect(splitLines("a\r\nb\n")).toEqual(["a", "b"]);
    expect(splitLines("a\nb")).toEqual(["a", "b"]);
    expect(splitLines("")).toEqual([]);
    expect(splitLines(null)).toBeNull();
  });
});

describe("gaps", () => {
  test("leading, middle and trailing gaps", () => {
    const g = computeGaps(hunks, splitLines(head), splitLines(base));
    expect(g.map((x) => [x.oldStart, x.newStart, x.len])).toEqual([
      [1, 1, 1],
      [9, 9, 9],
      [24, 26, 7],
    ]);
  });

  test("a file with no hunks is one gap", () => {
    const g = computeGaps([], ["a", "b"], ["a", "b"]);
    expect(g).toEqual([{ index: 0, oldStart: 1, newStart: 1, len: 2, leading: true, trailing: true }]);
  });
});

describe("unified rows", () => {
  test("collapsed gaps become expanders with the hunk header", () => {
    const rows = buildRows(diff, "unified", {}, []);
    const expanders = rows.filter((r) => r.type === "expander");
    expect(expanders.map((e) => (e.type === "expander" ? [e.hidden, e.canUp, e.canDown, e.header] : null))).toEqual([
      [1, true, false, "@@ -2,7 +2,7 @@"],
      [9, true, true, "@@ -18,6 +18,8 @@ fn x()"],
      [7, false, true, ""],
    ]);
    expect(rows.filter((r) => r.type === "unified")).toHaveLength(16);
  });

  test("expanding reveals the right lines from the file contents", () => {
    let exps = expand({}, 1, "down", 9);
    exps = expand(exps, 2, "down", 7);
    const rows = buildRows(diff, "unified", exps, []);
    const expanded = rows.filter((r) => r.type === "unified" && r.kind === "expanded");
    expect(expanded[0]).toMatchObject({ oldNo: 9, newNo: 9, text: "line 9", commentable: false });
    expect(expanded[8]).toMatchObject({ oldNo: 17, newNo: 17 });
    // After the insertion, old and new numbers diverge by two.
    expect(expanded[9]).toMatchObject({ oldNo: 24, newNo: 26, text: "line 24" });
    expect(expanded).toHaveLength(9 + 7);
    expect(rows.filter((r) => r.type === "expander")).toHaveLength(1);
  });

  test("show all on the leading gap reveals from the top of the file", () => {
    const rows = buildRows(diff, "unified", expand({}, 0, "all", 1), []);
    expect(rows[0]).toMatchObject({ type: "unified", newNo: 1, text: "line 1" });
  });

  test("threads follow their line; outdated ones are left out", () => {
    const rows = buildRows(diff, "unified", {}, [thread("RIGHT", 5), thread("LEFT", 5), thread("RIGHT", 21, true)]);
    const i = rows.findIndex((r) => r.type === "thread");
    expect(rows[i - 1]).toMatchObject({ kind: "del", oldNo: 5 });
    expect(rows[i]).toMatchObject({ side: "LEFT", line: 5 });
    expect(rows[i + 1]).toMatchObject({ kind: "add", newNo: 5 });
    expect(rows[i + 2]).toMatchObject({ type: "thread", side: "RIGHT", line: 5 });
    expect(rows.filter((r) => r.type === "thread")).toHaveLength(2);
  });
});

describe("split rows", () => {
  test("pairs deletions with additions", () => {
    const rows = buildRows(diff, "split", {}, []);
    const changed = rows.find((r) => r.type === "split" && r.left?.kind === "del");
    expect(changed).toMatchObject({ left: { no: 5, text: "line 5" }, right: { no: 5, text: "line five" } });
    const addOnly = rows.filter((r) => r.type === "split" && r.left === null);
    expect(addOnly.map((r) => (r.type === "split" ? r.right?.no : 0))).toEqual([21, 22]);
    expect(kinds(rows).filter((k) => k === "split")).toHaveLength(15);
  });
});

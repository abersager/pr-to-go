// Find in the diff: plain, case-insensitive text search over the lines on
// screen (collapsed context isn't searched, as with any find in page). Pure,
// so it's unit-tested.

import type { Row, Side } from "./rows";

export type Match = { row: number; side: Side; start: number; end: number };

/** Every match, in reading order (left before right on a split row). */
export function findMatches(rows: Row[], query: string): Match[] {
  const q = query.toLowerCase();
  if (!q) return [];
  const out: Match[] = [];
  const scan = (row: number, side: Side, text: string) => {
    const t = text.toLowerCase();
    for (let i = t.indexOf(q); i !== -1; i = t.indexOf(q, i + q.length)) {
      out.push({ row, side, start: i, end: i + q.length });
    }
  };
  rows.forEach((r, i) => {
    if (r.type === "unified") scan(i, r.kind === "del" ? "LEFT" : "RIGHT", r.text);
    else if (r.type === "split") {
      if (r.left) scan(i, "LEFT", r.left.text);
      if (r.right) scan(i, "RIGHT", r.right.text);
    }
  });
  return out;
}

export type Mark = { start: number; end: number; current: boolean };

/** Splits `pieces` (the line's text, possibly already cut into highlighted
 * tokens) at the marks. Each part says which piece it came from and whether
 * it's inside a mark. */
export function splitAtMarks(
  pieces: string[],
  marks: Mark[],
): { piece: number; text: string; mark: Mark | null }[] {
  const out: { piece: number; text: string; mark: Mark | null }[] = [];
  let pos = 0;
  pieces.forEach((text, piece) => {
    const from = pos;
    const to = pos + text.length;
    let at = from;
    const cuts = marks
      .filter((m) => m.end > from && m.start < to)
      .flatMap((m) => [Math.max(m.start, from), Math.min(m.end, to)]);
    for (const cut of [...cuts, to]) {
      if (cut > at) {
        const mark = marks.find((m) => m.start <= at && at < m.end) ?? null;
        out.push({ piece, text: text.slice(at - from, cut - from), mark });
        at = cut;
      }
    }
    pos = to;
  });
  return out;
}

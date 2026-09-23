// Turns a FileDiff into display rows. Hunks come from GitHub's patch; the
// unchanged gaps between them are filled from the stored file contents when
// the user expands them (DESIGN.md §8). Pure, so it's unit-tested.

import type { FileDiff, Hunk, LineKind, ThreadEntry } from "../types";

export type Side = "LEFT" | "RIGHT";

export type CellKind = LineKind | "expanded";

export type Cell = {
  side: Side;
  no: number;
  text: string;
  kind: CellKind;
  noNewline: boolean;
  /** GitHub accepts a comment on this line (it's inside a hunk). */
  commentable: boolean;
  hunk: number | null;
};

export type Row =
  | {
      type: "expander";
      key: string;
      gap: number;
      hidden: number;
      /** Reveal lines at the top of the gap (below the previous hunk). */
      canDown: boolean;
      /** Reveal lines at the bottom of the gap (above the next hunk). */
      canUp: boolean;
      /** Whether the gap's text is available to expand at all. */
      expandable: boolean;
      header: string;
    }
  | {
      type: "unified";
      key: string;
      kind: CellKind;
      oldNo: number | null;
      newNo: number | null;
      text: string;
      noNewline: boolean;
      commentable: boolean;
      hunk: number | null;
    }
  | { type: "split"; key: string; left: Cell | null; right: Cell | null }
  | { type: "thread"; key: string; side: Side; line: number; thread: ThreadEntry };

export type Expansion = { top: number; bottom: number };
export type Expansions = Record<number, Expansion>;

export type Gap = {
  index: number;
  oldStart: number;
  newStart: number;
  len: number;
  leading: boolean;
  trailing: boolean;
};

export function splitLines(text: string | null): string[] | null {
  if (text === null) return null;
  if (text === "") return [];
  const lines = text.split("\n").map((l) => (l.endsWith("\r") ? l.slice(0, -1) : l));
  if (text.endsWith("\n")) lines.pop();
  return lines;
}

// With a zero-length range, unified diffs give the line *before* the hunk.
const oldFirst = (h: Hunk) => (h.oldLen === 0 ? h.oldStart + 1 : h.oldStart);
const newFirst = (h: Hunk) => (h.newLen === 0 ? h.newStart + 1 : h.newStart);
const oldNext = (h: Hunk) => oldFirst(h) + h.oldLen;
const newNext = (h: Hunk) => newFirst(h) + h.newLen;

export function hunkHeader(h: Hunk): string {
  return `@@ -${h.oldStart},${h.oldLen} +${h.newStart},${h.newLen} @@${h.section ? " " + h.section : ""}`;
}

/** The unchanged stretches before, between and after the hunks. */
export function computeGaps(hunks: Hunk[], head: string[] | null, base: string[] | null): Gap[] {
  const fileLen = head?.length ?? base?.length ?? null;
  if (hunks.length === 0) {
    return [{ index: 0, oldStart: 1, newStart: 1, len: fileLen ?? 0, leading: true, trailing: true }];
  }
  const gaps: Gap[] = [];
  hunks.forEach((h, i) => {
    const prev = hunks[i - 1];
    const oldStart = prev ? oldNext(prev) : 1;
    const newStart = prev ? newNext(prev) : 1;
    gaps.push({ index: i, oldStart, newStart, len: Math.max(0, newFirst(h) - newStart), leading: i === 0, trailing: false });
  });
  const last = hunks[hunks.length - 1];
  const newStart = newNext(last);
  const oldStart = oldNext(last);
  const len = head ? head.length - newStart + 1 : base ? base.length - oldStart + 1 : 0;
  gaps.push({ index: hunks.length, oldStart, newStart, len: Math.max(0, len), leading: false, trailing: true });
  return gaps;
}

export type Mode = "split" | "unified";

export function buildRows(diff: FileDiff, mode: Mode, expansions: Expansions, threads: ThreadEntry[]): Row[] {
  const head = splitLines(diff.headText);
  const base = splitLines(diff.baseText);
  const gaps = computeGaps(diff.hunks, head, base);
  const byLine = new Map<string, ThreadEntry[]>();
  for (const t of threads) {
    if (t.subjectType !== "LINE" || t.isOutdated || t.line === null || t.path !== diff.path) continue;
    const key = `${t.diffSide ?? "RIGHT"}:${t.line}`;
    byLine.set(key, [...(byLine.get(key) ?? []), t]);
  }
  const rows: Row[] = [];

  const pushThreads = (side: Side, line: number | null) => {
    if (line === null) return;
    for (const t of byLine.get(`${side}:${line}`) ?? []) {
      rows.push({ type: "thread", key: `t-${t.nodeId}`, side, line, thread: t });
    }
  };

  const gapText = (g: Gap, offset: number): string | null => {
    const n = g.newStart + offset;
    const o = g.oldStart + offset;
    return head?.[n - 1] ?? base?.[o - 1] ?? null;
  };

  const pushContext = (g: Gap, offset: number) => {
    const oldNo = g.oldStart + offset;
    const newNo = g.newStart + offset;
    const text = gapText(g, offset) ?? "";
    if (mode === "unified") {
      rows.push({ type: "unified", key: `e-${newNo}`, kind: "expanded", oldNo, newNo, text, noNewline: false, commentable: false, hunk: null });
    } else {
      const cell = (side: Side, no: number): Cell => ({ side, no, text, kind: "expanded", noNewline: false, commentable: false, hunk: null });
      rows.push({ type: "split", key: `e-${newNo}`, left: cell("LEFT", oldNo), right: cell("RIGHT", newNo) });
    }
    pushThreads("LEFT", oldNo);
    pushThreads("RIGHT", newNo);
  };

  const pushGap = (g: Gap, header: string) => {
    if (g.len <= 0) return;
    const expandable = head !== null || base !== null;
    const e = expansions[g.index] ?? { top: 0, bottom: 0 };
    let top = 0;
    let bottom = 0;
    if (expandable) {
      if (g.leading && !g.trailing) bottom = Math.min(e.bottom, g.len);
      else if (g.trailing) top = Math.min(e.top, g.len);
      else {
        top = Math.min(e.top, g.len);
        bottom = Math.min(e.bottom, g.len - top);
      }
    }
    for (let i = 0; i < top; i++) pushContext(g, i);
    const hidden = g.len - top - bottom;
    if (hidden > 0) {
      rows.push({
        type: "expander",
        key: `x-${g.index}`,
        gap: g.index,
        hidden,
        canDown: expandable && !g.leading,
        canUp: expandable && !g.trailing,
        expandable,
        header,
      });
    }
    for (let i = g.len - bottom; i < g.len; i++) pushContext(g, i);
  };

  const pushHunk = (h: Hunk, hi: number) => {
    const commentable = diff.commentable;
    if (mode === "unified") {
      for (const l of h.lines) {
        rows.push({
          type: "unified",
          key: `h${hi}-${l.oldNo ?? ""}-${l.newNo ?? ""}`,
          kind: l.kind,
          oldNo: l.oldNo,
          newNo: l.newNo,
          text: l.text,
          noNewline: l.noNewline,
          commentable,
          hunk: hi,
        });
        if (l.kind !== "add") pushThreads("LEFT", l.oldNo);
        if (l.kind !== "del") pushThreads("RIGHT", l.newNo);
      }
      return;
    }
    const cell = (side: Side, no: number, l: Hunk["lines"][number]): Cell => ({
      side,
      no,
      text: l.text,
      kind: l.kind,
      noNewline: l.noNewline,
      commentable,
      hunk: hi,
    });
    let i = 0;
    const lines = h.lines;
    while (i < lines.length) {
      const l = lines[i];
      if (l.kind === "context") {
        rows.push({
          type: "split",
          key: `h${hi}-${l.oldNo}-${l.newNo}`,
          left: cell("LEFT", l.oldNo!, l),
          right: cell("RIGHT", l.newNo!, l),
        });
        pushThreads("LEFT", l.oldNo);
        pushThreads("RIGHT", l.newNo);
        i++;
        continue;
      }
      // A run of deletions followed by a run of additions, paired up.
      const dels = [];
      while (i < lines.length && lines[i].kind === "del") dels.push(lines[i++]);
      const adds = [];
      while (i < lines.length && lines[i].kind === "add") adds.push(lines[i++]);
      for (let k = 0; k < Math.max(dels.length, adds.length); k++) {
        const d = dels[k];
        const a = adds[k];
        rows.push({
          type: "split",
          key: `h${hi}-${d?.oldNo ?? ""}-${a?.newNo ?? ""}`,
          left: d ? cell("LEFT", d.oldNo!, d) : null,
          right: a ? cell("RIGHT", a.newNo!, a) : null,
        });
        if (d) pushThreads("LEFT", d.oldNo);
        if (a) pushThreads("RIGHT", a.newNo);
      }
    }
  };

  diff.hunks.forEach((h, i) => {
    pushGap(gaps[i], hunkHeader(h));
    pushHunk(h, i);
  });
  pushGap(gaps[gaps.length - 1], "");
  return rows;
}

/** Expansion step for the expander buttons. */
export const EXPAND_STEP = 20;

export function expand(exps: Expansions, gap: number, dir: "up" | "down" | "all", len: number): Expansions {
  const e = exps[gap] ?? { top: 0, bottom: 0 };
  const next =
    dir === "all"
      ? { top: len, bottom: len }
      : dir === "down"
        ? { ...e, top: e.top + EXPAND_STEP }
        : { ...e, bottom: e.bottom + EXPAND_STEP };
  return { ...exps, [gap]: next };
}

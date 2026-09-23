import { useVirtualizer } from "@tanstack/react-virtual";
import { type CSSProperties, useMemo, useRef, useState } from "react";
import { api, localUrl } from "../api";
import { HIGHLIGHT_MAX_CHARS, type LineTokens, type Token, useHighlight } from "../diff/highlight";
import {
  type Cell,
  type ComposerAt,
  type Expansions,
  type Mode,
  type Row,
  type Side,
  buildRows,
  expand,
  splitLines,
} from "../diff/rows";
import type { Draft, DraftComment, FileDiff, ThreadEntry } from "../types";
import { imageType } from "../util/format";
import { Composer } from "./Composer";
import { DraftCard, rangeLabel } from "./DraftCard";
import { ReplyArea } from "./ReplyArea";
import { ThreadView } from "./Thread";

function Code({ text, tokens }: { text: string; tokens: Token[] | undefined }) {
  // Only use tokens if they're for exactly this text (they come from the
  // stored file; the row text comes from GitHub's patch).
  if (tokens && tokens.map((t) => t[0]).join("") === text) {
    return (
      <>
        {tokens.map((t, i) => (
          <span
            key={i}
            className="tk"
            style={
              {
                "--l": t[1],
                "--d": t[2],
                fontStyle: t[3] & 1 ? "italic" : undefined,
                fontWeight: t[3] & 2 ? 600 : undefined,
              } as CSSProperties
            }
          >
            {t[0]}
          </span>
        ))}
      </>
    );
  }
  return <>{text}</>;
}

const marker = (kind: string) => (kind === "add" ? "+" : kind === "del" ? "−" : " ");

function ImageDiff({ diff }: { diff: FileDiff }) {
  const type = imageType(diff.path);
  const src = (oid: string | null) => (oid ? localUrl(`blob/${oid}?type=${encodeURIComponent(type ?? "")}`) : null);
  const before = diff.changeType === "added" ? null : src(diff.baseBlobOid);
  const after = diff.changeType === "removed" ? null : src(diff.headBlobOid);
  return (
    <div className="image-diff">
      <figure>
        <figcaption>Before</figcaption>
        {before ? <img src={before} alt="before" /> : <span className="muted">—</span>}
      </figure>
      <figure>
        <figcaption>After</figcaption>
        {after ? <img src={after} alt="after" /> : <span className="muted">—</span>}
      </figure>
    </div>
  );
}

type Point = { side: Side; line: number };

export type DiffViewProps = {
  diff: FileDiff;
  threads: ThreadEntry[];
  mode: Mode;
  prId: number;
  revisionId: number;
  draft: Draft | null;
  /** The draft can be changed (it isn't queued or being sent). */
  editable: boolean;
  onDraft: (d: Draft | null) => void;
  onNotEditable: () => void;
  fileComposer: boolean;
  onFileComposer: (open: boolean) => void;
};

export function DiffView({
  diff,
  threads,
  mode: preferred,
  prId,
  revisionId,
  draft,
  editable,
  onDraft,
  onNotEditable,
  fileComposer,
  onFileComposer,
}: DiffViewProps) {
  // An added or removed file has nothing to show on one side.
  const mode: Mode = diff.changeType === "added" || diff.changeType === "removed" ? "unified" : preferred;
  const [expansions, setExpansions] = useState<Expansions>({});
  const [composer, setComposerState] = useState<ComposerAt | null>(null);
  const [fileCommentId, setFileCommentId] = useState<number | null>(null);
  // Queued saves read these, so they must not lag behind a render.
  const composerRef = useRef<ComposerAt | null>(null);
  const fileCommentRef = useRef<number | null>(null);
  const lastClick = useRef<(Point & { hunk: number }) | null>(null);
  const setComposer = (c: ComposerAt | null) => {
    composerRef.current = c;
    setComposerState(c);
  };

  const headTokens = useHighlight(diff.headBlobOid, diff.path, diff.headText);
  const baseTokens = useHighlight(diff.baseBlobOid, diff.prevPath ?? diff.path, diff.baseText);
  const headLines = useMemo(() => splitLines(diff.headText), [diff.headText]);

  const fileDrafts = useMemo(
    () => (draft?.comments ?? []).filter((c) => c.kind === "thread" && c.path === diff.path),
    [draft, diff.path],
  );
  const lineDrafts = fileDrafts.filter((c) => c.subjectType === "LINE" && c.anchorRevisionId === revisionId);
  // Drafts written against an earlier version of the PR can't be placed by
  // line number here; they're listed above the diff.
  const earlierDrafts = fileDrafts.filter((c) => c.subjectType === "LINE" && c.anchorRevisionId !== revisionId);
  const fileLevelDrafts = fileDrafts.filter((c) => c.subjectType === "FILE");

  const rows = useMemo(
    () => buildRows(diff, mode, expansions, threads, { drafts: lineDrafts, composer }),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [diff, mode, expansions, threads, draft, composer, revisionId],
  );

  // Row index of each line, for ordering range ends and highlighting.
  const rowOf = useMemo(() => {
    const m = new Map<string, number>();
    rows.forEach((r, i) => {
      if (r.type === "unified") {
        if (r.kind !== "add" && r.oldNo !== null) m.set(`LEFT:${r.oldNo}`, i);
        if (r.kind !== "del" && r.newNo !== null) m.set(`RIGHT:${r.newNo}`, i);
      } else if (r.type === "split") {
        if (r.left) m.set(`LEFT:${r.left.no}`, i);
        if (r.right) m.set(`RIGHT:${r.right.no}`, i);
      }
    });
    return m;
  }, [rows]);

  const selected = useMemo(() => {
    if (!composer) return null;
    const end = rowOf.get(`${composer.side}:${composer.line}`);
    const start = composer.start ? rowOf.get(`${composer.start.side}:${composer.start.line}`) : end;
    if (end === undefined || start === undefined) return null;
    return [Math.min(start, end), Math.max(start, end)] as const;
  }, [composer, rowOf]);

  const parentRef = useRef<HTMLDivElement>(null);
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => parentRef.current,
    estimateSize: (i) => (["unified", "split", "expander"].includes(rows[i].type) ? 20 : 140),
    getItemKey: (i) => rows[i].key,
    overscan: 40,
  });

  const fileThreads = threads.filter((t) => t.path === diff.path && t.subjectType === "FILE");
  const outdated = threads.filter((t) => t.path === diff.path && t.subjectType === "LINE" && t.isOutdated);

  const tok = (side: Side, no: number | null): Token[] | undefined => {
    if (no === null) return undefined;
    const src: LineTokens[] | null = side === "LEFT" ? baseTokens : headTokens;
    return src?.[no - 1];
  };

  /** New-side lines a suggestion would replace, or null if not allowed. */
  const suggestionLines = (at: { side: Side; line: number; start: Point | null }): string[] | null => {
    if (at.side !== "RIGHT" || (at.start && at.start.side !== "RIGHT") || !headLines) return null;
    const from = at.start?.line ?? at.line;
    return headLines.slice(from - 1, at.line);
  };

  const originalFor = (c: DraftComment) =>
    c.side === "RIGHT" && (c.startSide ?? "RIGHT") === "RIGHT" && c.line !== null
      ? (headLines?.slice((c.startLine ?? c.line) - 1, c.line) ?? undefined)
      : undefined;

  const clickLine = (e: React.MouseEvent, side: Side, no: number, hunk: number | null, commentable: boolean) => {
    if (!commentable || hunk === null) return;
    if (!editable) return onNotEditable();
    const prev = lastClick.current;
    const cur = composerRef.current;
    if (e.shiftKey && prev && prev.hunk === hunk && cur && cur.commentId === null) {
      // Extend to a range within the hunk; the composer goes under its end.
      const a = rowOf.get(`${prev.side}:${prev.line}`) ?? 0;
      const b = rowOf.get(`${side}:${no}`) ?? 0;
      const [start, end] = a <= b ? [prev, { side, line: no }] : [{ side, line: no }, prev];
      const same = start.side === end.side && start.line === end.line;
      setComposer({ side: end.side, line: end.line, start: same ? null : { side: start.side, line: start.line }, commentId: null });
      return;
    }
    lastClick.current = { side, line: no, hunk };
    setComposer({ side, line: no, start: null, commentId: null });
  };

  const saveLine = async (body: string) => {
    const at = composerRef.current;
    if (!at) return;
    if (at.commentId !== null) {
      onDraft(await api.updateDraftComment(at.commentId, body));
      return;
    }
    const before = new Set((draft?.comments ?? []).map((c) => c.id));
    const d = await api.addDraftComment(prId, {
      revisionId,
      kind: "thread",
      subjectType: "LINE",
      path: diff.path,
      side: at.side,
      line: at.line,
      startSide: at.start?.side ?? null,
      startLine: at.start?.line ?? null,
      body,
    });
    const created = d.comments.find((c) => !before.has(c.id));
    if (created) composerRef.current = { ...at, commentId: created.id };
    onDraft(d);
    if (created) setComposerState(composerRef.current);
  };

  const saveFile = async (body: string) => {
    if (fileCommentRef.current !== null) {
      onDraft(await api.updateDraftComment(fileCommentRef.current, body));
      return;
    }
    const before = new Set((draft?.comments ?? []).map((c) => c.id));
    const d = await api.addDraftComment(prId, { revisionId, kind: "thread", subjectType: "FILE", path: diff.path, body });
    const created = d.comments.find((c) => !before.has(c.id));
    fileCommentRef.current = created?.id ?? null;
    setFileCommentId(fileCommentRef.current);
    onDraft(d);
  };

  const deleteComment = async (id: number) => onDraft(await api.deleteDraftComment(id));

  const lineNo = (side: Side, no: number, hunk: number | null, commentable: boolean, kind: string) => (
    <span
      className={`ln ${kind} ${commentable ? "commentable" : ""}`}
      onClick={(e) => clickLine(e, side, no, hunk, commentable)}
      title={commentable ? "Comment on this line (shift-click to select a range)" : undefined}
    >
      {no}
    </span>
  );

  const renderCell = (c: Cell | null) =>
    c ? (
      <>
        {lineNo(c.side, c.no, c.hunk, c.commentable, c.kind)}
        <span className={`code ${c.kind}`}>
          <span className="mk">{marker(c.kind)}</span>
          <Code text={c.text} tokens={tok(c.side, c.no)} />
          {c.noNewline && <span className="nonl" title="No newline at end of file">⏎̸</span>}
        </span>
      </>
    ) : (
      <>
        <span className="ln empty" />
        <span className="code empty" />
      </>
    );

  const sideClass = (side: Side) => (mode === "split" ? side.toLowerCase() : "");

  const renderRow = (r: Row, i: number) => {
    const sel = selected && i >= selected[0] && i <= selected[1] ? " selected-line" : "";
    switch (r.type) {
      case "expander":
        return (
          <div className="expander">
            {r.expandable ? (
              <>
                {r.canDown && (
                  <button onClick={() => setExpansions((e) => expand(e, r.gap, "down", r.hidden))} title="Show more below">
                    ↓
                  </button>
                )}
                {r.canUp && (
                  <button onClick={() => setExpansions((e) => expand(e, r.gap, "up", r.hidden))} title="Show more above">
                    ↑
                  </button>
                )}
                <button onClick={() => setExpansions((e) => expand(e, r.gap, "all", Number.MAX_SAFE_INTEGER))}>
                  Show {r.hidden} unchanged {r.hidden === 1 ? "line" : "lines"}
                </button>
              </>
            ) : (
              <span className="muted">{r.hidden} unchanged lines</span>
            )}
            {r.header && <span className="hunk-header">{r.header}</span>}
          </div>
        );
      case "unified": {
        const side: Side = r.kind === "del" ? "LEFT" : "RIGHT";
        const no = (side === "LEFT" ? r.oldNo : r.newNo)!;
        return (
          <div className={`line unified ${r.kind}${sel}`}>
            <span className="ln">{r.oldNo ?? ""}</span>
            {lineNo(side, no, r.hunk, r.commentable, "")}
            <span className={`code ${r.kind}`}>
              <span className="mk">{marker(r.kind)}</span>
              <Code text={r.text} tokens={tok(side, no)} />
              {r.noNewline && <span className="nonl" title="No newline at end of file">⏎̸</span>}
            </span>
          </div>
        );
      }
      case "split":
        return (
          <div className={`line split${sel}`}>
            {renderCell(r.left)}
            {renderCell(r.right)}
          </div>
        );
      case "thread":
        return (
          <div className={`thread-row ${sideClass(r.side)}`}>
            <ThreadView
              thread={r.thread}
              footer={
                <ReplyArea
                  prId={prId}
                  revisionId={revisionId}
                  thread={r.thread}
                  draft={draft}
                  editable={editable}
                  onDraft={onDraft}
                  onNotEditable={onNotEditable}
                />
              }
            />
          </div>
        );
      case "draft":
        return (
          <div className={`thread-row ${sideClass(r.side)}`}>
            <DraftCard
              comment={r.comment}
              original={originalFor(r.comment)}
              onEdit={
                editable
                  ? () =>
                      setComposer({
                        side: r.comment.side!,
                        line: r.comment.line!,
                        start: r.comment.startLine
                          ? { side: r.comment.startSide ?? r.comment.side!, line: r.comment.startLine }
                          : null,
                        commentId: r.comment.id,
                      })
                  : undefined
              }
              onDelete={editable ? () => void deleteComment(r.comment.id) : undefined}
            />
          </div>
        );
      case "composer": {
        const editingComment = draft?.comments.find((c) => c.id === r.at.commentId);
        return (
          <div className={`thread-row ${sideClass(r.side)}`}>
            <Composer
              key={`${r.at.side}:${r.at.line}:${r.at.start?.line ?? ""}`}
              label={`Comment on ${rangeLabel({
                side: r.at.side,
                line: r.at.line,
                startSide: r.at.start?.side ?? null,
                startLine: r.at.start?.line ?? null,
              })}`}
              initial={editingComment?.bodyMd ?? ""}
              save={saveLine}
              remove={async () => {
                const id = composerRef.current?.commentId;
                if (id != null) await deleteComment(id);
              }}
              close={() => setComposer(null)}
              suggestionLines={suggestionLines(r.at)}
            />
          </div>
        );
      }
    }
  };

  const binary = diff.baseBinary || diff.headBinary;
  const hasTop =
    fileThreads.length > 0 || outdated.length > 0 || fileLevelDrafts.length > 0 || earlierDrafts.length > 0 || fileComposer;
  return (
    <div className="diff">
      {diff.source === "local" && (
        <p className="notice warn">
          GitHub sent no diff for this file (it's too large). This diff was computed locally; line comments are off.
        </p>
      )}
      {diff.contentStatus !== "ok" && diff.contentStatus !== "binary_skipped" && (
        <p className="notice warn">The file contents aren't available offline ({diff.contentStatus.replace("_", " ")}).</p>
      )}
      {Math.max(diff.headText?.length ?? 0, diff.baseText?.length ?? 0) > HIGHLIGHT_MAX_CHARS && (
        <p className="notice small">This file is too large for syntax highlighting.</p>
      )}
      {hasTop && (
        <div className="file-threads">
          {fileThreads.map((t) => (
            <ThreadView
              key={t.nodeId}
              thread={t}
              footer={
                <ReplyArea
                  prId={prId}
                  revisionId={revisionId}
                  thread={t}
                  draft={draft}
                  editable={editable}
                  onDraft={onDraft}
                  onNotEditable={onNotEditable}
                />
              }
            />
          ))}
          {fileLevelDrafts
            .filter((c) => c.id !== fileCommentId)
            .map((c) => (
              <DraftCard
                key={c.id}
                comment={c}
                onEdit={
                  editable
                    ? () => {
                        fileCommentRef.current = c.id;
                        setFileCommentId(c.id);
                        onFileComposer(true);
                      }
                    : undefined
                }
                onDelete={editable ? () => void deleteComment(c.id) : undefined}
              />
            ))}
          {fileComposer && (
            <Composer
              label="Comment on this file"
              initial={fileLevelDrafts.find((c) => c.id === fileCommentId)?.bodyMd ?? ""}
              save={saveFile}
              remove={async () => {
                if (fileCommentRef.current !== null) await deleteComment(fileCommentRef.current);
              }}
              close={() => {
                fileCommentRef.current = null;
                setFileCommentId(null);
                onFileComposer(false);
              }}
            />
          )}
          {earlierDrafts.length > 0 && (
            <div className="notice small">
              {earlierDrafts.length === 1 ? "One draft comment was" : `${earlierDrafts.length} draft comments were`}{" "}
              written on an earlier version of this pull request:
              {earlierDrafts.map((c) => (
                <DraftCard key={c.id} comment={c} onDelete={editable ? () => void deleteComment(c.id) : undefined} />
              ))}
            </div>
          )}
          {outdated.length > 0 && (
            <details>
              <summary className="small">
                {outdated.length} outdated {outdated.length === 1 ? "thread" : "threads"}
              </summary>
              {outdated.map((t) => (
                <ThreadView key={t.nodeId} thread={t} showContext />
              ))}
            </details>
          )}
        </div>
      )}
      {binary ? (
        imageType(diff.path) ? <ImageDiff diff={diff} /> : <p className="muted pad">Binary file changed.</p>
      ) : diff.hunks.length === 0 && diff.changeType === "renamed" && rows.length <= 1 ? (
        <p className="muted pad">
          Renamed from <code>{diff.prevPath}</code> without changes.
        </p>
      ) : null}
      {!binary && (
        <div ref={parentRef} className={`diff-scroll ${mode}`}>
          <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
            {virtualizer.getVirtualItems().map((item) => (
              <div
                key={item.key}
                data-index={item.index}
                ref={virtualizer.measureElement}
                className="vrow"
                style={{ transform: `translateY(${item.start}px)` }}
              >
                {renderRow(rows[item.index], item.index)}
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

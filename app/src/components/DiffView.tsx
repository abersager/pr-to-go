import { useVirtualizer } from "@tanstack/react-virtual";
import { type CSSProperties, useMemo, useRef, useState } from "react";
import { localUrl } from "../api";
import { type LineTokens, type Token, useHighlight } from "../diff/highlight";
import { type Cell, type Expansions, type Mode, type Row, buildRows, expand } from "../diff/rows";
import type { FileDiff, ThreadEntry } from "../types";
import { imageType } from "../util/format";
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

export function DiffView({
  diff,
  threads,
  mode: preferred,
}: {
  diff: FileDiff;
  threads: ThreadEntry[];
  mode: Mode;
}) {
  // An added or removed file has nothing to show on one side.
  const mode: Mode = diff.changeType === "added" || diff.changeType === "removed" ? "unified" : preferred;
  const [expansions, setExpansions] = useState<Expansions>({});
  const headTokens = useHighlight(diff.headBlobOid, diff.path, diff.headText);
  const baseTokens = useHighlight(diff.baseBlobOid, diff.prevPath ?? diff.path, diff.baseText);
  const rows = useMemo(() => buildRows(diff, mode, expansions, threads), [diff, mode, expansions, threads]);
  const parentRef = useRef<HTMLDivElement>(null);
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => parentRef.current,
    estimateSize: (i) => (rows[i].type === "thread" ? 140 : 20),
    getItemKey: (i) => rows[i].key,
    overscan: 40,
  });

  const fileThreads = threads.filter((t) => t.path === diff.path && t.subjectType === "FILE");
  const outdated = threads.filter((t) => t.path === diff.path && t.subjectType === "LINE" && t.isOutdated);

  const tok = (side: "LEFT" | "RIGHT", no: number | null, kind: string): Token[] | undefined => {
    if (no === null) return undefined;
    const src: LineTokens[] | null = side === "LEFT" && kind === "del" ? baseTokens : side === "LEFT" ? baseTokens : headTokens;
    return src?.[no - 1];
  };

  const renderCell = (c: Cell | null) =>
    c ? (
      <>
        <span className={`ln ${c.kind}`}>{c.no}</span>
        <span className={`code ${c.kind}`}>
          <span className="mk">{marker(c.kind)}</span>
          <Code text={c.text} tokens={tok(c.side, c.no, c.kind)} />
          {c.noNewline && <span className="nonl" title="No newline at end of file">⏎̸</span>}
        </span>
      </>
    ) : (
      <>
        <span className="ln empty" />
        <span className="code empty" />
      </>
    );

  const renderRow = (r: Row) => {
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
      case "unified":
        return (
          <div className={`line unified ${r.kind}`}>
            <span className="ln">{r.oldNo ?? ""}</span>
            <span className="ln">{r.newNo ?? ""}</span>
            <span className={`code ${r.kind}`}>
              <span className="mk">{marker(r.kind)}</span>
              <Code
                text={r.text}
                tokens={r.kind === "del" ? tok("LEFT", r.oldNo, "del") : tok("RIGHT", r.newNo, r.kind)}
              />
              {r.noNewline && <span className="nonl" title="No newline at end of file">⏎̸</span>}
            </span>
          </div>
        );
      case "split":
        return (
          <div className="line split">
            {renderCell(r.left)}
            {renderCell(r.right)}
          </div>
        );
      case "thread":
        return (
          <div className={`thread-row ${mode === "split" ? r.side.toLowerCase() : ""}`}>
            <ThreadView thread={r.thread} />
          </div>
        );
    }
  };

  const binary = diff.baseBinary || diff.headBinary;
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
      {(fileThreads.length > 0 || outdated.length > 0) && (
        <div className="file-threads">
          {fileThreads.map((t) => (
            <ThreadView key={t.nodeId} thread={t} />
          ))}
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
                {renderRow(rows[item.index])}
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

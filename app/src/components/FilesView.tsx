import { useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api";
import { useCommand, withShortcut } from "../commands";
import type { Mode } from "../diff/rows";
import type { Draft, FileEntry, PrDetail } from "../types";
import { useAsync } from "../util/useAsync";
import { DiffView } from "./DiffView";

const STATUS: Record<string, string> = { added: "A", removed: "D", modified: "M", renamed: "R", copied: "C", changed: "M" };

function loadMode(): Mode {
  try {
    return localStorage.getItem("diffMode") === "unified" ? "unified" : "split";
  } catch {
    return "split";
  }
}

export function FilesView({
  pr,
  onViewedChanged,
  file,
  onFile,
  draft,
  editable,
  onDraft,
  onNotEditable,
}: {
  pr: PrDetail;
  onViewedChanged: () => void;
  file: string | null;
  onFile: (path: string) => void;
  draft: Draft | null;
  editable: boolean;
  onDraft: (d: Draft | null) => void;
  onNotEditable: () => void;
}) {
  const [mode, setMode] = useState<Mode>(loadMode);
  const [fileComposer, setFileComposer] = useState(false);
  // Generated files the user chose to show.
  const [shown, setShown] = useState<ReadonlySet<string>>(new Set());
  // "Viewed" as just set here, until the reloaded PR says the same.
  const [viewedNow, setViewedNow] = useState<Record<string, boolean>>({});
  useEffect(() => setViewedNow({}), [pr]);
  const isViewed = (f: FileEntry) => viewedNow[f.path] ?? f.viewed;
  const [find, setFind] = useState({ open: false, query: "", index: 0 });
  const [findCount, setFindCount] = useState(0);
  const findInput = useRef<HTMLInputElement>(null);
  const draftCount = useMemo(() => {
    const m = new Map<string, number>();
    for (const c of draft?.comments ?? []) if (c.path) m.set(c.path, (m.get(c.path) ?? 0) + 1);
    return m;
  }, [draft]);
  const selected = pr.files.some((f) => f.path === file) ? file : (pr.files[0]?.path ?? null);
  const setSelected = (path: string) => {
    setFileComposer(false);
    setFind((f) => ({ ...f, index: 0 }));
    onFile(path);
  };
  const revisionId = pr.revision?.id ?? null;
  const index = pr.files.findIndex((f) => f.path === selected);
  const threadsByPath = useMemo(() => {
    const m = new Map<string, number>();
    for (const t of pr.threads) if (!t.isOutdated) m.set(t.path, (m.get(t.path) ?? 0) + 1);
    return m;
  }, [pr.threads]);
  const entry = index >= 0 ? pr.files[index] : null;
  // Generated files stay collapsed unless someone commented on them.
  const collapsed =
    !!entry?.generated && !shown.has(entry.path) && !threadsByPath.get(entry.path) && !draftCount.get(entry.path);
  const diff = useAsync(
    () => (revisionId && selected && !collapsed ? api.fileDiff(revisionId, selected) : Promise.resolve(null)),
    [revisionId, selected, collapsed],
  );
  const go = (d: number) => {
    const f = pr.files[index + d];
    if (f) setSelected(f.path);
  };
  const setModeSaved = (m: Mode) => {
    setMode(m);
    try {
      localStorage.setItem("diffMode", m);
    } catch {
      /* per-viewer convenience only */
    }
  };
  const viewedCount = pr.files.filter(isViewed).length;

  const setViewed = async (f: FileEntry, viewed: boolean) => {
    setViewedNow((v) => ({ ...v, [f.path]: viewed }));
    await api.setFileViewed(pr.id, f.path, f.headBlobOid, viewed);
    onViewedChanged();
  };
  /** The first file after the current one (wrapping) not yet viewed. */
  const nextUnviewed = () => {
    for (let k = 1; k < pr.files.length; k++) {
      const f = pr.files[(Math.max(index, 0) + k) % pr.files.length];
      if (!isViewed(f)) return f;
    }
    return null;
  };
  const viewedAndNext = () => {
    if (!entry) return;
    const next = nextUnviewed();
    if (!isViewed(entry)) void setViewed(entry, true);
    if (next) setSelected(next.path);
  };

  const openFind = (query?: string) => {
    setFind((f) => (query === undefined ? { ...f, open: true } : { open: true, query, index: 0 }));
    requestAnimationFrame(() => {
      findInput.current?.focus();
      findInput.current?.select();
    });
  };
  const stepFind = (d: number) => setFind((f) => ({ ...f, open: true, index: f.index + d }));
  const closeFind = () => setFind((f) => ({ ...f, open: false }));
  const findPos = findCount ? (((find.index % findCount) + findCount) % findCount) + 1 : 0;
  useEffect(() => {
    if (!find.open) return;
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement;
      if (e.key === "Escape" && !["INPUT", "TEXTAREA"].includes(t.tagName)) closeFind();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [find.open]);

  const hasDiff = !!diff.data;
  const upcoming = nextUnviewed();
  useCommand("go.nextFile", () => go(1), { enabled: index >= 0 && index < pr.files.length - 1 });
  useCommand("go.prevFile", () => go(-1), { enabled: index > 0 });
  useCommand("go.nextUnviewed", () => upcoming && setSelected(upcoming.path), { enabled: !!upcoming });
  useCommand("file.viewedNext", viewedAndNext, { enabled: !!entry });
  useCommand("file.toggleViewed", () => entry && void setViewed(entry, !isViewed(entry)), {
    enabled: !!entry,
    checked: !!entry && isViewed(entry),
  });
  useCommand("file.comment", () => (editable ? setFileComposer(true) : onNotEditable()), {
    enabled: hasDiff && !fileComposer,
  });
  useCommand("view.unified", () => setModeSaved(mode === "unified" ? "split" : "unified"), {
    checked: mode === "unified",
  });
  useCommand("find.open", () => openFind(), { enabled: hasDiff });
  useCommand("find.next", () => stepFind(1), { enabled: hasDiff && find.query !== "" });
  useCommand("find.previous", () => stepFind(-1), { enabled: hasDiff && find.query !== "" });
  useCommand(
    "find.useSelection",
    () => {
      const text = window.getSelection()?.toString().split("\n")[0].trim();
      if (text) openFind(text);
    },
    { enabled: hasDiff },
  );

  return (
    <div className="files-view">
      <nav className="file-list">
        <div className="small muted file-list-head">
          {viewedCount}/{pr.files.length} viewed
        </div>
        <ul>
          {pr.files.map((f) => (
            <li
              key={f.path}
              className={`${f.path === selected ? "selected" : ""} ${f.generated ? "generated" : ""}`}
              onClick={() => setSelected(f.path)}
            >
              <input
                type="checkbox"
                checked={isViewed(f)}
                title="Viewed"
                onClick={(e) => e.stopPropagation()}
                onChange={() => void setViewed(f, !isViewed(f))}
              />
              <span className={`status ${f.changeType}`}>{STATUS[f.changeType] ?? "?"}</span>
              <span className="path" title={f.prevPath ? `${f.prevPath} → ${f.path}` : f.path}>
                {f.path}
              </span>
              {f.generated && <span className="tag">generated</span>}
              {threadsByPath.get(f.path) && <span className="count">{threadsByPath.get(f.path)}</span>}
              {draftCount.get(f.path) && (
                <span className="count draft" title="Your draft comments">
                  {draftCount.get(f.path)}
                </span>
              )}
              {(f.patchStatus === "too_large" || (f.contentStatus !== "ok" && f.contentStatus !== "binary_skipped")) && (
                <span className="warn-dot" title="Not fully available offline" />
              )}
            </li>
          ))}
        </ul>
      </nav>
      <section className="file-diff">
        <div className="diff-toolbar">
          <button onClick={() => go(-1)} disabled={index <= 0} title={withShortcut("Previous file", "go.prevFile")}>
            ←
          </button>
          <button
            onClick={() => go(1)}
            disabled={index < 0 || index >= pr.files.length - 1}
            title={withShortcut("Next file", "go.nextFile")}
          >
            →
          </button>
          <span className="diff-title">
            {selected}
            {diff.data?.prevPath && <span className="muted"> (from {diff.data.prevPath})</span>}
          </span>
          <span className="spacer" />
          {entry && (
            <label className="toggle viewed-toggle" title={withShortcut("Viewed", "file.toggleViewed")}>
              <input type="checkbox" checked={isViewed(entry)} onChange={() => void setViewed(entry, !isViewed(entry))} />
              Viewed
            </label>
          )}
          {entry && (upcoming || !isViewed(entry)) && (
            <button
              className="primary"
              onClick={viewedAndNext}
              title={withShortcut("Mark this file viewed and open the next unviewed one", "file.viewedNext")}
            >
              {upcoming ? "Viewed, next file →" : "Mark viewed"}
            </button>
          )}
          {entry && !upcoming && isViewed(entry) && <span className="small muted">All files viewed</span>}
          <button
            onClick={() => (editable ? setFileComposer(true) : onNotEditable())}
            disabled={!diff.data || fileComposer}
            title={withShortcut("Comment on the whole file", "file.comment")}
          >
            Comment on file
          </button>
          <div className="segmented">
            <button className={mode === "split" ? "on" : ""} onClick={() => setModeSaved("split")}>
              Split
            </button>
            <button className={mode === "unified" ? "on" : ""} onClick={() => setModeSaved("unified")}>
              Unified
            </button>
          </div>
        </div>
        {find.open && (
          <div className="find-bar" role="search">
            <input
              ref={findInput}
              aria-label="Find in this file"
              placeholder="Find in this file"
              value={find.query}
              onChange={(e) => setFind({ open: true, query: e.target.value, index: 0 })}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  stepFind(e.shiftKey ? -1 : 1);
                } else if (e.key === "Escape") {
                  e.preventDefault();
                  closeFind();
                }
              }}
            />
            <span className="small muted find-count">
              {find.query ? (findCount ? `${findPos} of ${findCount}` : "Not found") : ""}
            </span>
            <button onClick={() => stepFind(-1)} disabled={!findCount} title={withShortcut("Previous match", "find.previous")}>
              ↑
            </button>
            <button onClick={() => stepFind(1)} disabled={!findCount} title={withShortcut("Next match", "find.next")}>
              ↓
            </button>
            <button className="link" onClick={closeFind}>
              Done
            </button>
          </div>
        )}
        {diff.error && <p className="error pad">{diff.error.message}</p>}
        {collapsed && entry && (
          <div className="generated-note pad">
            <p>
              This file is generated, so its diff is hidden.{" "}
              <span className="adds">+{entry.additions}</span> <span className="dels">−{entry.deletions}</span>
            </p>
            <button onClick={() => setShown(new Set(shown).add(entry.path))}>Show diff</button>
          </div>
        )}
        {diff.data && revisionId && (
          <DiffView
            key={`${revisionId}:${selected}`}
            diff={diff.data}
            threads={pr.threads}
            mode={mode}
            prId={pr.id}
            revisionId={revisionId}
            draft={draft}
            editable={editable}
            onDraft={onDraft}
            onNotEditable={onNotEditable}
            fileComposer={fileComposer}
            onFileComposer={setFileComposer}
            find={find.open && find.query ? { query: find.query, index: find.index } : undefined}
            onFindCount={setFindCount}
          />
        )}
        {!selected && <p className="muted pad">This pull request has no changed files.</p>}
      </section>
    </div>
  );
}

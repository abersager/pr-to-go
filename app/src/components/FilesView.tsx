import { useMemo, useState } from "react";
import { api } from "../api";
import type { Mode } from "../diff/rows";
import type { FileEntry, PrDetail } from "../types";
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
}: {
  pr: PrDetail;
  onViewedChanged: () => void;
  file: string | null;
  onFile: (path: string) => void;
}) {
  const [mode, setMode] = useState<Mode>(loadMode);
  const selected = pr.files.some((f) => f.path === file) ? file : (pr.files[0]?.path ?? null);
  const setSelected = onFile;
  const revisionId = pr.revision?.id ?? null;
  const diff = useAsync(
    () => (revisionId && selected ? api.fileDiff(revisionId, selected) : Promise.resolve(null)),
    [revisionId, selected],
  );
  const index = pr.files.findIndex((f) => f.path === selected);
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
  const viewedCount = pr.files.filter((f) => f.viewed).length;
  const threadsByPath = useMemo(() => {
    const m = new Map<string, number>();
    for (const t of pr.threads) if (!t.isOutdated) m.set(t.path, (m.get(t.path) ?? 0) + 1);
    return m;
  }, [pr.threads]);

  const toggleViewed = async (f: FileEntry) => {
    await api.setFileViewed(pr.id, f.path, f.headBlobOid, !f.viewed);
    onViewedChanged();
  };

  return (
    <div className="files-view">
      <nav className="file-list">
        <div className="small muted file-list-head">
          {viewedCount}/{pr.files.length} viewed
        </div>
        <ul>
          {pr.files.map((f) => (
            <li key={f.path} className={f.path === selected ? "selected" : ""} onClick={() => setSelected(f.path)}>
              <input
                type="checkbox"
                checked={f.viewed}
                title="Viewed"
                onClick={(e) => e.stopPropagation()}
                onChange={() => void toggleViewed(f)}
              />
              <span className={`status ${f.changeType}`}>{STATUS[f.changeType] ?? "?"}</span>
              <span className="path" title={f.prevPath ? `${f.prevPath} → ${f.path}` : f.path}>
                {f.path}
              </span>
              {threadsByPath.get(f.path) && <span className="count">{threadsByPath.get(f.path)}</span>}
              {(f.patchStatus === "too_large" || (f.contentStatus !== "ok" && f.contentStatus !== "binary_skipped")) && (
                <span className="warn-dot" title="Not fully available offline" />
              )}
            </li>
          ))}
        </ul>
      </nav>
      <section className="file-diff">
        <div className="diff-toolbar">
          <button onClick={() => go(-1)} disabled={index <= 0} title="Previous file">
            ←
          </button>
          <button onClick={() => go(1)} disabled={index < 0 || index >= pr.files.length - 1} title="Next file">
            →
          </button>
          <span className="diff-title">
            {selected}
            {diff.data?.prevPath && <span className="muted"> (from {diff.data.prevPath})</span>}
          </span>
          <span className="spacer" />
          <div className="segmented">
            <button className={mode === "split" ? "on" : ""} onClick={() => setModeSaved("split")}>
              Split
            </button>
            <button className={mode === "unified" ? "on" : ""} onClick={() => setModeSaved("unified")}>
              Unified
            </button>
          </div>
        </div>
        {diff.error && <p className="error pad">{diff.error.message}</p>}
        {diff.data && <DiffView key={`${revisionId}:${selected}`} diff={diff.data} threads={pr.threads} mode={mode} />}
        {!selected && <p className="muted pad">This pull request has no changed files.</p>}
      </section>
    </div>
  );
}

// Where moved comments go (DESIGN.md §10.4, §11): each comment's proposal
// from the remap engine, with a before/after snippet and a choice.

import { useState } from "react";
import { api } from "../api";
import type { CommentAction, DraftComment, FileDiff, Proposal } from "../types";
import { useAsync } from "../util/useAsync";
import { rangeLabel } from "./DraftCard";

export function proposalOf(c: DraftComment): Proposal | null {
  return (c.remapProposal as Proposal | null) ?? null;
}

/** What to do by default: accept clean moves; the rest need a decision. */
export function defaultAction(p: Proposal | null): CommentAction | undefined {
  if (!p) return undefined;
  if (p.status === "clean") return "remap";
  return undefined;
}

/** The suggested action, applied by "Accept all suggestions". */
export function suggestedAction(p: Proposal | null): CommentAction {
  if (!p) return "to_summary";
  if (p.status === "clean" || p.status === "fuzzy") return "remap";
  if (p.status === "not_commentable" && p.fileInDiff) return "to_file";
  return p.fileInDiff ? "to_file" : "to_summary";
}

function statusText(p: Proposal): string {
  const at = p.line === null ? "" : rangeLabel({ side: p.side, line: p.line, startSide: p.startSide, startLine: p.startLine });
  switch (p.status) {
    case "clean":
      return `Unchanged; now at ${at}`;
    case "fuzzy":
      return `Changed; best match at ${at} (${Math.round(p.confidence * 100)}% similar)`;
    case "not_commentable":
      return `Unchanged, but no longer part of the diff`;
    case "orphaned":
      return p.fileInDiff ? "Those lines are gone" : "The file is no longer part of this pull request";
  }
}

const LABEL: Record<CommentAction, string> = {
  remap: "Move there",
  to_file: "Make it a file comment",
  to_summary: "Move into the summary",
  drop: "Drop it",
  keep: "Keep",
};

export function ProposalRow({
  comment,
  action,
  onAction,
}: {
  comment: DraftComment;
  action: CommentAction | undefined;
  onAction: (a: CommentAction) => void;
}) {
  const p = proposalOf(comment);
  const options: CommentAction[] = [];
  if (p && p.line !== null && (p.status === "clean" || p.status === "fuzzy")) options.push("remap");
  if (!p || p.fileInDiff) options.push("to_file");
  options.push("to_summary", "drop");
  const first = comment.bodyMd.split("\n").find((l) => l.trim() && !l.startsWith("```")) ?? "";
  return (
    <div className={`proposal ${p?.status ?? "orphaned"}`}>
      <div className="small">
        <code>{comment.path}</code> <span className="muted">{rangeLabel(comment)}</span> — “{first.slice(0, 80)}”
      </div>
      {p && (
        <>
          <div className={`proposal-status small ${p.status}`}>{statusText(p)}</div>
          {(p.oldLines.length > 0 || p.newLines.length > 0) && p.status !== "clean" && (
            <pre className="hunk-snippet">
              {p.oldLines.map((l, i) => (
                <div key={`o${i}`} className="del">
                  -{l}
                </div>
              ))}
              {p.newLines.map((l, i) => (
                <div key={`n${i}`} className="add">
                  +{l}
                </div>
              ))}
            </pre>
          )}
          {p.suggestionStale && (
            <div className="small warn-text">Its suggested change was written for code that has since changed.</div>
          )}
        </>
      )}
      <div className="proposal-actions">
        {options.map((o) => (
          <label key={o} className="choice inline">
            <input type="radio" name={`c${comment.id}`} checked={action === o} onChange={() => onAction(o)} />
            {LABEL[o]}
          </label>
        ))}
      </div>
    </div>
  );
}

const CHANGE_LABEL: Record<string, string> = {
  changed: "changed",
  newly_changed: "now changed by this pull request",
  no_longer_changed: "no longer changed by this pull request",
  unknown: "joined or left the pull request (contents not synced)",
};

/** A diff rendered simply, for read-only views like the interdiff. */
export function SimpleDiff({ diff }: { diff: FileDiff }) {
  if (diff.baseBinary || diff.headBinary) return <p className="small muted">Binary file changed.</p>;
  if (diff.hunks.length === 0) return null;
  return (
    <pre className="simple-diff">
      {diff.hunks.map((h, i) => (
        <div key={i}>
          <div className="hdr">
            @@ -{h.oldStart},{h.oldLen} +{h.newStart},{h.newLen} @@
          </div>
          {h.lines.map((l, j) => (
            <div key={j} className={l.kind}>
              {l.kind === "add" ? "+" : l.kind === "del" ? "-" : " "}
              {l.text}
            </div>
          ))}
        </div>
      ))}
    </pre>
  );
}

/** What changed between the reviewed version and now. */
export function Interdiff({ from, to, rebased }: { from: number; to: number; rebased?: boolean }) {
  const [open, setOpen] = useState(false);
  const diffs = useAsync(() => (open ? api.interdiff(from, to) : Promise.resolve(null)), [open, from, to]);
  return (
    <details className="interdiff" onToggle={(e) => setOpen((e.target as HTMLDetailsElement).open)}>
      <summary className="small">View changes since your review</summary>
      {rebased && (
        <p className="small muted">The base branch moved too, so this also includes changes from the base branch.</p>
      )}
      {diffs.data?.length === 0 && <p className="small muted">No file contents changed.</p>}
      {diffs.data?.map((d) => (
        <div key={d.path}>
          <div className="small">
            <code>{d.path}</code> <span className="muted">{CHANGE_LABEL[d.changeType] ?? d.changeType}</span>
          </div>
          <SimpleDiff diff={d} />
        </div>
      ))}
    </details>
  );
}

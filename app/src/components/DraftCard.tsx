import type { DraftComment } from "../types";
import { renderMarkdown } from "../util/markdown";

export function rangeLabel(c: Pick<DraftComment, "side" | "line" | "startSide" | "startLine">): string {
  const side = (s: string | null) => (s === "LEFT" ? "L" : "R");
  if (c.line === null) return "";
  if (c.startLine === null) return `line ${c.line}`;
  if ((c.startSide ?? c.side) === c.side) return `lines ${c.startLine}–${c.line}`;
  return `lines ${side(c.startSide)}${c.startLine}–${side(c.side)}${c.line}`;
}

export function DraftCard({
  comment,
  original,
  onEdit,
  onDelete,
}: {
  comment: DraftComment;
  original?: string[];
  onEdit?: () => void;
  onDelete?: () => void;
}) {
  return (
    <div className="draft-card">
      <div className="draft-head small">
        <span className="badge pending">Draft</span>
        <span className="muted">{comment.kind === "reply" ? "reply" : rangeLabel(comment)}</span>
        <span className="spacer" />
        {onEdit && (
          <button className="link" onClick={onEdit}>
            Edit
          </button>
        )}
        {onDelete && (
          <button className="link" onClick={onDelete}>
            Delete
          </button>
        )}
      </div>
      <div className="markdown" dangerouslySetInnerHTML={{ __html: renderMarkdown(comment.bodyMd, original) }} />
    </div>
  );
}

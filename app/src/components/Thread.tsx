import type { ThreadEntry } from "../types";
import { ago } from "../util/format";
import { Html } from "./Html";

export function DiffHunkSnippet({ hunk }: { hunk: string }) {
  // GitHub's diffHunk ends at the commented line; show the last few lines.
  const lines = hunk.split("\n").slice(-6);
  return (
    <pre className="hunk-snippet">
      {lines.map((l, i) => (
        <div key={i} className={l.startsWith("+") ? "add" : l.startsWith("-") ? "del" : l.startsWith("@@") ? "hdr" : ""}>
          {l || " "}
        </div>
      ))}
    </pre>
  );
}

export function ThreadView({
  thread,
  showContext = false,
  footer,
}: {
  thread: ThreadEntry;
  showContext?: boolean;
  footer?: React.ReactNode;
}) {
  const first = thread.comments[0];
  return (
    <div className={`thread ${thread.isResolved ? "resolved" : ""}`}>
      {(thread.isOutdated || thread.isResolved) && (
        <div className="thread-flags small">
          {thread.isOutdated && <span className="badge">Outdated</span>}
          {thread.isResolved && <span className="badge">Resolved</span>}
        </div>
      )}
      {showContext && first?.diffHunk && <DiffHunkSnippet hunk={first.diffHunk} />}
      {thread.comments.map((c) => (
        <div key={c.nodeId} className="comment">
          <div className="comment-head small">
            <strong>{c.author ?? "ghost"}</strong> <span className="muted">{ago(c.createdAt)}</span>
            {c.state === "PENDING" && <span className="badge">Pending on github.com</span>}
          </div>
          <Html html={c.bodyHtml} />
        </div>
      ))}
      {footer}
    </div>
  );
}

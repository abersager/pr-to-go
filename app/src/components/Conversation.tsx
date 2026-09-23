import type { Draft, PrDetail } from "../types";
import { ago, short } from "../util/format";
import { Html, openExternal } from "./Html";
import { ReplyArea } from "./ReplyArea";
import { ThreadView } from "./Thread";

const REVIEW_LABEL: Record<string, string> = {
  APPROVED: "approved",
  CHANGES_REQUESTED: "requested changes",
  COMMENTED: "reviewed",
  DISMISSED: "reviewed (dismissed)",
  PENDING: "has a pending review",
};

export function Conversation({
  pr,
  draft,
  editable,
  onDraft,
  onNotEditable,
  onOpenReview,
}: {
  pr: PrDetail;
  draft: Draft | null;
  editable: boolean;
  onDraft: (d: Draft | null) => void;
  onNotEditable: () => void;
  onOpenReview: () => void;
}) {
  type Item =
    | { kind: "review"; at: string; key: string; el: React.ReactNode }
    | { kind: "comment"; at: string; key: string; el: React.ReactNode };
  const items: Item[] = [
    ...pr.reviews.map((r) => ({
      kind: "review" as const,
      at: r.submittedAt ?? "9999",
      key: r.nodeId,
      el: (
        <div className="timeline-item">
          <div className="small">
            <strong>{r.author ?? "ghost"}</strong>{" "}
            <span className={`review-state ${r.state.toLowerCase()}`}>{REVIEW_LABEL[r.state] ?? r.state}</span>{" "}
            <span className="muted">
              {ago(r.submittedAt)} {r.commitOid && `on ${short(r.commitOid)}`}
            </span>
          </div>
          {r.bodyHtml.trim() && <Html html={r.bodyHtml} />}
        </div>
      ),
    })),
    ...pr.issueComments.map((c) => ({
      kind: "comment" as const,
      at: c.createdAt,
      key: c.nodeId,
      el: (
        <div className="timeline-item">
          <div className="small">
            <strong>{c.author ?? "ghost"}</strong> <span className="muted">commented {ago(c.createdAt)}</span>
          </div>
          <Html html={c.bodyHtml} />
        </div>
      ),
    })),
  ].sort((a, b) => a.at.localeCompare(b.at));

  const checks = pr.checks;
  return (
    <div className="conversation">
      <section className="description">
        <Html html={pr.bodyHtml} />
      </section>

      {pr.revision && pr.revision.partialReasons.length > 0 && (
        <section className="notice warn">
          <strong>Some files aren't fully available offline:</strong>
          <ul>
            {pr.revision.partialReasons.map((r) => (
              <li key={r.path}>
                <code>{r.path}</code>: {r.reason}
              </li>
            ))}
          </ul>
        </section>
      )}

      {checks && (
        <section className="checks">
          <h3>
            Checks <span className={`rollup ${(checks.rollupState ?? "").toLowerCase()}`}>{checks.rollupState ?? "none"}</span>{" "}
            <span className="muted small">as of {ago(checks.capturedAt)}</span>
          </h3>
          <ul>
            {checks.contexts.map((c, i) => (
              <li key={i}>
                <span className={`check ${(c.conclusion ?? c.status).toLowerCase()}`} />
                {c.url ? (
                  <a href={c.url} onClick={(e) => (e.preventDefault(), openExternal(c.url!))}>
                    {c.name}
                  </a>
                ) : (
                  c.name
                )}{" "}
                <span className="muted small">{(c.conclusion ?? c.status).toLowerCase()}</span>
              </li>
            ))}
          </ul>
        </section>
      )}

      <section className="commits">
        <h3>Commits</h3>
        <ul>
          {pr.commits.map((c) => (
            <li key={c.oid}>
              <code>{short(c.oid)}</code> {c.headline} <span className="muted small">{c.author}</span>
            </li>
          ))}
        </ul>
      </section>

      <section className="timeline">
        <h3>Activity</h3>
        {draft && (
          <div className="timeline-item draft-review-card" onClick={onOpenReview}>
            <div className="small">
              <span className="badge pending">Your review</span>{" "}
              <span className="muted">
                {draft.status === "draft" ? "draft" : draft.status.replace("_", " ")} ·{" "}
                {draft.comments.length} {draft.comments.length === 1 ? "comment" : "comments"}
                {draft.verdict && draft.verdict !== "COMMENT" && ` · ${draft.verdict === "APPROVE" ? "approve" : "request changes"}`}
              </span>
            </div>
            {draft.bodyMd.trim() && <div className="small draft-snippet">{draft.bodyMd.split("\n")[0]}</div>}
          </div>
        )}
        {items.length === 0 && <p className="muted">No reviews or comments yet.</p>}
        {items.map((i) => (
          <div key={i.key}>{i.el}</div>
        ))}
      </section>

      {pr.threads.length > 0 && (
        <section className="threads">
          <h3>Review threads</h3>
          {pr.threads.map((t) => (
            <div key={t.nodeId} className="thread-block">
              <div className="small muted">
                <code>{t.path}</code>
                {t.subjectType === "LINE" && `:${t.line ?? t.originalLine}`}
              </div>
              <ThreadView
                thread={t}
                showContext
                footer={
                  pr.revision && (
                    <ReplyArea
                      prId={pr.id}
                      revisionId={pr.revision.id}
                      thread={t}
                      draft={draft}
                      editable={editable}
                      onDraft={onDraft}
                      onNotEditable={onNotEditable}
                    />
                  )
                }
              />
            </div>
          ))}
        </section>
      )}
    </div>
  );
}

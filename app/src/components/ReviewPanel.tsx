import { useEffect, useRef, useState } from "react";
import { api } from "../api";
import type { Draft, DraftStatus, PrDetail, Verdict } from "../types";
import { ago } from "../util/format";
import { renderMarkdown } from "../util/markdown";
import { rangeLabel } from "./DraftCard";
import { openExternal } from "./Html";

const STATUS_TEXT: Record<DraftStatus, string> = {
  draft: "Draft — only on this computer",
  queued: "Queued — sent automatically when you're online",
  preflight: "Checking the pull request before sending…",
  needs_attention: "Needs your attention before it can be sent",
  staging: "Sending…",
  submitting: "Sending…",
  submitted: "Sent",
  discarded: "Discarded",
};

const VERDICTS: { value: Verdict; label: string; hint: string }[] = [
  { value: "COMMENT", label: "Comment", hint: "General feedback without approving" },
  { value: "APPROVE", label: "Approve", hint: "Approve merging these changes" },
  { value: "REQUEST_CHANGES", label: "Request changes", hint: "Feedback that must be addressed" },
];

/** The first line of prose in a comment, skipping code fences. */
function snippet(body: string): string {
  const line = body.split("\n").find((l) => l.trim() && !l.trim().startsWith("```"));
  return line ?? (body.includes("```suggestion") ? "Suggested change" : "");
}

export function ReviewPanel({
  pr,
  draft,
  onDraft,
  onClose,
  onOpenFile,
}: {
  pr: PrDetail;
  draft: Draft | null;
  onDraft: (d: Draft | null) => void;
  onClose: () => void;
  onOpenFile: (path: string) => void;
}) {
  const [summary, setSummary] = useState(draft?.bodyMd ?? "");
  const [preview, setPreview] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const timer = useRef<number | undefined>(undefined);
  const chain = useRef<Promise<void>>(Promise.resolve());
  const status: DraftStatus = draft?.status ?? "draft";
  const editable = status === "draft";
  // Optimistic, so the radio doesn't flick back while the change is saved.
  const [pendingVerdict, setPendingVerdict] = useState<Verdict | null>(null);
  const verdict = pendingVerdict ?? draft?.verdict ?? "COMMENT";

  useEffect(() => {
    // Follow changes made elsewhere, unless the user is typing here.
    if (timer.current === undefined) setSummary(draft?.bodyMd ?? "");
  }, [draft?.bodyMd]);
  useEffect(() => () => window.clearTimeout(timer.current), []);

  const run = async (f: () => Promise<Draft | null | void>) => {
    setBusy(true);
    setError(null);
    try {
      await chain.current;
      const d = await f();
      if (d !== undefined) onDraft(d);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const saveSummary = (text: string) => {
    setSummary(text);
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => {
      timer.current = undefined;
      chain.current = chain.current.then(() =>
        api.updateDraftReview(pr.id, { body: text }).then(onDraft, (e: Error) => setError(e.message)),
      );
    }, 400);
  };

  const flushSummary = async () => {
    if (timer.current !== undefined) {
      window.clearTimeout(timer.current);
      timer.current = undefined;
      chain.current = chain.current.then(() => api.updateDraftReview(pr.id, { body: summary }).then(onDraft));
    }
    await chain.current;
  };

  const comments = draft?.comments ?? [];
  const own = pr.viewerDidAuthor;

  return (
    <aside className="review-panel">
      <div className="review-panel-head">
        <h2>Your review</h2>
        <span className="spacer" />
        <button className="link" onClick={onClose}>
          Close
        </button>
      </div>
      <p className={`review-status ${status}`}>{STATUS_TEXT[status]}</p>
      {draft?.lastError && status !== "draft" && (
        <p className="error small">
          Last attempt failed{draft.nextAttemptAt ? `; retrying ${ago(draft.nextAttemptAt)}` : ""}: {draft.lastError}
        </p>
      )}
      {status === "submitted" && draft?.submittedUrl && (
        <p>
          <a href={draft.submittedUrl} onClick={(e) => (e.preventDefault(), openExternal(draft.submittedUrl!))}>
            View on GitHub
          </a>
        </p>
      )}

      <section>
        <div className="panel-label">
          <span>Summary</span>
          <span className="spacer" />
          <div className="segmented small">
            <button className={preview ? "" : "on"} onClick={() => setPreview(false)}>
              Write
            </button>
            <button className={preview ? "on" : ""} onClick={() => setPreview(true)}>
              Preview
            </button>
          </div>
        </div>
        {preview ? (
          <div
            className="markdown composer-preview"
            dangerouslySetInnerHTML={{ __html: renderMarkdown(summary || "_No summary._") }}
          />
        ) : (
          <textarea
            value={summary}
            rows={6}
            disabled={!editable}
            placeholder="Overall feedback (optional for comments and approvals)"
            onChange={(e) => saveSummary(e.target.value)}
          />
        )}
      </section>

      <section>
        <div className="panel-label">Verdict</div>
        {VERDICTS.map((v) => {
          const blocked = own && v.value !== "COMMENT";
          return (
            <label key={v.value} className={`verdict ${blocked ? "muted" : ""}`} title={blocked ? "Not on your own pull request" : v.hint}>
              <input
                type="radio"
                name="verdict"
                checked={verdict === v.value}
                disabled={!editable || blocked}
                onChange={() => {
                  setPendingVerdict(v.value);
                  void run(() => api.updateDraftReview(pr.id, { verdict: v.value })).finally(() =>
                    setPendingVerdict(null),
                  );
                }}
              />
              {v.label}
            </label>
          );
        })}
      </section>

      <section>
        <div className="panel-label">
          {comments.length === 0 ? "No comments yet" : `${comments.length} ${comments.length === 1 ? "comment" : "comments"}`}
        </div>
        <ul className="draft-list">
          {comments.map((c) => (
            <li key={c.id} onClick={() => c.path && onOpenFile(c.path)}>
              <code>{c.path}</code>{" "}
              <span className="muted small">
                {c.kind === "reply" ? "reply" : c.subjectType === "FILE" ? "file" : rangeLabel(c)}
              </span>
              <div className="small draft-snippet">{snippet(c.bodyMd)}</div>
            </li>
          ))}
        </ul>
      </section>

      {error && <p className="error small">{error}</p>}
      <div className="review-actions">
        {status === "draft" && (
          <button
            className="primary"
            disabled={busy}
            onClick={() =>
              void run(async () => {
                await flushSummary();
                return api.queueReview(pr.id);
              })
            }
          >
            Submit review
          </button>
        )}
        {(status === "queued" || status === "needs_attention") && (
          <button disabled={busy} onClick={() => void run(() => api.unqueueReview(pr.id))}>
            Edit review
          </button>
        )}
        {draft && ["draft", "queued", "needs_attention"].includes(status) && (
          <button
            disabled={busy}
            onClick={() => {
              if (window.confirm("Discard this review and all its comments?")) {
                void run(async () => {
                  await api.discardReview(pr.id);
                  return null;
                });
              }
            }}
          >
            Discard
          </button>
        )}
      </div>
      {status === "draft" && (
        <p className="muted small">Submitting works offline too: the review waits in the outbox until you're connected.</p>
      )}
    </aside>
  );
}

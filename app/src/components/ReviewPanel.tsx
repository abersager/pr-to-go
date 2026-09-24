import { useEffect, useRef, useState } from "react";
import { api, copyText } from "../api";
import { type CommandId, runCommand, useCommand, withShortcut } from "../commands";
import type { Draft, DraftStatus, PrDetail, Verdict } from "../types";
import { ago, until } from "../util/format";
import { AttentionView } from "./AttentionView";
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

const VERDICT_COMMANDS: [Verdict, CommandId][] = [
  ["COMMENT", "review.verdictComment"],
  ["APPROVE", "review.verdictApprove"],
  ["REQUEST_CHANGES", "review.verdictRequestChanges"],
];

const SENDING: DraftStatus[] = ["queued", "preflight", "staging", "submitting"];

/** Which review actions make sense right now (for buttons and the menu). */
export function reviewAvailability(pr: PrDetail | null, draft: Draft | null) {
  const status: DraftStatus = draft?.status ?? "draft";
  const editable = status === "draft";
  const sending = SENDING.includes(status);
  return {
    verdict: (v: Verdict) => !!pr && editable && (!pr.viewerDidAuthor || v === "COMMENT"),
    submit: !!draft && editable,
    edit: !!draft && sending,
    retry: !!draft && sending && !!draft.lastError,
    copy: !!draft && status !== "submitted",
    discard: !!draft && status !== "submitted" && status !== "discarded",
  };
}

/** Review commands and when each is available. */
export function reviewCommands(pr: PrDetail | null, draft: Draft | null) {
  const a = reviewAvailability(pr, draft);
  const verdict = draft?.verdict ?? "COMMENT";
  return [
    ...VERDICT_COMMANDS.map(([v, id]) => ({ id, enabled: a.verdict(v), checked: verdict === v })),
    { id: "review.submit" as CommandId, enabled: a.submit, checked: false },
    { id: "review.edit" as CommandId, enabled: a.edit, checked: false },
    { id: "review.retry" as CommandId, enabled: a.retry, checked: false },
    { id: "review.copy" as CommandId, enabled: a.copy, checked: false },
    { id: "review.discard" as CommandId, enabled: a.discard, checked: false },
  ];
}

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
  command = null,
  onCommandDone,
}: {
  pr: PrDetail;
  draft: Draft | null;
  onDraft: (d: Draft | null) => void;
  onClose: () => void;
  onOpenFile: (path: string) => void;
  /** A menu command to run once the panel is up (it was closed). */
  command?: CommandId | null;
  onCommandDone?: () => void;
}) {
  const [summary, setSummary] = useState(draft?.bodyMd ?? "");
  const [preview, setPreview] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState(false);
  const [confirmDiscard, setConfirmDiscard] = useState(false);
  const confirmRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (confirmDiscard) confirmRef.current?.scrollIntoView({ block: "nearest" });
  }, [confirmDiscard]);
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
  const avail = reviewAvailability(pr, draft);

  const setVerdict = (v: Verdict) => {
    setPendingVerdict(v);
    void run(() => api.updateDraftReview(pr.id, { verdict: v })).finally(() => setPendingVerdict(null));
  };
  const submit = () =>
    void run(async () => {
      await flushSummary();
      return api.queueReview(pr.id);
    });
  const unqueue = () => void run(() => api.unqueueReview(pr.id));
  const retry = () => void run(async () => api.retryReview(pr.id));
  const copy = () =>
    void run(async () => {
      const md = await api.exportReviewMarkdown(pr.id);
      await copyText(md);
      setError(null);
      setCopied(true);
    });

  for (const [v, id] of VERDICT_COMMANDS) {
    // A fixed list, so the hooks are always called in the same order.
    useCommand(id, () => setVerdict(v), { enabled: avail.verdict(v) && !busy, checked: verdict === v });
  }
  useCommand("review.submit", submit, { enabled: avail.submit && !busy });
  useCommand("review.edit", unqueue, { enabled: avail.edit && !busy });
  useCommand("review.retry", retry, { enabled: avail.retry && !busy });
  useCommand("review.copy", copy, { enabled: avail.copy && !busy });
  useCommand("review.discard", () => setConfirmDiscard(true), { enabled: avail.discard && !busy });
  useEffect(() => {
    if (command) {
      runCommand(command);
      onCommandDone?.();
    }
  }, [command, onCommandDone]);
  // No draft in progress: show the last review that went out.
  const sent = !draft && pr.lastReview ? pr.lastReview : null;

  return (
    <aside className="review-panel">
      <div className="review-panel-head">
        <h2>Your review</h2>
        <span className="spacer" />
        <button className="link" onClick={onClose}>
          Close
        </button>
      </div>
      {sent ? (
        <div className="review-status submitted">
          Sent {ago(sent.submittedAt)}
          {sent.submittedUrl && (
            <>
              {" · "}
              <a href={sent.submittedUrl} onClick={(e) => (e.preventDefault(), openExternal(sent.submittedUrl!))}>
                View on GitHub
              </a>
            </>
          )}
          <div className="small muted">Anything you write now starts a new review.</div>
        </div>
      ) : (
        <p className={`review-status ${status}`}>{STATUS_TEXT[status]}</p>
      )}
      {draft?.lastError && ["queued", "preflight", "staging", "submitting"].includes(status) && (
        <p className="small retry-line">
          {draft.lastErrorKind === "offline" ? (
            <span className="muted">Waiting for a connection.</span>
          ) : (
            <span className="error">
              Last attempt failed{draft.nextAttemptAt ? `; trying again ${until(draft.nextAttemptAt)}` : ""}:{" "}
              {draft.lastError}
            </span>
          )}{" "}
          <button className="link" onClick={retry}>
            Try now
          </button>
        </p>
      )}
      {status === "needs_attention" && draft && (
        <AttentionView
          pr={pr}
          draft={draft}
          onDraft={onDraft}
          onEdit={unqueue}
          onSignInAgain={() => void api.signOut().then(() => window.location.reload())}
        />
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
        {own && (
          <p className="small muted own-pr-note">
            This is your own pull request. GitHub doesn't let authors approve it or request changes, so your review
            goes out as comments.
          </p>
        )}
        {VERDICTS.map((v) => {
          const blocked = own && v.value !== "COMMENT";
          return (
            <label
              key={v.value}
              className={`verdict ${blocked ? "muted" : ""}`}
              title={blocked ? "Not available on your own pull request" : v.hint}
            >
              <input
                type="radio"
                name="verdict"
                checked={verdict === v.value}
                disabled={!editable || blocked}
                onChange={() => setVerdict(v.value)}
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
          <button className="primary" disabled={busy} onClick={submit} title={withShortcut("Submit review", "review.submit")}>
            Submit review
          </button>
        )}
        {avail.edit && (
          <button disabled={busy} onClick={unqueue}>
            Edit review
          </button>
        )}
        {avail.copy && (
          <button disabled={busy} title={withShortcut("Copy the whole review as Markdown", "review.copy")} onClick={copy}>
            {copied ? "Copied" : "Copy as Markdown"}
          </button>
        )}
        {avail.discard && !confirmDiscard && (
          <button disabled={busy} onClick={() => setConfirmDiscard(true)}>
            Discard…
          </button>
        )}
      </div>
      {confirmDiscard && draft && (
        <div
          ref={confirmRef}
          className="notice warn confirm-discard"
          role="alertdialog"
          aria-label="Discard review"
          onKeyDown={(e) => e.key === "Escape" && setConfirmDiscard(false)}
        >
          <p>
            Discard this review{comments.length > 0 ? ` and its ${comments.length === 1 ? "comment" : `${comments.length} comments`}` : ""}?
            This can't be undone.
          </p>
          <div className="review-actions">
            <button
              className="danger"
              disabled={busy}
              onClick={() =>
                void run(async () => {
                  await api.discardReview(pr.id);
                  setConfirmDiscard(false);
                  return null;
                })
              }
            >
              Discard review
            </button>
            <button disabled={busy} autoFocus onClick={() => setConfirmDiscard(false)}>
              Keep it
            </button>
          </div>
        </div>
      )}
      {status === "draft" && (
        <p className="muted small">Submitting works offline too: the review waits in the outbox until you're connected.</p>
      )}
    </aside>
  );
}

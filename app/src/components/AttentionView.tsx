// What to do when a queued review can't go as it is (DESIGN.md §10.3–10.4).
// Every reason gets a decision, then one resolution is sent and the outbox
// checks everything again before anything reaches GitHub.

import { useMemo, useState } from "react";
import { api } from "../api";
import type { CommentAction, CommentResolution, Draft, DraftComment, PrDetail, Reason, Verdict } from "../types";
import { rangeLabel } from "./DraftCard";
import { openExternal } from "./Html";

const ACTION_LABEL: Record<CommentAction, string> = {
  remap: "Move to a new line",
  to_file: "Make it a file comment",
  to_summary: "Move into the summary",
  drop: "Drop it",
  keep: "Keep",
};

function commentLabel(c: DraftComment | undefined): string {
  if (!c) return "a comment";
  const where = c.kind === "reply" ? "reply" : c.subjectType === "FILE" ? "file comment" : rangeLabel(c);
  const first = c.bodyMd.split("\n").find((l) => l.trim() && !l.startsWith("```")) ?? "";
  return `${c.path ?? ""} ${where}: “${first.slice(0, 60)}${first.length > 60 ? "…" : ""}”`;
}

function ActionPicker({
  value,
  options,
  onChange,
}: {
  value: CommentAction | undefined;
  options: CommentAction[];
  onChange: (a: CommentAction) => void;
}) {
  return (
    <select value={value ?? ""} onChange={(e) => onChange(e.target.value as CommentAction)}>
      <option value="" disabled>
        Choose…
      </option>
      {options.map((o) => (
        <option key={o} value={o}>
          {ACTION_LABEL[o]}
        </option>
      ))}
    </select>
  );
}

export function AttentionView({
  pr,
  draft,
  onDraft,
  onEdit,
  onSignInAgain,
}: {
  pr: PrDetail;
  draft: Draft;
  onDraft: (d: Draft) => void;
  onEdit: () => void;
  onSignInAgain: () => void;
}) {
  const reasons = ((draft.attention as { reasons?: Reason[] } | null)?.reasons ?? []) as Reason[];
  const byId = useMemo(() => new Map(draft.comments.map((c) => [c.id, c])), [draft.comments]);
  const [ack, setAck] = useState<Set<string>>(new Set());
  const [target, setTarget] = useState<"current_head" | "reviewed_commit" | null>(null);
  const [actions, setActions] = useState<Record<number, CommentAction>>({});
  const [verdict, setVerdict] = useState<Verdict | "keep" | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const toggleAck = (key: string, on: boolean) =>
    setAck((a) => {
      const next = new Set(a);
      if (on) next.add(key);
      else next.delete(key);
      return next;
    });
  const setAction = (id: number, a: CommentAction) => setActions((x) => ({ ...x, [id]: a }));

  const headMoved = reasons.find((r) => r.kind === "head_moved");
  const blocking = reasons.filter((r) => r.kind === "new_blocking_review");
  const needsVerdict = (headMoved?.kind === "head_moved" && headMoved.verdict_stale) || blocking.length > 0;

  // Which comments need an action, and which actions make sense for each.
  const commentChoices: { id: number; options: CommentAction[] }[] = [];
  for (const r of reasons) {
    if (r.kind === "head_moved" && target === "current_head") {
      for (const id of r.comments) commentChoices.push({ id, options: ["to_file", "to_summary", "drop"] });
    }
    if (r.kind === "reply_target_gone") commentChoices.push({ id: r.comment, options: ["to_summary", "drop"] });
    if (r.kind === "comment_rejected" && r.comment !== null) {
      const c = byId.get(r.comment);
      commentChoices.push({
        id: r.comment,
        options: c?.kind === "reply" ? ["to_summary", "drop"] : ["to_file", "to_summary", "drop"],
      });
    }
  }

  const blockers = reasons.filter((r) =>
    ["auth", "permission", "pr_gone", "repo_archived", "reviewed_commit_unavailable"].includes(r.kind),
  );
  const ackKeys = reasons.flatMap((r) =>
    r.kind === "pr_state_changed"
      ? [`pr_state:${r.state}`]
      : r.kind === "pr_locked"
        ? ["pr_locked"]
        : r.kind === "new_blocking_review"
          ? [`blocking:${r.review}`]
          : r.kind === "existing_pending_review"
            ? [`merge_pending:${r.review}`]
            : [],
  );
  const ready =
    blockers.every((b) => b.kind === "permission" || b.kind === "reviewed_commit_unavailable") &&
    ackKeys.every((k) => ack.has(k)) &&
    (!headMoved || target !== null) &&
    (!needsVerdict || verdict !== null) &&
    commentChoices.every((c) => actions[c.id] !== undefined);

  const submit = async () => {
    setBusy(true);
    setError(null);
    try {
      const comments: CommentResolution[] = commentChoices.map((c) => ({ id: c.id, action: actions[c.id] }));
      const d = await api.resolveReview(pr.id, {
        acknowledge: [...ack],
        targetMode: target ?? undefined,
        comments,
        verdict: verdict === null || verdict === "keep" ? undefined : verdict,
      });
      onDraft(d);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="attention">
      {reasons.map((r, i) => (
        <div key={i} className="attention-card">
          {r.kind === "head_moved" && (
            <>
              <p>
                <strong>The pull request changed since you reviewed it.</strong>{" "}
                {r.comments.length > 0 &&
                  `${r.comments.length} of your comments ${r.comments.length === 1 ? "was" : "were"} written on the earlier version.`}
              </p>
              <label className="choice">
                <input type="radio" name="target" checked={target === "reviewed_commit"} onChange={() => setTarget("reviewed_commit")} />
                Keep every comment where I wrote it (post on the version I reviewed; GitHub shows changed lines as outdated)
              </label>
              <label className="choice">
                <input type="radio" name="target" checked={target === "current_head"} onChange={() => setTarget("current_head")} />
                Send to the new version, and decide per comment
              </label>
              {r.verdict_stale && (
                <p className="small">
                  Your {draft.verdict === "APPROVE" ? "approval" : "request for changes"} was for the earlier version.
                </p>
              )}
            </>
          )}
          {r.kind === "pr_state_changed" && (
            <label className="choice">
              <input type="checkbox" checked={ack.has(`pr_state:${r.state}`)} onChange={(e) => toggleAck(`pr_state:${r.state}`, e.target.checked)} />
              The pull request was {r.state.toLowerCase()}. Send the review anyway.
            </label>
          )}
          {r.kind === "pr_locked" && (
            <label className="choice">
              <input type="checkbox" checked={ack.has("pr_locked")} onChange={(e) => toggleAck("pr_locked", e.target.checked)} />
              The conversation is locked. Try sending anyway (only collaborators can comment).
            </label>
          )}
          {r.kind === "new_blocking_review" && (
            <>
              <p>
                <strong>{r.author ?? "Someone"} requested changes</strong> after you drafted your approval.
              </p>
              <label className="choice">
                <input type="checkbox" checked={ack.has(`blocking:${r.review}`)} onChange={(e) => toggleAck(`blocking:${r.review}`, e.target.checked)} />
                I've read it
              </label>
            </>
          )}
          {r.kind === "existing_pending_review" && (
            <label className="choice">
              <input type="checkbox" checked={ack.has(`merge_pending:${r.review}`)} onChange={(e) => toggleAck(`merge_pending:${r.review}`, e.target.checked)} />
              You have a pending review on github.com. Add these comments to it and submit them together.
            </label>
          )}
          {r.kind === "reply_target_gone" && <p>The thread you replied to was deleted or can't take replies.</p>}
          {r.kind === "comment_rejected" && (
            <p>
              GitHub refused {r.comment !== null ? "a comment" : "the review"}: <em>{r.message}</em>
            </p>
          )}
          {r.kind === "reviewed_commit_unavailable" && (
            <p>
              GitHub no longer accepts the version you reviewed (it was force-pushed away). Choose Edit review and send to
              the new version instead.
            </p>
          )}
          {r.kind === "auth" && (
            <p>
              GitHub rejected your token. <button onClick={onSignInAgain}>Sign in again</button> — the review stays
              queued.
            </p>
          )}
          {r.kind === "permission" && (
            <p>
              GitHub refused: {r.message}{" "}
              {r.sso_url && (
                <button onClick={() => openExternal(r.sso_url!)}>Authorize for this organization</button>
              )}
            </p>
          )}
          {r.kind === "pr_gone" && <p>The pull request no longer exists or you lost access to it.</p>}
          {r.kind === "repo_archived" && <p>The repository was archived, so it can't take reviews.</p>}
        </div>
      ))}

      {commentChoices.length > 0 && (
        <div className="attention-card">
          <p className="small">
            <strong>Your comments</strong>
          </p>
          {commentChoices.map((c) => (
            <div key={c.id} className="comment-choice">
              <div className="small">{commentLabel(byId.get(c.id))}</div>
              <ActionPicker value={actions[c.id]} options={c.options} onChange={(a) => setAction(c.id, a)} />
            </div>
          ))}
        </div>
      )}

      {needsVerdict && (
        <div className="attention-card">
          <p className="small">
            <strong>Your verdict</strong>
          </p>
          <label className="choice">
            <input type="radio" name="verdict" checked={verdict === "keep"} onChange={() => setVerdict("keep")} />
            Keep it: {draft.verdict === "APPROVE" ? "approve" : draft.verdict === "REQUEST_CHANGES" ? "request changes" : "comment"}
          </label>
          <label className="choice">
            <input type="radio" name="verdict" checked={verdict === "COMMENT"} onChange={() => setVerdict("COMMENT")} />
            Change it to a plain comment
          </label>
        </div>
      )}

      {error && <p className="error small">{error}</p>}
      <div className="review-actions">
        {(ready || blockers.some((b) => b.kind === "permission")) && reasons.length > 0 && (
          <button className="primary" disabled={busy || !ready} onClick={() => void submit()}>
            Continue sending
          </button>
        )}
        <button disabled={busy} onClick={onEdit}>
          Edit review
        </button>
      </div>
    </div>
  );
}

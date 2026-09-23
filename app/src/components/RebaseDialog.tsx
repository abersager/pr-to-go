// Moves a draft still being written onto the PR's current version: each
// comment from an earlier version gets a proposal to accept or change.

import { useEffect, useState } from "react";
import { api } from "../api";
import type { CommentAction, Draft } from "../types";
import { Interdiff, ProposalRow, defaultAction, proposalOf, suggestedAction } from "./Proposals";

export function RebaseDialog({
  prId,
  currentRevision,
  onDraft,
  onClose,
}: {
  prId: number;
  currentRevision: number;
  onDraft: (d: Draft) => void;
  onClose: () => void;
}) {
  const [draft, setDraft] = useState<Draft | null>(null);
  const [actions, setActions] = useState<Record<number, CommentAction>>({});
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    (async () => {
      try {
        await api.draftProposals(prId);
        const d = await api.draft(prId);
        setDraft(d);
        const defaults: Record<number, CommentAction> = {};
        for (const c of d?.comments ?? []) {
          const a = defaultAction(proposalOf(c));
          if (a && c.anchorRevisionId !== currentRevision) defaults[c.id] = a;
        }
        setActions(defaults);
      } catch (e) {
        setError((e as Error).message);
      }
    })();
  }, [prId, currentRevision]);

  const stale = (draft?.comments ?? []).filter(
    (c) =>
      c.kind === "thread" &&
      c.anchorRevisionId !== currentRevision &&
      c.resolution !== "drop" &&
      c.resolution !== "to_summary",
  );
  const from = stale[0]?.anchorRevisionId ?? null;
  const ready = stale.length > 0 && stale.every((c) => actions[c.id] !== undefined);

  const apply = async () => {
    setBusy(true);
    setError(null);
    try {
      const d = await api.rebaseDraft(
        prId,
        stale.map((c) => ({ id: c.id, action: actions[c.id] })),
      );
      onDraft(d);
      onClose();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal" onClick={(e) => e.stopPropagation()} role="dialog" aria-label="Move comments to this version">
        <div className="rebase-banner">
          <h2>Move your comments to this version</h2>
          <span className="spacer" />
          <button className="link" onClick={onClose}>
            Close
          </button>
        </div>
        {!draft && !error && <p className="muted">Working out where each comment goes…</p>}
        {from !== null && <Interdiff from={from} to={currentRevision} />}
        {stale.length > 0 && (
          <div className="rebase-banner small">
            <span className="muted">
              {stale.length} {stale.length === 1 ? "comment was" : "comments were"} written on an earlier version.
            </span>
            <span className="spacer" />
            <button
              className="link"
              onClick={() =>
                setActions((x) => {
                  const next = { ...x };
                  for (const c of stale) next[c.id] ??= suggestedAction(proposalOf(c));
                  return next;
                })
              }
            >
              Accept all suggestions
            </button>
          </div>
        )}
        {stale.map((c) => (
          <ProposalRow key={c.id} comment={c} action={actions[c.id]} onAction={(a) => setActions((x) => ({ ...x, [c.id]: a }))} />
        ))}
        {draft && stale.length === 0 && <p className="muted">All your comments are on this version already.</p>}
        {error && <p className="error small">{error}</p>}
        <div className="review-actions">
          <button className="primary" disabled={!ready || busy} onClick={() => void apply()}>
            Move comments
          </button>
          <button onClick={onClose}>Not now</button>
        </div>
      </div>
    </div>
  );
}

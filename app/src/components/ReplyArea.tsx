import { useRef, useState } from "react";
import { api } from "../api";
import type { Draft, ThreadEntry } from "../types";
import { Composer } from "./Composer";
import { DraftCard } from "./DraftCard";

/** Draft replies to a thread, and a composer for a new one. */
export function ReplyArea({
  prId,
  revisionId,
  thread,
  draft,
  editable,
  onDraft,
  onNotEditable,
}: {
  prId: number;
  revisionId: number;
  thread: ThreadEntry;
  draft: Draft | null;
  editable: boolean;
  onDraft: (d: Draft | null) => void;
  onNotEditable: () => void;
}) {
  const [editing, setEditingState] = useState<{ id: number | null } | null>(null);
  const replies = (draft?.comments ?? []).filter((c) => c.kind === "reply" && c.replyToThread === thread.nodeId);
  // Read by queued saves, so it must not lag behind a render.
  const idRef = useRef<number | null>(null);
  const setEditing = (e: { id: number | null } | null) => {
    idRef.current = e?.id ?? null;
    setEditingState(e);
  };

  const save = async (body: string) => {
    if (idRef.current === null) {
      const before = new Set((draft?.comments ?? []).map((c) => c.id));
      const d = await api.addDraftComment(prId, {
        revisionId,
        kind: "reply",
        subjectType: "LINE",
        replyToThread: thread.nodeId,
        body,
      });
      onDraft(d);
      const created = d.comments.find((c) => !before.has(c.id));
      idRef.current = created?.id ?? null;
      setEditing({ id: idRef.current });
    } else {
      onDraft(await api.updateDraftComment(idRef.current, body));
    }
  };

  return (
    <div className="reply-area">
      {replies
        .filter((r) => r.id !== editing?.id)
        .map((r) => (
          <DraftCard
            key={r.id}
            comment={r}
            onEdit={editable ? () => setEditing({ id: r.id }) : undefined}
            onDelete={editable ? async () => onDraft(await api.deleteDraftComment(r.id)) : undefined}
          />
        ))}
      {editing ? (
        <Composer
          label={`Reply to ${thread.comments[0]?.author ?? "thread"}`}
          initial={replies.find((r) => r.id === editing.id)?.bodyMd ?? ""}
          save={save}
          remove={async () => {
            if (idRef.current !== null) onDraft(await api.deleteDraftComment(idRef.current));
          }}
          close={() => setEditing(null)}
        />
      ) : (
        thread.viewerCanReply && (
          <button className="reply-button" onClick={() => (editable ? setEditing({ id: null }) : onNotEditable())}>
            Reply…
          </button>
        )
      )}
    </div>
  );
}

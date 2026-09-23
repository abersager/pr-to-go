import { useEffect, useState } from "react";
import { api } from "../api";
import type { Draft } from "../types";
import { ago, plural, short } from "../util/format";
import type { Route } from "../util/route";
import { useAsync } from "../util/useAsync";
import { Conversation } from "./Conversation";
import { FilesView } from "./FilesView";
import { openExternal } from "./Html";
import { SyncBadge } from "./Inbox";
import { ReviewPanel } from "./ReviewPanel";

export function PrView({
  prId,
  tick,
  route,
  onRoute,
}: {
  prId: number;
  tick: number;
  route: Route;
  onRoute: (r: Partial<Route>) => void;
}) {
  const { data: pr, error, reload } = useAsync(() => api.getPr(prId), [prId, tick]);
  const tab = route.tab;
  const setTab = (t: Route["tab"]) => onRoute({ tab: t, file: null });
  const [syncing, setSyncing] = useState(false);
  const [syncError, setSyncError] = useState<string | null>(null);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [panel, setPanel] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const revisionId = pr?.revision?.id;

  useEffect(() => {
    if (revisionId) void api.markSeen(prId);
  }, [prId, revisionId]);
  useEffect(() => {
    if (pr) setDraft(pr.draft);
  }, [pr]);

  if (error) return <p className="error pad">{error.message}</p>;
  if (!pr) return <p className="muted pad">Loading…</p>;

  const editable = !draft || draft.status === "draft";
  const notEditable = () => {
    setNotice("This review is queued. Choose Edit review to change it.");
    setPanel(true);
  };
  const onDraft = (d: Draft | null) => {
    setDraft(d);
    setNotice(null);
  };

  const sync = async () => {
    setSyncing(true);
    setSyncError(null);
    try {
      await api.syncPr(prId);
      reload();
    } catch (e) {
      setSyncError((e as Error).message);
    } finally {
      setSyncing(false);
    }
  };

  const draftCount = draft?.comments.length ?? 0;
  const moved = draft && pr.revision && draft.basisRevisionId !== pr.revision.id;

  return (
    <div className={`pr-view ${panel ? "with-panel" : ""}`}>
      <div className="pr-main">
        <header className="pr-header">
          <h1>
            {pr.title} <span className="muted">#{pr.number}</span>
          </h1>
          <div className="pr-meta small">
            <span className={`state ${pr.isDraft ? "draft" : pr.state.toLowerCase()}`}>
              {pr.isDraft ? "Draft" : pr.state.toLowerCase()}
            </span>
            <span>
              <strong>{pr.author ?? "ghost"}</strong> wants to merge into <code>{pr.baseRef}</code> from{" "}
              <code>{pr.headRef}</code>
            </span>
            <span className="muted">{pr.repo}</span>
            {pr.revision && (
              <span className="muted" title={pr.revision.headOid}>
                at {short(pr.revision.headOid)}
              </span>
            )}
            <a href={pr.url} onClick={(e) => (e.preventDefault(), openExternal(pr.url))}>
              Open on GitHub
            </a>
          </div>
          <div className="pr-sync small">
            <SyncBadge pr={pr} />
            <span className="muted">Synced {ago(pr.lastSyncedAt)}</span>
            <button onClick={sync} disabled={syncing}>
              {syncing ? "Syncing…" : "Sync now"}
            </button>
            {syncError && <span className="error">{syncError}</span>}
            <span className="spacer" />
            <button className={`review-button ${draft ? draft.status : ""}`} onClick={() => setPanel(!panel)}>
              Review{draftCount > 0 && <span className="count">{draftCount}</span>}
              {draft && draft.status !== "draft" && <span className="small"> · {draft.status.replace("_", " ")}</span>}
            </button>
          </div>
          {moved && (
            <p className="notice warn small">
              The pull request changed since you started this review. Your comments stay attached to the version you
              reviewed; you'll choose where they go before anything is sent.
            </p>
          )}
          {notice && <p className="notice small">{notice}</p>}
          <nav className="tabs">
            <button className={tab === "conversation" ? "on" : ""} onClick={() => setTab("conversation")}>
              Conversation
            </button>
            <button className={tab === "files" ? "on" : ""} onClick={() => setTab("files")}>
              Files <span className="muted">{pr.files.length}</span>{" "}
              <span className="adds">+{pr.additions}</span> <span className="dels">−{pr.deletions}</span>
            </button>
          </nav>
        </header>
        {tab === "conversation" ? (
          <Conversation
            pr={pr}
            draft={draft}
            editable={editable}
            onDraft={onDraft}
            onNotEditable={notEditable}
            onOpenReview={() => setPanel(true)}
          />
        ) : pr.revision ? (
          <FilesView
            pr={pr}
            onViewedChanged={reload}
            file={route.file}
            onFile={(file) => onRoute({ file })}
            draft={draft}
            editable={editable}
            onDraft={onDraft}
            onNotEditable={notEditable}
          />
        ) : (
          <p className="muted pad">Not synced yet. {plural(pr.changedFiles, "file")} changed.</p>
        )}
      </div>
      {panel && (
        <ReviewPanel
          pr={pr}
          draft={draft}
          onDraft={onDraft}
          onClose={() => setPanel(false)}
          onOpenFile={(file) => onRoute({ tab: "files", file })}
        />
      )}
    </div>
  );
}

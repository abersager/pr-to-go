import { useEffect, useState } from "react";
import { api } from "../api";
import { ago, plural, short } from "../util/format";
import type { Route } from "../util/route";
import { useAsync } from "../util/useAsync";
import { Conversation } from "./Conversation";
import { FilesView } from "./FilesView";
import { openExternal } from "./Html";
import { SyncBadge } from "./Inbox";

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
  const revisionId = pr?.revision?.id;

  useEffect(() => {
    if (revisionId) void api.markSeen(prId);
  }, [prId, revisionId]);

  if (error) return <p className="error pad">{error.message}</p>;
  if (!pr) return <p className="muted pad">Loading…</p>;

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

  return (
    <div className="pr-view">
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
        </div>
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
        <Conversation pr={pr} />
      ) : pr.revision ? (
        <FilesView pr={pr} onViewedChanged={reload} file={route.file} onFile={(file) => onRoute({ file })} />
      ) : (
        <p className="muted pad">Not synced yet. {plural(pr.changedFiles, "file")} changed.</p>
      )}
    </div>
  );
}

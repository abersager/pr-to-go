import { useState } from "react";
import type { OutboxItem, PrSummary, Readiness } from "../types";
import { ago, bytes, until } from "../util/format";
import { withShortcut } from "../commands";
import { Browse } from "./Browse";

export function SyncBadge({ pr }: { pr: PrSummary }) {
  const map: Record<string, [string, string]> = {
    ready: ["ok", "Available offline"],
    partial: ["warn", "Partly available offline"],
    fetching: ["busy", "Syncing…"],
    stale: ["stale", `Last synced ${ago(pr.lastSyncedAt)}`],
    error: ["err", pr.syncError ?? "Sync failed"],
    indexed: ["none", "Not synced yet"],
    dormant: ["none", "No longer in your inbox"],
  };
  const [cls, title] = map[pr.syncState] ?? ["none", pr.syncState];
  return <span className={`sync ${cls}`} title={title} aria-label={title} />;
}

const OUTBOX_STATUS: Record<string, string> = {
  queued: "queued",
  preflight: "checking…",
  staging: "sending…",
  submitting: "sending…",
  needs_attention: "needs attention",
  submitted: "sent",
};

function outboxDetail(o: OutboxItem): string {
  if (o.status === "submitted") return `sent ${ago(o.submittedAt)}`;
  if (o.lastErrorKind === "offline") return "waiting for a connection";
  if (o.lastError && o.nextAttemptAt) return `retrying ${until(o.nextAttemptAt)}`;
  return OUTBOX_STATUS[o.status] ?? o.status;
}

export function Outbox({ items, onOpen }: { items: OutboxItem[]; onOpen: (prId: number) => void }) {
  if (items.length === 0) return null;
  const active = items.filter((o) => o.status !== "submitted").length;
  return (
    <section className="outbox">
      <h3>
        Outbox {active > 0 && <span className="count">{active}</span>}
      </h3>
      <ul>
        {items.map((o) => (
          <li key={o.draftReviewId} className={`outbox-item ${o.status}`} onClick={() => onOpen(o.prId)}>
            <span className={`outbox-dot ${o.status}`} />
            <span className="title">{o.title}</span>
            <span className="small muted">
              {o.repo}#{o.number} · {outboxDetail(o)}
            </span>
          </li>
        ))}
      </ul>
    </section>
  );
}

export function Inbox({
  prs,
  selected,
  onSelect,
  onAdd,
  outbox,
  onOpenReview,
  readiness,
  progress,
  syncing,
  syncError,
  hasSubscriptions,
  onSyncAll,
  onSettings,
  onFollowReviewRequests,
  onBrowseOpen,
  mode,
  onMode,
}: {
  prs: PrSummary[];
  selected: number | null;
  onSelect: (id: number) => void;
  onAdd: (input: string) => Promise<void>;
  outbox: OutboxItem[];
  onOpenReview: (prId: number) => void;
  readiness: Readiness | null;
  progress: { done: number; total: number } | null;
  syncing: boolean;
  syncError: string | null;
  hasSubscriptions: boolean;
  onSyncAll: () => void;
  onSettings: () => void;
  onFollowReviewRequests: () => void;
  /** A PR picked under Browse (already added and synced). */
  onBrowseOpen: (id: number) => void;
  mode: "inbox" | "browse";
  onMode: (mode: "inbox" | "browse") => void;
}) {
  const [input, setInput] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  return (
    <aside className="inbox">
      <div className="sidebar-tabs segmented" role="tablist">
        <button
          role="tab"
          aria-selected={mode === "inbox"}
          className={mode === "inbox" ? "on" : ""}
          onClick={() => onMode("inbox")}
          title={withShortcut("Inbox", "view.inbox")}
        >
          Inbox
        </button>
        <button
          role="tab"
          aria-selected={mode === "browse"}
          className={mode === "browse" ? "on" : ""}
          onClick={() => onMode("browse")}
          title={withShortcut("Browse", "view.browse")}
        >
          Browse
        </button>
      </div>
      {mode === "browse" ? (
        <Browse selected={selected} onOpen={onBrowseOpen} />
      ) : (
        <>
          <form
            className="add-pr"
            onSubmit={async (e) => {
              e.preventDefault();
              setBusy(true);
              setError(null);
              try {
                await onAdd(input);
                setInput("");
              } catch (err) {
                setError((err as Error).message);
              } finally {
                setBusy(false);
              }
            }}
          >
            <input
              id="add-pr-input"
              placeholder="Add a PR: URL or owner/repo#123"
              value={input}
              onChange={(e) => setInput(e.target.value)}
              disabled={busy}
            />
            <button type="submit" disabled={busy || !input.trim()}>
              {busy ? "Syncing…" : "Add"}
            </button>
            {error && <p className="error small">{error}</p>}
          </form>
          <div className="inbox-head">
            <button
              onClick={onSyncAll}
              disabled={syncing}
              title={withShortcut("Sync everything you follow, for offline review", "inbox.syncAll")}
            >
              {syncing ? (progress && progress.total > 0 ? `Syncing ${progress.done}/${progress.total}…` : "Checking…") : "Sync all"}
            </button>
            {readiness && readiness.total > 0 && (
              <span className="readiness" title={`${readiness.partial} partly available, ${readiness.notSynced} not synced yet`}>
                {readiness.ready + readiness.partial}/{readiness.total} offline · {bytes(readiness.bytes)}
              </span>
            )}
            <span className="spacer" />
            <button className="icon-button" onClick={onSettings} title={withShortcut("Settings", "app.settings")}>
              ⚙
            </button>
          </div>
          {syncError && <p className="error small pad">{syncError}</p>}
          <Outbox items={outbox} onOpen={onOpenReview} />
          {prs.length === 0 && !hasSubscriptions && (
            <div className="empty-inbox muted small">
              <p>Follow the pull requests you review, and they'll be ready whenever you're offline.</p>
              <button onClick={onFollowReviewRequests}>+ Review requested from me</button>{" "}
              <button onClick={onSettings}>More options…</button>
            </div>
          )}
          {prs.length === 0 && hasSubscriptions && <p className="muted empty-list">Nothing in your inbox right now.</p>}
          <ul>
            {prs.map((pr) => (
              <li key={pr.id} className={pr.id === selected ? "selected" : ""} onClick={() => onSelect(pr.id)}>
                <div className="row1">
                  <SyncBadge pr={pr} />
                  <span className="title">{pr.title}</span>
                  {pr.updatedSinceViewed && <span className="dot-new" title="Updated since you last looked" />}
                </div>
                <div className="row2 muted small">
                  {pr.repo}#{pr.number} · {pr.author ?? "ghost"} · {ago(pr.updatedAt)}
                  {pr.state !== "OPEN" && <span className={`state ${pr.state.toLowerCase()}`}>{pr.state.toLowerCase()}</span>}
                  {pr.draftStatus && <span className="badge">{pr.draftStatus === "draft" ? "draft review" : pr.draftStatus.replace("_", " ")}</span>}
                </div>
              </li>
            ))}
          </ul>
        </>
      )}
    </aside>
  );
}

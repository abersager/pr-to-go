import { useEffect, useRef, useState } from "react";
import { ApiError, api } from "../api";
import type { BrowsePage, BrowsePr, BrowseScope } from "../types";
import { ago } from "../util/format";

function scopeText(s: BrowseScope): string {
  if (s.custom) return "Searching only where your filter says.";
  const parts = ["your repositories"];
  parts.push(...s.orgs.slice(0, 3));
  if (s.orgs.length > 3) parts.push(`${s.orgs.length - 3} more organizations`);
  if (s.sharedRepos > 0) parts.push(`${s.sharedRepos} shared ${s.sharedRepos === 1 ? "repository" : "repositories"}`);
  const list = parts.length > 1 ? `${parts.slice(0, -1).join(", ")} and ${parts[parts.length - 1]}` : parts[0];
  return `Open pull requests in ${list}.`;
}

/** Every open PR the user can reach, for picking what to take offline. */
export function Browse({ selected, onOpen }: { selected: number | null; onOpen: (id: number) => void }) {
  const [input, setInput] = useState("");
  const [filter, setFilter] = useState("");
  const [page, setPage] = useState<BrowsePage | null>(null);
  const [prs, setPrs] = useState<BrowsePr[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<ApiError | null>(null);
  const [adding, setAdding] = useState<string | null>(null);
  // Ignore answers to requests the user has since replaced.
  const generation = useRef(0);

  const load = async (f: string, cursor: string | null) => {
    const gen = ++generation.current;
    setLoading(true);
    setError(null);
    try {
      const p = await api.browsePrs(f, cursor);
      if (gen !== generation.current) return;
      setPage(p);
      setPrs((old) => (cursor ? [...old, ...p.prs.filter((n) => !old.some((o) => o.nodeId === n.nodeId))] : p.prs));
    } catch (e) {
      if (gen === generation.current) setError(e as ApiError);
    } finally {
      if (gen === generation.current) setLoading(false);
    }
  };

  useEffect(() => {
    setPrs([]);
    void load(filter, null);
  }, [filter]);

  const open = async (pr: BrowsePr) => {
    if (pr.localId !== null) return onOpen(pr.localId);
    setAdding(pr.nodeId);
    setError(null);
    try {
      const id = await api.addPr(pr.url);
      setPrs((all) => all.map((p) => (p.nodeId === pr.nodeId ? { ...p, localId: id, offline: true } : p)));
      onOpen(id);
    } catch (e) {
      setError(e as ApiError);
    } finally {
      setAdding(null);
    }
  };

  return (
    <div className="browse">
      <form
        className="add-pr"
        onSubmit={(e) => {
          e.preventDefault();
          const f = input.trim();
          if (f === filter) void load(f, null);
          else setFilter(f);
        }}
      >
        <input
          aria-label="Filter pull requests"
          placeholder="Filter: words, author:bob, repo:acme/api"
          value={input}
          onChange={(e) => setInput(e.target.value)}
        />
        <button type="submit" disabled={loading}>
          {loading && prs.length === 0 ? "Loading…" : "Search"}
        </button>
      </form>
      {page && (
        <p className="small muted browse-scope">
          {scopeText(page.scope)} {page.total > 0 && <>{page.total} open.</>}
        </p>
      )}
      {error && (
        <p className={`small pad ${error.kind === "offline" ? "muted" : "error"}`}>
          {error.kind === "offline" ? "Browsing needs a connection. Your inbox works offline." : error.message}
        </p>
      )}
      {page && !loading && prs.length === 0 && !error && (
        <p className="muted empty-list">No open pull requests found.</p>
      )}
      <ul>
        {prs.map((pr) => (
          <li
            key={pr.nodeId}
            className={pr.localId !== null && pr.localId === selected ? "selected" : ""}
            onClick={() => void open(pr)}
            title={pr.localId !== null ? "Open" : "Take this pull request offline and open it"}
          >
            <div className="row1">
              <span
                className={`sync ${pr.offline ? "ok" : "none"}`}
                title={pr.offline ? "Available offline" : "Not synced"}
                aria-label={pr.offline ? "Available offline" : "Not synced"}
              />
              <span className="title">{pr.title}</span>
            </div>
            <div className="row2 muted small">
              {pr.repo}#{pr.number} · {pr.author ?? "ghost"} · {ago(pr.updatedAt)}
              {pr.isDraft && <span className="badge">draft</span>}
              {adding === pr.nodeId && <span className="badge">adding…</span>}
            </div>
          </li>
        ))}
      </ul>
      {page?.cursor && (
        <div className="pad">
          <button onClick={() => void load(filter, page.cursor)} disabled={loading}>
            {loading ? "Loading…" : "Load more"}
          </button>
        </div>
      )}
    </div>
  );
}

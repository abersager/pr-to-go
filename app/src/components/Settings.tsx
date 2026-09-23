import { useState } from "react";
import { api, inTauri } from "../api";
import { ago, bytes } from "../util/format";
import { useAsync } from "../util/useAsync";

export function Settings({ onClose, onChanged }: { onClose: () => void; onChanged: () => void }) {
  const subs = useAsync(() => api.subscriptions(), []);
  const presets = useAsync(() => api.subscriptionPresets(), []);
  const readiness = useAsync(() => api.readiness(), []);
  const [repo, setRepo] = useState("");
  const [query, setQuery] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [cleaned, setCleaned] = useState<string | null>(null);

  const add = async (kind: "repo" | "search", value: string, label?: string) => {
    setError(null);
    try {
      await api.addSubscription(kind, value, label);
      setRepo("");
      setQuery("");
      subs.reload();
      onChanged();
    } catch (e) {
      setError((e as Error).message);
    }
  };

  const followed = new Set((subs.data ?? []).map((s) => s.query));
  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal" role="dialog" aria-label="Settings" onClick={(e) => e.stopPropagation()}>
        <div className="rebase-banner">
          <h2>What to keep for offline review</h2>
          <span className="spacer" />
          <button className="link" onClick={onClose}>
            Close
          </button>
        </div>
        <p className="small muted">
          Pull requests from these are synced every 15 minutes while you're online, with everything needed to review
          them offline.
        </p>
        <ul className="sub-list">
          {(subs.data ?? []).map((s) => (
            <li key={s.id}>
              <div>
                <strong>{s.label}</strong>{" "}
                <span className="muted small">
                  {s.kind === "repo" ? "repository" : s.query} · {s.prs} open ·{" "}
                  {s.lastPolledAt ? `checked ${ago(s.lastPolledAt)}` : "not checked yet"}
                </span>
                {s.lastError && <div className="small warn-text">{s.lastError}</div>}
              </div>
              <button
                className="link"
                onClick={async () => {
                  await api.removeSubscription(s.id);
                  subs.reload();
                  onChanged();
                }}
              >
                Remove
              </button>
            </li>
          ))}
          {subs.data?.length === 0 && <li className="muted small">Nothing yet.</li>}
        </ul>
        <div className="small">
          {(presets.data ?? [])
            .filter((p) => !followed.has(p.query) && !followed.has(`${p.query} is:pr`))
            .map((p) => (
              <button key={p.query} onClick={() => void add("search", p.query, p.label)}>
                + {p.label}
              </button>
            ))}
        </div>
        <form
          className="add-sub"
          onSubmit={(e) => {
            e.preventDefault();
            void add("repo", repo);
          }}
        >
          <input placeholder="Repository: owner/name" value={repo} onChange={(e) => setRepo(e.target.value)} />
          <button type="submit" disabled={!repo.trim()}>
            Follow repository
          </button>
        </form>
        <form
          className="add-sub"
          onSubmit={(e) => {
            e.preventDefault();
            void add("search", query);
          }}
        >
          <input
            placeholder="Search, e.g. is:open org:acme label:needs-review"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
          <button type="submit" disabled={!query.trim()}>
            Add search
          </button>
        </form>
        {error && <p className="error small">{error}</p>}
        <h3 className="small">Storage</h3>
        <p className="small">
          {readiness.data ? `${bytes(readiness.data.bytes)} used.` : "…"}{" "}
          <button
            className="link"
            onClick={async () => {
              const r = await api.gc();
              setCleaned(`Removed ${r.prs} old pull requests and ${r.revisions} old versions.`);
              readiness.reload();
            }}
          >
            Clean up now
          </button>{" "}
          <span className="muted">{cleaned}</span>
        </p>
        <p className="small muted">
          Pull requests that leave your inbox are removed after two weeks, unless you have a review in progress for
          them or added them by hand.
        </p>
        {inTauri && (
          <p className="small">
            <button className="link" onClick={() => void api.revealLogs()}>
              Show log files
            </button>{" "}
            <span className="muted">Useful when reporting a problem. They never contain your token.</span>
          </p>
        )}
      </div>
    </div>
  );
}

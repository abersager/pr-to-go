import type { AuthStatus, Connectivity } from "../types";

export function TopBar({
  auth,
  conn,
  onToggleOffline,
  onCheck,
  onSignOut,
  onToggleInbox,
}: {
  auth: AuthStatus;
  conn: Connectivity;
  onToggleOffline: (offline: boolean) => void;
  onCheck: () => void;
  onSignOut: () => void;
  onToggleInbox: () => void;
}) {
  const state = conn.workOffline ? "working-offline" : conn.online ? "online" : "offline";
  const label = conn.workOffline ? "Working offline" : conn.online ? "Online" : "Offline";
  return (
    <header className="topbar">
      <button className="icon-button" onClick={onToggleInbox} title="Show or hide the pull request list">
        ☰
      </button>
      <span className="brand">PR to Go</span>
      <button className={`pill ${state}`} onClick={onCheck} title={conn.detail ?? "Check connection"}>
        <span className="dot" /> {label}
      </button>
      <label className="toggle" title="Don't use the network, even if it's there">
        <input type="checkbox" checked={conn.workOffline} onChange={(e) => onToggleOffline(e.target.checked)} />
        Work offline
      </label>
      <span className="spacer" />
      <span className="muted">{auth.login}</span>
      <button className="link" onClick={onSignOut}>
        Sign out
      </button>
    </header>
  );
}

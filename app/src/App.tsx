import { getCurrentWebview } from "@tauri-apps/api/webview";
import { useCallback, useEffect, useState } from "react";
import { api, inTauri, onCoreEvent } from "./api";
import { handleShortcut, useCommand } from "./commands";
import { openExternal } from "./components/Html";
import { Inbox } from "./components/Inbox";
import { PrView } from "./components/PrView";
import { Settings } from "./components/Settings";
import { Shortcuts } from "./components/Shortcuts";
import { SignIn } from "./components/SignIn";
import { TopBar } from "./components/TopBar";
import type { AuthStatus, Connectivity } from "./types";
import { useRoute } from "./util/route";
import { useAsync } from "./util/useAsync";

const OFFLINE: Connectivity = { online: false, workOffline: false, detail: null, rateRemaining: null };

const ZOOM_STEPS = [0.5, 0.67, 0.75, 0.8, 0.9, 1, 1.1, 1.25, 1.5, 1.75, 2];

function loadZoom(): number {
  try {
    const z = Number(localStorage.getItem("zoom"));
    return ZOOM_STEPS.includes(z) ? z : 1;
  } catch {
    return 1;
  }
}

export function App() {
  const [auth, setAuth] = useState<AuthStatus | null>(null);
  const [conn, setConn] = useState<Connectivity>(OFFLINE);
  const [tick, setTick] = useState(0);
  const [inboxHidden, setInboxHidden] = useState(() => {
    try {
      return localStorage.getItem("inboxHidden") === "1";
    } catch {
      return false;
    }
  });
  const showInbox = (show: boolean) => {
    setInboxHidden(!show);
    try {
      localStorage.setItem("inboxHidden", show ? "0" : "1");
    } catch {
      /* per-viewer convenience only */
    }
  };
  const toggleInbox = () => showInbox(inboxHidden);
  const [sidebar, setSidebar] = useState<"inbox" | "browse">("inbox");
  const [shortcuts, setShortcuts] = useState(false);
  const [zoom, setZoom] = useState(loadZoom);
  const [route, setRoute] = useRoute();
  const selected = route.prId;
  const setSelected = (prId: number) => setRoute({ prId, tab: "conversation", file: null });
  const signedIn = auth?.signedIn ?? false;
  const prs = useAsync(() => (signedIn ? api.listPrs() : Promise.resolve([])), [signedIn, tick]);
  const outbox = useAsync(() => (signedIn ? api.outbox() : Promise.resolve([])), [signedIn, tick]);
  const [reviewFor, setReviewFor] = useState<number | null>(null);
  const readiness = useAsync(() => (signedIn ? api.readiness() : Promise.resolve(null)), [signedIn, tick]);
  const subscriptions = useAsync(() => (signedIn ? api.subscriptions() : Promise.resolve([])), [signedIn, tick]);
  const [progress, setProgress] = useState<{ done: number; total: number } | null>(null);
  const [syncing, setSyncing] = useState(false);
  const [syncError, setSyncError] = useState<string | null>(null);
  const [settings, setSettings] = useState(false);
  const syncAll = useCallback(async () => {
    setSyncing(true);
    setSyncError(null);
    try {
      const r = await api.syncInbox();
      if (r.failed > 0) setSyncError(`${r.failed} could not be synced: ${r.errors[0] ?? ""}`);
    } catch (e) {
      setSyncError((e as Error).message);
    } finally {
      setSyncing(false);
      setProgress(null);
      setTick((t) => t + 1);
    }
  }, []);

  useEffect(() => {
    api.authStatus().then(setAuth, () => setAuth({ signedIn: false, login: null, source: null, scopes: null }));
    api.connectivity().then(setConn, () => {});
  }, []);

  useEffect(() => {
    const unlisten = onCoreEvent((e) => {
      if (e.type === "connectivity") {
        setConn((c) => ({ ...c, online: e.online, workOffline: e.workOffline, detail: e.detail }));
      }
      if (e.type === "syncProgress") {
        setProgress({ done: e.done, total: e.total });
        return;
      }
      setTick((t) => t + 1);
    });
    return () => {
      void unlisten.then((f) => f()).catch(() => {});
    };
  }, []);

  const checkConnection = useCallback(() => {
    api.checkConnectivity().then(setConn, () => {});
  }, []);

  const setWorkOffline = async (offline: boolean) => {
    await api.setWorkOffline(offline);
    setConn((c) => ({ ...c, workOffline: offline, online: offline ? false : c.online }));
    if (!offline) checkConnection();
  };

  // Keyboard shortcuts: the desktop app gets them from its menu bar; the
  // browser build has none, so they're handled here.
  useEffect(() => {
    if (inTauri) {
      void import("./menu").then((m) => m.installMenu());
      return;
    }
    window.addEventListener("keydown", handleShortcut);
    return () => window.removeEventListener("keydown", handleShortcut);
  }, []);

  useEffect(() => {
    try {
      localStorage.setItem("zoom", String(zoom));
    } catch {
      /* per-viewer convenience only */
    }
    if (inTauri) void getCurrentWebview().setZoom(zoom);
    else document.documentElement.style.setProperty("zoom", String(zoom));
  }, [zoom]);
  const stepZoom = (d: number) =>
    setZoom((z) => ZOOM_STEPS[Math.min(ZOOM_STEPS.length - 1, Math.max(0, ZOOM_STEPS.indexOf(z) + d))]);

  // PRs in the order the sidebar lists them.
  const list = prs.data ?? [];
  const at = list.findIndex((p) => p.id === selected);
  const goPr = (d: number) => {
    const next = list[at === -1 ? (d > 0 ? 0 : list.length - 1) : at + d];
    if (next) setSelected(next.id);
  };
  const showSidebar = (mode: "inbox" | "browse") => {
    setSidebar(mode);
    showInbox(true);
  };

  useCommand("app.settings", () => setSettings(true), { enabled: signedIn });
  useCommand(
    "pr.add",
    () => {
      showSidebar("inbox");
      requestAnimationFrame(() => document.getElementById("add-pr-input")?.focus());
    },
    { enabled: signedIn },
  );
  useCommand("inbox.syncAll", () => void syncAll(), { enabled: signedIn && !syncing });
  useCommand("net.workOffline", () => void setWorkOffline(!conn.workOffline), {
    enabled: signedIn,
    checked: conn.workOffline,
  });
  useCommand("view.inbox", () => showSidebar("inbox"), { enabled: signedIn, checked: sidebar === "inbox" });
  useCommand("view.browse", () => showSidebar("browse"), { enabled: signedIn, checked: sidebar === "browse" });
  useCommand("view.sidebar", toggleInbox, { enabled: signedIn, checked: !inboxHidden });
  useCommand("view.zoomIn", () => stepZoom(1), { enabled: zoom < ZOOM_STEPS[ZOOM_STEPS.length - 1] });
  useCommand("view.zoomOut", () => stepZoom(-1), { enabled: zoom > ZOOM_STEPS[0] });
  useCommand("view.actualSize", () => setZoom(1), { enabled: zoom !== 1 });
  useCommand("go.nextPr", () => goPr(1), { enabled: signedIn && list.length > 0 && at < list.length - 1 });
  useCommand("go.prevPr", () => goPr(-1), { enabled: signedIn && list.length > 0 && at !== 0 });
  useCommand("help.shortcuts", () => setShortcuts(true));
  useCommand("help.project", () => openExternal("https://github.com/abersager/pr-to-go"));

  if (!auth) return <main className="empty muted">Loading…</main>;
  if (!auth.signedIn) return <SignIn onSignedIn={setAuth} />;

  return (
    <div className={`app ${inboxHidden ? "inbox-hidden" : ""}`}>
      <TopBar
        auth={auth}
        conn={conn}
        onCheck={checkConnection}
        onToggleOffline={(offline) => void setWorkOffline(offline)}
        onToggleInbox={toggleInbox}
        onSignOut={async () => {
          await api.signOut();
          setAuth({ signedIn: false, login: null, source: null, scopes: null });
        }}
      />
      <Inbox
        prs={prs.data ?? []}
        selected={selected}
        onSelect={setSelected}
        outbox={outbox.data ?? []}
        readiness={readiness.data ?? null}
        progress={progress}
        syncing={syncing}
        syncError={syncError}
        hasSubscriptions={(subscriptions.data ?? []).length > 0}
        onSyncAll={() => void syncAll()}
        onSettings={() => setSettings(true)}
        onFollowReviewRequests={async () => {
          await api.addSubscription("search", "is:open is:pr review-requested:@me", "Review requested from me");
          void syncAll();
        }}
        onOpenReview={(prId) => {
          setReviewFor(prId);
          setRoute({ prId, tab: "conversation", file: null });
        }}
        onAdd={async (input) => {
          const id = await api.addPr(input);
          setSelected(id);
          prs.reload();
        }}
        onBrowseOpen={(id) => {
          setSelected(id);
          prs.reload();
          readiness.reload();
        }}
        mode={sidebar}
        onMode={setSidebar}
      />
      {shortcuts && <Shortcuts onClose={() => setShortcuts(false)} />}
      {settings && (
        <Settings
          onClose={() => setSettings(false)}
          onChanged={() => setTick((t) => t + 1)}
        />
      )}
      <main className="main">
        {selected !== null ? (
          <PrView
            key={selected}
            prId={selected}
            tick={tick}
            route={route}
            onRoute={setRoute}
            openReview={reviewFor === selected}
            onReviewOpened={() => setReviewFor(null)}
          />
        ) : (
          <div className="empty muted">
            <p>Select a pull request, or add one by URL.</p>
            <p className="small">Everything you open here is kept for offline review.</p>
          </div>
        )}
      </main>
    </div>
  );
}

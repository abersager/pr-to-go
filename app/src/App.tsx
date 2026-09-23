import { useCallback, useEffect, useState } from "react";
import { api, onCoreEvent } from "./api";
import { Inbox } from "./components/Inbox";
import { PrView } from "./components/PrView";
import { SignIn } from "./components/SignIn";
import { TopBar } from "./components/TopBar";
import type { AuthStatus, Connectivity } from "./types";
import { useRoute } from "./util/route";
import { useAsync } from "./util/useAsync";

const OFFLINE: Connectivity = { online: false, workOffline: false, detail: null, rateRemaining: null };

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
  const toggleInbox = () =>
    setInboxHidden((h) => {
      try {
        localStorage.setItem("inboxHidden", h ? "0" : "1");
      } catch {
        /* per-viewer convenience only */
      }
      return !h;
    });
  const [route, setRoute] = useRoute();
  const selected = route.prId;
  const setSelected = (prId: number) => setRoute({ prId, tab: "conversation", file: null });
  const signedIn = auth?.signedIn ?? false;
  const prs = useAsync(() => (signedIn ? api.listPrs() : Promise.resolve([])), [signedIn, tick]);
  const outbox = useAsync(() => (signedIn ? api.outbox() : Promise.resolve([])), [signedIn, tick]);
  const [reviewFor, setReviewFor] = useState<number | null>(null);

  useEffect(() => {
    api.authStatus().then(setAuth, () => setAuth({ signedIn: false, login: null, source: null, scopes: null }));
    api.connectivity().then(setConn, () => {});
  }, []);

  useEffect(() => {
    const unlisten = onCoreEvent((e) => {
      if (e.type === "connectivity") {
        setConn((c) => ({ ...c, online: e.online, workOffline: e.workOffline, detail: e.detail }));
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

  if (!auth) return <main className="empty muted">Loading…</main>;
  if (!auth.signedIn) return <SignIn onSignedIn={setAuth} />;

  return (
    <div className={`app ${inboxHidden ? "inbox-hidden" : ""}`}>
      <TopBar
        auth={auth}
        conn={conn}
        onCheck={checkConnection}
        onToggleOffline={async (offline) => {
          await api.setWorkOffline(offline);
          setConn((c) => ({ ...c, workOffline: offline, online: offline ? false : c.online }));
          if (!offline) checkConnection();
        }}
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
        onOpenReview={(prId) => {
          setReviewFor(prId);
          setRoute({ prId, tab: "conversation", file: null });
        }}
        onAdd={async (input) => {
          const id = await api.addPr(input);
          setSelected(id);
          prs.reload();
        }}
      />
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

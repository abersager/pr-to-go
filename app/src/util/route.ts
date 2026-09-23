import { useCallback, useEffect, useState } from "react";

/** Where the user is: `#/pr/<id>[/files[/<path>]]`. */
export type Route = { prId: number | null; tab: "conversation" | "files"; file: string | null };

export function parseRoute(hash: string): Route {
  const parts = hash.replace(/^#\/?/, "").split("/");
  if (parts[0] !== "pr" || !/^\d+$/.test(parts[1] ?? "")) return { prId: null, tab: "conversation", file: null };
  const prId = Number(parts[1]);
  if (parts[2] !== "files") return { prId, tab: "conversation", file: null };
  const file = parts.length > 3 ? decodeURIComponent(parts.slice(3).join("/")) : null;
  return { prId, tab: "files", file };
}

export function formatRoute(r: Route): string {
  if (r.prId === null) return "#/";
  if (r.tab === "conversation") return `#/pr/${r.prId}`;
  return `#/pr/${r.prId}/files${r.file ? `/${encodeURIComponent(r.file)}` : ""}`;
}

export function useRoute(): [Route, (r: Partial<Route>) => void] {
  const [route, setRoute] = useState(() => parseRoute(window.location.hash));
  useEffect(() => {
    const on = () => setRoute(parseRoute(window.location.hash));
    window.addEventListener("hashchange", on);
    return () => window.removeEventListener("hashchange", on);
  }, []);
  const update = useCallback((r: Partial<Route>) => {
    setRoute((cur) => {
      const next = { ...cur, ...r };
      const hash = formatRoute(next);
      if (hash !== window.location.hash) window.history.replaceState(null, "", hash);
      return next;
    });
  }, []);
  return [route, update];
}

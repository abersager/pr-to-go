import { useEffect, useState } from "react";
import type { Request, Response, Token } from "./highlight.worker";

export type { Token };
export type LineTokens = Token[];

/** Files bigger than this aren't highlighted. */
export const HIGHLIGHT_MAX_CHARS = 500_000;

let worker: Worker | null = null;
let nextId = 1;
const waiting = new Map<number, (lines: LineTokens[] | null) => void>();
const cache = new Map<string, Promise<LineTokens[] | null>>();

function getWorker(): Worker | null {
  if (typeof Worker === "undefined") return null;
  if (!worker) {
    worker = new Worker(new URL("./highlight.worker.ts", import.meta.url), { type: "module" });
    worker.onmessage = (e: MessageEvent<Response>) => {
      waiting.get(e.data.id)?.(e.data.lines);
      waiting.delete(e.data.id);
    };
  }
  return worker;
}

/** Tokens per line (index = line number − 1), or null if not highlighted. */
export function highlight(key: string, path: string, code: string): Promise<LineTokens[] | null> {
  const cacheKey = `${key}:${path}`;
  const hit = cache.get(cacheKey);
  if (hit) return hit;
  const w = getWorker();
  const p =
    !w || code.length > HIGHLIGHT_MAX_CHARS
      ? Promise.resolve(null)
      : new Promise<LineTokens[] | null>((resolve) => {
          const id = nextId++;
          waiting.set(id, resolve);
          w.postMessage({ id, path, code } satisfies Request);
        });
  cache.set(cacheKey, p);
  return p;
}

export function useHighlight(key: string | null, path: string, code: string | null): LineTokens[] | null {
  const [lines, setLines] = useState<LineTokens[] | null>(null);
  useEffect(() => {
    setLines(null);
    if (!key || code === null) return;
    let live = true;
    highlight(key, path, code).then((l) => live && setLines(l));
    return () => {
      live = false;
    };
  }, [key, path, code]);
  return lines;
}

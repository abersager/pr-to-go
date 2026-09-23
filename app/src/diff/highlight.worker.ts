// Syntax highlighting off the main thread. Whole files are tokenized, so
// multi-line strings and comments are coloured correctly inside hunks.

import {
  bundledLanguagesInfo,
  createHighlighter,
  createJavaScriptRegexEngine,
  type BundledLanguage,
  type Highlighter,
} from "shiki";
import { langForPath } from "./lang";

export type Token = [content: string, light: string, dark: string, fontStyle: number];
export type Request = { id: number; path: string; code: string };
export type Response = { id: number; lines: Token[][] | null; error?: string };

const known = new Set(bundledLanguagesInfo.flatMap((l) => [l.id, ...(l.aliases ?? [])]));
let highlighter: Promise<Highlighter> | null = null;

function get(): Promise<Highlighter> {
  highlighter ??= createHighlighter({
    themes: ["github-light", "github-dark"],
    langs: [],
    engine: createJavaScriptRegexEngine(),
  });
  return highlighter;
}

self.onmessage = async (e: MessageEvent<Request>) => {
  const { id, path, code } = e.data;
  const reply = (r: Response) => (self as unknown as Worker).postMessage(r);
  const lang = langForPath(path, (l) => known.has(l));
  if (!lang) return reply({ id, lines: null });
  try {
    const h = await get();
    if (!h.getLoadedLanguages().includes(lang)) await h.loadLanguage(lang as BundledLanguage);
    const { tokens } = h.codeToTokens(code, {
      lang: lang as BundledLanguage,
      themes: { light: "github-light", dark: "github-dark" },
      defaultColor: false,
    });
    const lines = tokens.map((line) =>
      line.map((t): Token => [
        t.content,
        t.htmlStyle?.["--shiki-light"] ?? "",
        t.htmlStyle?.["--shiki-dark"] ?? "",
        t.fontStyle ?? 0,
      ]),
    );
    reply({ id, lines });
  } catch (err) {
    reply({ id, lines: null, error: String(err) });
  }
};

// Local Markdown rendering for the user's own drafts (GitHub renders
// everything else). Suggestion blocks are shown as a before/after preview.

import MarkdownIt from "markdown-it";
import { sanitize } from "../components/Html";

const md = new MarkdownIt({ linkify: true, breaks: false });

const esc = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

const defaultFence = md.renderer.rules.fence!;
md.renderer.rules.fence = (tokens, idx, options, env: { original?: string[] }, self) => {
  const t = tokens[idx];
  if (t.info.trim().split(/\s+/)[0] !== "suggestion") return defaultFence(tokens, idx, options, env, self);
  const removed = (env.original ?? []).map((l) => `<div class="del">${esc(l) || " "}</div>`).join("");
  const added = t.content
    .replace(/\n$/, "")
    .split("\n")
    .map((l) => `<div class="add">${esc(l) || " "}</div>`)
    .join("");
  return `<div class="suggestion"><div class="suggestion-head">Suggested change</div><pre>${removed}${added}</pre></div>`;
};

/** Renders the user's Markdown. `original` is what a suggestion replaces. */
export function renderMarkdown(src: string, original?: string[]): string {
  return sanitize(md.render(src, { original }));
}

/** A suggestion block that replaces `lines`, ready to edit. */
export function suggestionBlock(lines: string[]): string {
  return "```suggestion\n" + lines.join("\n") + (lines.length ? "\n" : "") + "```\n";
}

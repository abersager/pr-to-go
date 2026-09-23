// GitHub's rendered HTML (PR bodies, comments), sanitized. PR content is
// attacker-controlled on public repos, so: DOMPurify, only local images
// (remote ones would be blocked by the CSP anyway), and links open in the
// system browser, never in the app's webview.

import DOMPurify from "dompurify";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useMemo } from "react";
import { inTauri, localUrl } from "../api";

const ALLOWED_URI = /^(?:(?:https?|mailto|prtg):|[^a-z]|[a-z+.-]+(?:[^a-z+.\-:]|$))/i;
const LOCAL = "prtg://localhost/";

DOMPurify.addHook("afterSanitizeAttributes", (node) => {
  if (node.tagName === "A") {
    node.setAttribute("rel", "noreferrer");
  }
  if (node.tagName === "IMG") {
    const src = node.getAttribute("src") ?? "";
    if (src.startsWith(LOCAL)) {
      node.setAttribute("src", localUrl(src.slice(LOCAL.length)));
    } else {
      // Not cached for offline use; show the alt text instead of a broken image.
      const alt = node.getAttribute("alt") || "image";
      const span = node.ownerDocument.createElement("span");
      span.className = "missing-image";
      span.textContent = `[${alt} — not available offline]`;
      node.replaceWith(span);
    }
  }
});

export function sanitize(html: string): string {
  return DOMPurify.sanitize(html, {
    ALLOWED_URI_REGEXP: ALLOWED_URI,
    FORBID_TAGS: ["style", "form", "iframe", "video", "audio", "source"],
    FORBID_ATTR: ["style"],
  });
}

export function openExternal(href: string) {
  if (!/^https?:\/\//.test(href)) return;
  if (inTauri) void openUrl(href);
  else window.open(href, "_blank", "noopener");
}

export function Html({ html, className = "" }: { html: string; className?: string }) {
  const clean = useMemo(() => sanitize(html), [html]);
  if (!clean.trim()) return <p className="muted">No description.</p>;
  return (
    <div
      className={`markdown ${className}`}
      dangerouslySetInnerHTML={{ __html: clean }}
      onClick={(e) => {
        const a = (e.target as HTMLElement).closest("a");
        if (a?.href) {
          e.preventDefault();
          openExternal(a.href);
        }
      }}
    />
  );
}

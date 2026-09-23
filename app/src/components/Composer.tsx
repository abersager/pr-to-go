// Writes one draft comment (or reply). Saves as you type, so nothing is lost
// if the laptop dies mid-flight.

import { useEffect, useRef, useState } from "react";
import { renderMarkdown, suggestionBlock } from "../util/markdown";

export type ComposerProps = {
  initial: string;
  /** Saves the text; returns once stored. */
  save: (body: string) => Promise<void>;
  /** Removes the comment (for a new one, cancelling does this). */
  remove?: () => Promise<void>;
  close: () => void;
  /** Lines a suggestion would replace (new-side lines only). */
  suggestionLines?: string[] | null;
  label: string;
  autoFocus?: boolean;
};

const SAVE_DELAY = 400;

export function Composer({ initial, save, remove, close, suggestionLines, label, autoFocus = true }: ComposerProps) {
  const [body, setBody] = useState(initial);
  const [preview, setPreview] = useState(false);
  const [state, setState] = useState<"saved" | "saving" | "error">("saved");
  const [error, setError] = useState<string | null>(null);
  const saved = useRef(initial);
  const area = useRef<HTMLTextAreaElement>(null);
  const timer = useRef<number | undefined>(undefined);
  // Saves run one at a time: the first save of a new comment creates it, and
  // a second one must not start before the first has its id.
  const chain = useRef<Promise<void>>(Promise.resolve());

  const flush = (text: string): Promise<void> => {
    window.clearTimeout(timer.current);
    chain.current = chain.current.then(async () => {
      if (text === saved.current) return;
      setState("saving");
      try {
        await save(text);
        saved.current = text;
        setState("saved");
        setError(null);
      } catch (e) {
        setState("error");
        setError((e as Error).message);
      }
    });
    return chain.current;
  };

  useEffect(() => () => window.clearTimeout(timer.current), []);

  const onChange = (text: string) => {
    setBody(text);
    window.clearTimeout(timer.current);
    if (text.trim()) timer.current = window.setTimeout(() => void flush(text), SAVE_DELAY);
  };

  const done = async () => {
    await chain.current;
    if (!body.trim()) {
      if (remove) await remove();
      close();
      return;
    }
    await flush(body);
    close();
  };

  const insertSuggestion = () => {
    const block = suggestionBlock(suggestionLines ?? []);
    const el = area.current;
    const at = el?.selectionStart ?? body.length;
    const next = body.slice(0, at) + (at > 0 && !body.slice(0, at).endsWith("\n") ? "\n" : "") + block + body.slice(at);
    onChange(next);
    requestAnimationFrame(() => el?.focus());
  };

  return (
    <div className="composer" onClick={(e) => e.stopPropagation()}>
      <div className="composer-head small">
        <span>{label}</span>
        <span className="spacer" />
        <div className="segmented">
          <button className={preview ? "" : "on"} onClick={() => setPreview(false)}>
            Write
          </button>
          <button className={preview ? "on" : ""} onClick={() => setPreview(true)}>
            Preview
          </button>
        </div>
      </div>
      {preview ? (
        <div
          className="markdown composer-preview"
          dangerouslySetInnerHTML={{ __html: renderMarkdown(body || "_Nothing to preview._", suggestionLines ?? undefined) }}
        />
      ) : (
        <textarea
          ref={area}
          value={body}
          autoFocus={autoFocus}
          rows={Math.min(14, Math.max(3, body.split("\n").length + 1))}
          placeholder="Leave a comment (Markdown)"
          onChange={(e) => onChange(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
              e.preventDefault();
              void done();
            } else if (e.key === "Escape") {
              e.preventDefault();
              void done();
            }
          }}
        />
      )}
      <div className="composer-foot small">
        {suggestionLines && (
          <button onClick={insertSuggestion} title="Propose replacement code">
            ± Suggest change
          </button>
        )}
        <span className="muted">
          {state === "saving" ? "Saving…" : state === "error" ? "" : body.trim() ? "Saved as draft" : ""}
        </span>
        {error && <span className="error">{error}</span>}
        <span className="spacer" />
        {remove && initial && (
          <button
            onClick={async () => {
              await remove();
              close();
            }}
          >
            Delete
          </button>
        )}
        <button className="primary" onClick={() => void done()} title="⌘↩">
          Done
        </button>
      </div>
    </div>
  );
}

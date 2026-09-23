import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AuthStatus,
  Connectivity,
  CoreEvent,
  Draft,
  CommentResolution,
  FileDiff as FileDiffT,
  NewComment,
  OutboxItem,
  Proposal,
  Resolution,
  Verdict,
  FileDiff,
  PrDetail,
  PrSummary,
  SyncOutcome,
} from "./types";

/** An error from the core, with a kind the UI can branch on. */
export class ApiError extends Error {
  constructor(
    message: string,
    public kind: "offline" | "rateLimited" | "auth" | "notFound" | "invalid" | "other",
  ) {
    super(message);
  }
}

/** Running inside the desktop app (vs. a browser against `prtg-dev`). */
export const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

function toApiError(e: unknown): ApiError {
  const err = e as { message?: string; kind?: ApiError["kind"] };
  return new ApiError(err?.message ?? String(e), err?.kind ?? "other");
}

async function call<T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> {
  if (inTauri) {
    try {
      return await invoke<T>("core", { cmd, args });
    } catch (e) {
      throw toApiError(e);
    }
  }
  // Browser development: the same API over HTTP from `prtg-dev`.
  const res = await fetch(`/api/${cmd}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(args),
  });
  const body = await res.json().catch(() => null);
  if (!res.ok) throw toApiError(body ?? { message: `HTTP ${res.status}` });
  return body as T;
}

export const api = {
  authStatus: () => call<AuthStatus>("auth_status"),
  signIn: (token: string) => call<AuthStatus>("sign_in", { token }),
  signInWithGh: () => call<AuthStatus>("sign_in_with_gh"),
  signOut: () => call<void>("sign_out"),
  connectivity: () => call<Connectivity>("connectivity"),
  checkConnectivity: () => call<Connectivity>("check_connectivity"),
  setWorkOffline: (offline: boolean) => call<void>("set_work_offline", { offline }),
  addPr: (input: string) => call<number>("add_pr", { input }),
  syncPr: (prId: number) => call<SyncOutcome>("sync_pr", { prId }),
  listPrs: () => call<PrSummary[]>("list_prs"),
  getPr: (prId: number) => call<PrDetail>("get_pr", { prId }),
  fileDiff: (revisionId: number, path: string) => call<FileDiff>("file_diff", { revisionId, path }),
  markSeen: (prId: number) => call<void>("mark_seen", { prId }),
  setFileViewed: (prId: number, path: string, headBlobOid: string | null, viewed: boolean) =>
    call<void>("set_file_viewed", { prId, path, headBlobOid, viewed }),
  draft: (prId: number) => call<Draft | null>("draft", { prId }),
  addDraftComment: (prId: number, comment: NewComment) => call<Draft>("add_draft_comment", { prId, comment }),
  updateDraftComment: (commentId: number, body: string) => call<Draft>("update_draft_comment", { commentId, body }),
  deleteDraftComment: (commentId: number) => call<Draft | null>("delete_draft_comment", { commentId }),
  updateDraftReview: (prId: number, change: { body?: string; verdict?: Verdict | null }) =>
    call<Draft>("update_draft_review", { prId, ...change }),
  queueReview: (prId: number) => call<Draft>("queue_review", { prId }),
  unqueueReview: (prId: number) => call<Draft>("unqueue_review", { prId }),
  discardReview: (prId: number) => call<void>("discard_review", { prId }),
  outbox: () => call<OutboxItem[]>("outbox"),
  retryReview: (prId: number) => call<void>("retry_review", { prId }),
  exportReviewMarkdown: (prId: number) => call<string>("export_review_markdown", { prId }),
  resolveReview: (prId: number, resolution: Resolution) => call<Draft>("resolve_review", { prId, resolution }),
  draftProposals: (prId: number) =>
    call<{ commentId: number; proposal: Proposal }[]>("draft_proposals", { prId }),
  rebaseDraft: (prId: number, comments: CommentResolution[]) => call<Draft>("rebase_draft", { prId, comments }),
  interdiff: (fromRevision: number, toRevision: number) =>
    call<FileDiffT[]>("interdiff", { fromRevision, toRevision }),
};

export function onCoreEvent(cb: (e: CoreEvent) => void): Promise<UnlistenFn> {
  if (inTauri) return listen<CoreEvent>("core-event", (e) => cb(e.payload));
  const source = new EventSource("/events");
  source.onmessage = (m) => cb(JSON.parse(m.data) as CoreEvent);
  return Promise.resolve(() => source.close());
}

/** URL for a cached image or blob served by the app's `prtg:` scheme. */
export function localUrl(path: string): string {
  if (!inTauri) return `/prtg/${path}`;
  // Windows webviews reach custom schemes as http://<scheme>.localhost/.
  const windows = typeof navigator !== "undefined" && /Windows/.test(navigator.userAgent);
  return windows ? `http://prtg.localhost/${path}` : `prtg://localhost/${path}`;
}

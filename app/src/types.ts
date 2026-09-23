// Mirrors the core's read models (crates/core/src/views.rs, service.rs).

export type AuthStatus = {
  signedIn: boolean;
  login: string | null;
  source: string | null;
  scopes: string[] | null;
};

export type Connectivity = {
  online: boolean;
  workOffline: boolean;
  detail: string | null;
  rateRemaining: number | null;
};

export type SyncState = "indexed" | "fetching" | "ready" | "partial" | "stale" | "error" | "dormant";

export type PrSummary = {
  id: number;
  repo: string;
  number: number;
  title: string;
  author: string | null;
  state: "OPEN" | "CLOSED" | "MERGED";
  isDraft: boolean;
  updatedAt: string;
  syncState: SyncState;
  syncError: string | null;
  lastSyncedAt: string | null;
  additions: number;
  deletions: number;
  changedFiles: number;
  reviewDecision: string | null;
  inInbox: boolean;
  pinned: boolean;
  draftStatus: string | null;
  updatedSinceViewed: boolean;
};

export type PartialReason = { path: string; reason: string };

export type RevisionInfo = {
  id: number;
  headOid: string;
  baseOid: string;
  mergeBaseOid: string;
  fetchedAt: string;
  status: "complete" | "partial";
  partialReasons: PartialReason[];
  isForcePush: boolean | null;
};

export type FileEntry = {
  path: string;
  prevPath: string | null;
  changeType: string;
  additions: number;
  deletions: number;
  patchStatus: "ok" | "too_large" | "binary" | "missing";
  contentStatus: "ok" | "missing" | "too_large" | "binary_skipped";
  headBlobOid: string | null;
  viewed: boolean;
};

export type CommitEntry = {
  oid: string;
  headline: string;
  body: string;
  author: string | null;
  authoredAt: string | null;
};

export type CheckContext = {
  name: string;
  status: string;
  conclusion: string | null;
  url: string | null;
  description?: string | null;
};

export type CheckSnapshot = {
  capturedAt: string;
  rollupState: string | null;
  contexts: CheckContext[];
};

export type ReviewEntry = {
  nodeId: string;
  author: string | null;
  state: string;
  bodyHtml: string;
  submittedAt: string | null;
  commitOid: string | null;
};

export type CommentEntry = {
  nodeId: string;
  author: string | null;
  bodyMd: string;
  bodyHtml: string;
  createdAt: string;
  state: string | null;
  diffHunk: string | null;
  originalCommitOid: string | null;
};

export type ThreadEntry = {
  nodeId: string;
  path: string;
  subjectType: "LINE" | "FILE";
  diffSide: "LEFT" | "RIGHT" | null;
  line: number | null;
  startLine: number | null;
  startDiffSide: "LEFT" | "RIGHT" | null;
  originalLine: number | null;
  isOutdated: boolean;
  isResolved: boolean;
  viewerCanReply: boolean;
  comments: CommentEntry[];
};

export type IssueCommentEntry = {
  nodeId: string;
  author: string | null;
  bodyHtml: string;
  createdAt: string;
};

export type PrDetail = PrSummary & {
  url: string;
  bodyHtml: string;
  baseRef: string;
  headRef: string;
  locked: boolean;
  viewerDidAuthor: boolean;
  repoArchived: boolean;
  revision: RevisionInfo | null;
  files: FileEntry[];
  commits: CommitEntry[];
  checks: CheckSnapshot | null;
  reviews: ReviewEntry[];
  threads: ThreadEntry[];
  issueComments: IssueCommentEntry[];
};

export type LineKind = "context" | "add" | "del";

export type DiffLine = {
  kind: LineKind;
  oldNo: number | null;
  newNo: number | null;
  text: string;
  noNewline: boolean;
};

export type Hunk = {
  oldStart: number;
  oldLen: number;
  newStart: number;
  newLen: number;
  section: string;
  lines: DiffLine[];
};

export type FileDiff = {
  path: string;
  prevPath: string | null;
  changeType: string;
  patchStatus: FileEntry["patchStatus"];
  contentStatus: FileEntry["contentStatus"];
  hunks: Hunk[];
  source: "github" | "local" | "none";
  commentable: boolean;
  baseText: string | null;
  headText: string | null;
  baseBlobOid: string | null;
  headBlobOid: string | null;
  baseBinary: boolean;
  headBinary: boolean;
};

export type SyncOutcome = { prId: number; revisionId: number; headMoved: boolean; partial: boolean };

export type CoreEvent =
  | { type: "prUpdated"; prId: number; headMoved: boolean }
  | { type: "syncStarted"; prId: number | null; label: string }
  | { type: "syncFailed"; prId: number | null; label: string; message: string }
  | { type: "connectivity"; online: boolean; workOffline: boolean; detail: string | null }
  | { type: "outboxChanged"; prId: number; draftReviewId: number; status: string }
  | { type: "inboxChanged" };

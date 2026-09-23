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
  /** Lockfiles, `linguist-generated` and the user's patterns: starts collapsed. */
  generated: boolean;
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
  draft: Draft | null;
  lastReview: Draft | null;
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
  | { type: "inboxChanged" }
  | { type: "syncProgress"; done: number; total: number };

export type Side = "LEFT" | "RIGHT";
export type Verdict = "COMMENT" | "APPROVE" | "REQUEST_CHANGES";

export type SideSnapshot = { first: number; last: number; lines: string[]; before: string[]; after: string[] };

export type AnchorSnapshot = {
  hunkHeader: string;
  left: SideSnapshot | null;
  right: SideSnapshot | null;
  baseBlobOid: string | null;
  headBlobOid: string | null;
};

export type DraftComment = {
  id: number;
  kind: "thread" | "reply";
  subjectType: "LINE" | "FILE";
  path: string | null;
  side: Side | null;
  line: number | null;
  startSide: Side | null;
  startLine: number | null;
  replyToThread: string | null;
  bodyMd: string;
  anchorRevisionId: number | null;
  anchor: AnchorSnapshot | null;
  remapStatus: "ok" | "clean" | "fuzzy" | "not_commentable" | "orphaned";
  remapProposal: unknown;
  resolution: "remap" | "to_file" | "to_summary" | "drop" | null;
  staged: boolean;
  createdAt: string;
  updatedAt: string;
};

export type DraftStatus =
  | "draft"
  | "queued"
  | "preflight"
  | "needs_attention"
  | "staging"
  | "submitting"
  | "submitted"
  | "discarded";

export type Draft = {
  id: number;
  prId: number;
  status: DraftStatus;
  bodyMd: string;
  verdict: Verdict | null;
  basisRevisionId: number;
  targetMode: "current_head" | "reviewed_commit";
  targetRevisionId: number | null;
  attention: unknown;
  lastError: string | null;
  lastErrorKind: string | null;
  nextAttemptAt: string | null;
  queuedAt: string | null;
  submittedAt: string | null;
  submittedUrl: string | null;
  comments: DraftComment[];
};

export type NewComment = {
  revisionId: number;
  kind: "thread" | "reply";
  subjectType: "LINE" | "FILE";
  path?: string | null;
  side?: Side | null;
  line?: number | null;
  startSide?: Side | null;
  startLine?: number | null;
  replyToThread?: string | null;
  body: string;
};

export type Reason =
  | { kind: "head_moved"; from_revision: number; to_revision: number; comments: number[]; verdict_stale: boolean }
  | { kind: "pr_state_changed"; state: string }
  | { kind: "pr_locked" }
  | { kind: "repo_archived" }
  | { kind: "new_blocking_review"; review: string; author: string | null }
  | { kind: "existing_pending_review"; review: string }
  | { kind: "reply_target_gone"; comment: number }
  | { kind: "comment_rejected"; comment: number | null; message: string }
  | { kind: "reviewed_commit_unavailable"; message: string }
  | { kind: "auth"; message: string }
  | { kind: "permission"; message: string; sso_url: string | null }
  | { kind: "pr_gone" };

export type Attention = { reasons: Reason[]; acknowledged: string[] };

export type CommentAction = "remap" | "to_file" | "to_summary" | "drop" | "keep";

export type CommentResolution = {
  id: number;
  action: CommentAction;
  side?: Side | null;
  line?: number | null;
  startSide?: Side | null;
  startLine?: number | null;
  path?: string | null;
};

export type Resolution = {
  acknowledge?: string[];
  targetMode?: "current_head" | "reviewed_commit";
  comments?: CommentResolution[];
  verdict?: Verdict | null;
};

export type OutboxItem = {
  draftReviewId: number;
  prId: number;
  repo: string;
  number: number;
  title: string;
  status: DraftStatus;
  verdict: Verdict | null;
  comments: number;
  queuedAt: string | null;
  submittedAt: string | null;
  submittedUrl: string | null;
  lastError: string | null;
  lastErrorKind: string | null;
  nextAttemptAt: string | null;
  attention: Attention | null;
};

export type RemapStatus = "clean" | "fuzzy" | "not_commentable" | "orphaned";

export type Proposal = {
  status: RemapStatus;
  path: string | null;
  side: Side | null;
  line: number | null;
  startSide: Side | null;
  startLine: number | null;
  confidence: number;
  oldLines: string[];
  newLines: string[];
  suggestionStale: boolean;
  fileInDiff: boolean;
  toRevision: number;
};

export type Subscription = {
  id: number;
  kind: "repo" | "search";
  repo: string | null;
  query: string | null;
  label: string;
  enabled: boolean;
  lastPolledAt: string | null;
  lastError: string | null;
  prs: number;
};

export type Readiness = {
  total: number;
  ready: number;
  partial: number;
  notSynced: number;
  failed: number;
  bytes: number;
};

export type InboxSync = { polled: number; synced: number; failed: number; errors: string[] };

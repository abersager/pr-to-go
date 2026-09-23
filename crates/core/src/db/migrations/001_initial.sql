-- Initial schema. See docs/DESIGN.md §7.
--
-- Two kinds of tables:
--   (M) mirror tables hold what GitHub told us. Sync may replace them freely.
--   (U) user tables hold what the user wrote or decided. Sync never writes them.
--
-- No foreign key cascades from a mirror row into a user row. User rows point at
-- mirror rows (the PR, the revision they were written against) *without*
-- cascade, so a buggy GC that tries to delete a pinned revision or PR fails with
-- a constraint error instead of silently deleting the user's drafts.

-- ─── Settings & account ───────────────────────────────────────────────────

CREATE TABLE setting (                                           -- (U)
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
) STRICT;

CREATE TABLE account (                                           -- (M)
  id           INTEGER PRIMARY KEY,
  node_id      TEXT NOT NULL UNIQUE,
  login        TEXT NOT NULL,
  token_source TEXT NOT NULL CHECK (token_source IN ('pat', 'gh')),
  scopes       TEXT,                -- JSON array, as reported by X-OAuth-Scopes
  verified_at  TEXT NOT NULL
) STRICT;

CREATE TABLE subscription (                                      -- (U)
  id                INTEGER PRIMARY KEY,
  kind              TEXT NOT NULL CHECK (kind IN ('repo', 'search')),
  repo_full_name    TEXT,           -- kind = 'repo'
  query             TEXT,           -- kind = 'search'
  label             TEXT NOT NULL,
  enabled           INTEGER NOT NULL DEFAULT 1,
  last_polled_at    TEXT,           -- local clock
  cursor_updated_at TEXT,           -- newest PR updatedAt seen (server clock)
  last_error        TEXT,           -- or a warning, such as truncated search results
  created_at        TEXT NOT NULL,
  CHECK ((kind = 'repo' AND repo_full_name IS NOT NULL) OR (kind = 'search' AND query IS NOT NULL))
) STRICT;

-- Persistent ETag cache for REST GETs, so a restart doesn't make every request
-- count against the rate limit again.
CREATE TABLE http_cache (                                        -- (M)
  url_key   TEXT PRIMARY KEY,
  etag      TEXT NOT NULL,
  body      BLOB NOT NULL,
  stored_at TEXT NOT NULL
) STRICT;

-- ─── Repos & PRs ──────────────────────────────────────────────────────────

CREATE TABLE repo (                                              -- (M)
  id                INTEGER PRIMARY KEY,
  node_id           TEXT NOT NULL UNIQUE,
  owner             TEXT NOT NULL,
  name              TEXT NOT NULL,
  is_private        INTEGER NOT NULL DEFAULT 0,
  is_archived       INTEGER NOT NULL DEFAULT 0,
  viewer_permission TEXT,
  UNIQUE (owner, name)
) STRICT;

CREATE TABLE pull_request (                                      -- (M)
  id                  INTEGER PRIMARY KEY,
  node_id             TEXT NOT NULL UNIQUE,
  repo_id             INTEGER NOT NULL REFERENCES repo(id),
  number              INTEGER NOT NULL,
  title               TEXT NOT NULL,
  author_login        TEXT,
  state               TEXT NOT NULL,          -- OPEN | CLOSED | MERGED
  is_draft            INTEGER NOT NULL DEFAULT 0,
  locked              INTEGER NOT NULL DEFAULT 0,
  base_ref_name       TEXT NOT NULL,
  head_ref_name       TEXT NOT NULL,
  head_repo_full_name TEXT,
  body_md             TEXT NOT NULL DEFAULT '',
  body_html           TEXT NOT NULL DEFAULT '', -- GitHub's HTML, images rewritten to local assets
  review_decision     TEXT,
  url                 TEXT NOT NULL,
  viewer_did_author   INTEGER NOT NULL DEFAULT 0,
  additions           INTEGER NOT NULL DEFAULT 0,
  deletions           INTEGER NOT NULL DEFAULT 0,
  changed_files       INTEGER NOT NULL DEFAULT 0,
  created_at          TEXT NOT NULL,
  updated_at          TEXT NOT NULL,          -- server clock
  current_revision_id INTEGER REFERENCES pr_revision(id),  -- newest complete revision
  index_head_oid      TEXT,                   -- from the inbox index; may be ahead of current
  index_updated_at    TEXT,
  sync_state          TEXT NOT NULL DEFAULT 'indexed'
                      CHECK (sync_state IN ('indexed', 'fetching', 'ready', 'partial', 'stale', 'error', 'dormant')),
  sync_error          TEXT,
  last_synced_at      TEXT,                   -- local clock
  in_inbox            INTEGER NOT NULL DEFAULT 0,
  left_inbox_at       TEXT,
  UNIQUE (repo_id, number)
) STRICT;

-- Which PRs each subscription currently includes. `pull_request.in_inbox`
-- is derived from this after every poll.
CREATE TABLE subscription_member (                               -- (M)
  subscription_id INTEGER NOT NULL REFERENCES subscription(id) ON DELETE CASCADE,
  pr_id           INTEGER NOT NULL REFERENCES pull_request(id) ON DELETE CASCADE,
  PRIMARY KEY (subscription_id, pr_id)
) STRICT;

CREATE TABLE pr_local (                                          -- (U)
  pr_id                   INTEGER PRIMARY KEY REFERENCES pull_request(id),
  pinned                  INTEGER NOT NULL DEFAULT 0,  -- added by hand; never pruned
  starred                 INTEGER NOT NULL DEFAULT 0,
  hidden                  INTEGER NOT NULL DEFAULT 0,
  last_viewed_revision_id INTEGER REFERENCES pr_revision(id)
) STRICT;

CREATE TABLE file_viewed (                                       -- (U)
  pr_id         INTEGER NOT NULL REFERENCES pull_request(id),
  path          TEXT NOT NULL,
  head_blob_oid TEXT,               -- the checkbox resets when the file's contents change
  PRIMARY KEY (pr_id, path)
) STRICT;

-- ─── Revisions & content ──────────────────────────────────────────────────

-- One immutable snapshot per distinct (head, merge base) of a PR.
CREATE TABLE pr_revision (                                       -- (M)
  id               INTEGER PRIMARY KEY,
  pr_id            INTEGER NOT NULL REFERENCES pull_request(id) ON DELETE CASCADE,
  head_oid         TEXT NOT NULL,
  base_oid         TEXT NOT NULL,
  merge_base_oid   TEXT NOT NULL,
  fetched_at       TEXT NOT NULL,   -- local clock
  status           TEXT NOT NULL CHECK (status IN ('fetching', 'complete', 'partial')),
  partial_reasons  TEXT,            -- JSON: [{path, reason}]
  prev_revision_id INTEGER REFERENCES pr_revision(id) ON DELETE SET NULL,
  is_force_push    INTEGER,         -- 1 if the previous head is not in the new commit list
  UNIQUE (pr_id, head_oid, merge_base_oid)
) STRICT;

CREATE TABLE revision_file (                                     -- (M)
  revision_id    INTEGER NOT NULL REFERENCES pr_revision(id) ON DELETE CASCADE,
  position       INTEGER NOT NULL,
  path           TEXT NOT NULL,
  prev_path      TEXT,
  change_type    TEXT NOT NULL,     -- added | removed | modified | renamed | copied | changed
  additions      INTEGER NOT NULL DEFAULT 0,
  deletions      INTEGER NOT NULL DEFAULT 0,
  base_blob_oid  TEXT,
  head_blob_oid  TEXT,
  patch          TEXT,              -- GitHub's unified patch for this file, if it sent one
  patch_status   TEXT NOT NULL CHECK (patch_status IN ('ok', 'too_large', 'binary', 'missing')),
  content_status TEXT NOT NULL DEFAULT 'ok'
                 CHECK (content_status IN ('ok', 'missing', 'too_large', 'binary_skipped')),
  PRIMARY KEY (revision_id, path)
) STRICT;

-- File contents keyed by git blob id. Small blobs inline, large ones on disk.
CREATE TABLE blob (                                              -- (M)
  oid        TEXT PRIMARY KEY,
  byte_size  INTEGER NOT NULL,
  is_binary  INTEGER NOT NULL,
  location   TEXT NOT NULL CHECK (location IN ('inline', 'file')),
  content    BLOB,
  fetched_at TEXT NOT NULL
) STRICT;

CREATE TABLE pr_commit (                                         -- (M)
  revision_id      INTEGER NOT NULL REFERENCES pr_revision(id) ON DELETE CASCADE,
  position         INTEGER NOT NULL,
  oid              TEXT NOT NULL,
  message_headline TEXT NOT NULL,
  message_body     TEXT NOT NULL DEFAULT '',
  author_login     TEXT,
  author_name      TEXT,
  authored_at      TEXT,
  PRIMARY KEY (revision_id, position)
) STRICT;

CREATE TABLE check_snapshot (                                    -- (M)
  revision_id  INTEGER PRIMARY KEY REFERENCES pr_revision(id) ON DELETE CASCADE,
  captured_at  TEXT NOT NULL,       -- local clock
  rollup_state TEXT,
  contexts     TEXT NOT NULL        -- JSON: [{name, status, conclusion, url}]
) STRICT;

-- ─── GitHub discussion (mirror) ───────────────────────────────────────────

CREATE TABLE review (                                            -- (M)
  id           INTEGER PRIMARY KEY,
  node_id      TEXT NOT NULL UNIQUE,
  pr_id        INTEGER NOT NULL REFERENCES pull_request(id) ON DELETE CASCADE,
  author_login TEXT,
  state        TEXT NOT NULL,       -- APPROVED | CHANGES_REQUESTED | COMMENTED | DISMISSED | PENDING
  body_md      TEXT NOT NULL DEFAULT '',
  body_html    TEXT NOT NULL DEFAULT '',
  submitted_at TEXT,
  commit_oid   TEXT
) STRICT;

CREATE TABLE review_thread (                                     -- (M)
  id                  INTEGER PRIMARY KEY,
  node_id             TEXT NOT NULL UNIQUE,
  pr_id               INTEGER NOT NULL REFERENCES pull_request(id) ON DELETE CASCADE,
  path                TEXT NOT NULL,
  subject_type        TEXT NOT NULL,  -- LINE | FILE
  diff_side           TEXT,
  line                INTEGER,
  start_line          INTEGER,
  start_diff_side     TEXT,
  original_line       INTEGER,
  original_start_line INTEGER,
  is_outdated         INTEGER NOT NULL,
  is_resolved         INTEGER NOT NULL,
  viewer_can_reply    INTEGER NOT NULL
) STRICT;

CREATE TABLE review_comment (                                    -- (M)
  id                  INTEGER PRIMARY KEY,
  node_id             TEXT NOT NULL UNIQUE,
  thread_id           INTEGER NOT NULL REFERENCES review_thread(id) ON DELETE CASCADE,
  position            INTEGER NOT NULL,
  review_node_id      TEXT,
  author_login        TEXT,
  body_md             TEXT NOT NULL,
  body_html           TEXT NOT NULL,
  diff_hunk           TEXT,
  commit_oid          TEXT,
  original_commit_oid TEXT,
  state               TEXT,           -- PENDING marks the viewer's own pending comments from the web
  created_at          TEXT NOT NULL,
  updated_at          TEXT
) STRICT;

CREATE TABLE issue_comment (                                     -- (M)
  id           INTEGER PRIMARY KEY,
  node_id      TEXT NOT NULL UNIQUE,
  pr_id        INTEGER NOT NULL REFERENCES pull_request(id) ON DELETE CASCADE,
  author_login TEXT,
  body_md      TEXT NOT NULL,
  body_html    TEXT NOT NULL,
  created_at   TEXT NOT NULL
) STRICT;

CREATE TABLE asset (                                             -- (M)
  sha256       TEXT PRIMARY KEY,
  source_url   TEXT NOT NULL,
  content_type TEXT,
  byte_size    INTEGER NOT NULL,
  fetched_at   TEXT NOT NULL
) STRICT;

CREATE TABLE pr_asset (                                          -- (M)
  pr_id  INTEGER NOT NULL REFERENCES pull_request(id) ON DELETE CASCADE,
  sha256 TEXT NOT NULL REFERENCES asset(sha256),
  PRIMARY KEY (pr_id, sha256)
) STRICT;

-- ─── Drafts & outbox (user) ───────────────────────────────────────────────

CREATE TABLE draft_review (                                      -- (U)
  id                       INTEGER PRIMARY KEY,
  pr_id                    INTEGER NOT NULL REFERENCES pull_request(id),
  basis_revision_id        INTEGER NOT NULL REFERENCES pr_revision(id),
  basis_review_cursor      TEXT,    -- newest review submittedAt known at basis (server clock)
  body_md                  TEXT NOT NULL DEFAULT '',
  verdict                  TEXT CHECK (verdict IN ('COMMENT', 'APPROVE', 'REQUEST_CHANGES')),
  status                   TEXT NOT NULL CHECK (status IN (
                             'draft', 'queued', 'preflight', 'needs_attention',
                             'staging', 'submitting', 'submitted', 'discarded')),
  attention                TEXT,    -- JSON: reasons, acknowledgements
  target_mode              TEXT NOT NULL DEFAULT 'current_head'
                           CHECK (target_mode IN ('current_head', 'reviewed_commit')),
  target_revision_id       INTEGER REFERENCES pr_revision(id),
  server_pending_review_id TEXT,
  pending_review_owned     INTEGER NOT NULL DEFAULT 0,
  preflight_pending_seen   TEXT,    -- pending review id seen at preflight, '' for none
  attempts                 INTEGER NOT NULL DEFAULT 0,
  next_attempt_at          TEXT,    -- local clock
  last_error_kind          TEXT,
  last_error               TEXT,
  queued_at                TEXT,
  submitted_at             TEXT,
  submitted_review_node_id TEXT,
  submitted_url            TEXT,
  cleanup_review_id        TEXT,    -- our pending review to delete once online (after discard)
  inflight                 TEXT,    -- mutation sent but its result not yet stored: reconcile first
  created_at               TEXT NOT NULL,
  updated_at               TEXT NOT NULL
) STRICT;

-- GitHub allows one pending review per user per PR; we allow one active draft.
CREATE UNIQUE INDEX draft_review_one_active_per_pr
  ON draft_review(pr_id) WHERE status NOT IN ('submitted', 'discarded');

CREATE TABLE draft_comment (                                     -- (U)
  id                      INTEGER PRIMARY KEY,
  draft_review_id         INTEGER NOT NULL REFERENCES draft_review(id) ON DELETE CASCADE,
  kind                    TEXT NOT NULL CHECK (kind IN ('thread', 'reply')),
  reply_to_thread_node_id TEXT,
  path                    TEXT,
  subject_type            TEXT NOT NULL CHECK (subject_type IN ('LINE', 'FILE')),
  side                    TEXT CHECK (side IN ('LEFT', 'RIGHT')),
  line                    INTEGER,
  start_side              TEXT CHECK (start_side IN ('LEFT', 'RIGHT')),
  start_line              INTEGER,
  anchor_revision_id      INTEGER REFERENCES pr_revision(id),
  anchor_snapshot         TEXT,     -- JSON: anchored lines + context, see remap.rs
  body_md                 TEXT NOT NULL,
  remap_status            TEXT NOT NULL DEFAULT 'ok'
                          CHECK (remap_status IN ('ok', 'clean', 'fuzzy', 'not_commentable', 'orphaned')),
  remap_proposal          TEXT,     -- JSON
  resolution              TEXT CHECK (resolution IN ('remap', 'to_file', 'to_summary', 'drop')),
  staged_node_id          TEXT,     -- the comment's id on our pending review
  staged_thread_node_id   TEXT,
  created_at              TEXT NOT NULL,
  updated_at              TEXT NOT NULL,
  CHECK (kind = 'thread' OR reply_to_thread_node_id IS NOT NULL),
  CHECK (subject_type = 'FILE' OR kind = 'reply' OR (line IS NOT NULL AND side IS NOT NULL))
) STRICT;

CREATE INDEX draft_comment_by_review ON draft_comment(draft_review_id);

CREATE TABLE outbox_log (                                        -- (U)
  id              INTEGER PRIMARY KEY,
  draft_review_id INTEGER NOT NULL REFERENCES draft_review(id) ON DELETE CASCADE,
  at              TEXT NOT NULL,
  step            TEXT NOT NULL,
  outcome         TEXT NOT NULL,
  detail          TEXT
) STRICT;

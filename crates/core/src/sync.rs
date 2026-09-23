//! Deep sync of one PR (DESIGN.md §9.2).
//!
//! Everything is fetched first; then one short transaction writes the new
//! revision and flips `current_revision_id`. Blobs are content-addressed and
//! written as they arrive, so an interrupted sync resumes without refetching
//! them, and the UI never sees a half-synced revision.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use futures_util::{StreamExt, stream};
use rusqlite::{OptionalExtension, Transaction, params};
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::blobstore::BlobStore;
use crate::clock::{Clock, rfc3339};
use crate::db::Db;
use crate::error::{Error, Result};
use crate::github::queries::*;
use crate::github::{GhError, GitHub, OpKind};

/// Blobs larger than this aren't stored (the file shows as unavailable).
const MAX_BLOB: i64 = 50 * 1024 * 1024;
/// Binary blobs are only fetched if they're images below this size.
const MAX_IMAGE_BLOB: i64 = 5 * 1024 * 1024;
const MAX_ASSET: usize = 10 * 1024 * 1024;
const MAX_ASSETS_PER_PR: usize = 50 * 1024 * 1024;
const BLOB_BATCH: usize = 25;
/// Blob batches in flight at once for one PR (the client's own limit on
/// concurrent requests still applies across all PRs).
const BLOB_BATCHES_IN_FLIGHT: usize = 4;

/// URL scheme the UI serves local images and blobs from.
pub const ASSET_URL_PREFIX: &str = "prtg://localhost/asset/";

#[derive(Clone)]
pub struct SyncCtx {
    pub db: Arc<Db>,
    pub gh: Arc<GitHub>,
    pub blobs: BlobStore,
    pub clock: Arc<dyn Clock>,
    pub assets_dir: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PrRef {
    pub owner: String,
    pub name: String,
    pub number: i64,
}

impl PrRef {
    /// Accepts `https://github.com/o/r/pull/5[/files…]`, `o/r#5` and `o/r/pull/5`.
    pub fn parse(input: &str) -> Option<PrRef> {
        let s = input.trim();
        let s = s.strip_prefix("https://").or_else(|| s.strip_prefix("http://")).unwrap_or(s);
        let s = s.strip_prefix("www.").unwrap_or(s);
        let s = s.strip_prefix("github.com/").unwrap_or(s);
        let parts: Vec<&str> = s.split('/').collect();
        if parts.len() >= 4 && (parts[2] == "pull" || parts[2] == "pulls") {
            let number = parts[3].split(['?', '#']).next()?.parse().ok()?;
            return Some(PrRef { owner: parts[0].into(), name: parts[1].into(), number });
        }
        let (repo, num) = s.split_once('#')?;
        let (owner, name) = repo.split_once('/')?;
        if name.contains('/') {
            return None;
        }
        Some(PrRef { owner: owner.into(), name: name.into(), number: num.trim().parse().ok()? })
    }

    pub fn display(&self) -> String {
        format!("{}/{}#{}", self.owner, self.name, self.number)
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncOutcome {
    pub pr_id: i64,
    pub revision_id: i64,
    /// The head or merge base differs from the previous current revision.
    pub head_moved: bool,
    pub partial: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct PartialReason {
    pub path: String,
    pub reason: String,
}

struct FileRow {
    path: String,
    prev_path: Option<String>,
    change_type: String,
    additions: i64,
    deletions: i64,
    base_blob_oid: Option<String>,
    head_blob_oid: Option<String>,
    patch: Option<String>,
    patch_status: &'static str,
    content_status: &'static str,
}

struct NewRevision {
    files: Vec<FileRow>,
    commits: Vec<CommitInfo>,
    partial: Vec<PartialReason>,
    /// The head's root `.gitattributes`, if it has one.
    gitattributes: Option<String>,
}

enum FilesResult {
    /// Files, why the revision is partial, and the root `.gitattributes`.
    Done(Vec<FileRow>, Vec<PartialReason>, Option<String>),
    /// The PR's head moved while we were fetching; start over.
    HeadMoved,
}

pub async fn sync_pr(ctx: &SyncCtx, pr: &PrRef) -> Result<SyncOutcome> {
    let started = std::time::Instant::now();
    for _ in 0..3 {
        match sync_once(ctx, pr).await {
            Ok(Some(outcome)) => {
                tracing::info!(
                    "synced {} in {} ms{}{}",
                    pr.display(),
                    started.elapsed().as_millis(),
                    if outcome.head_moved { ", new revision" } else { "" },
                    if outcome.partial { ", partial" } else { "" },
                );
                return Ok(outcome);
            }
            Ok(None) => tracing::info!("{} moved during sync; retrying", pr.display()),
            Err(e) => {
                tracing::info!(
                    "sync of {} failed after {} ms: {e}",
                    pr.display(),
                    started.elapsed().as_millis()
                );
                return Err(e);
            }
        }
    }
    Err(Error::GitHub(GhError::Server(409)))
}

async fn sync_once(ctx: &SyncCtx, r: &PrRef) -> Result<Option<SyncOutcome>> {
    let gh = &ctx.gh;
    // 1. Metadata.
    let data: DetailsData = gh
        .graphql(
            "PullRequestDetails",
            PULL_REQUEST_DETAILS,
            json!({ "owner": r.owner, "name": r.name, "number": r.number }),
            OpKind::Query,
        )
        .await?;
    let repo = data.repository.ok_or(GhError::NotFound)?;
    let pr = repo.pull_request.as_ref().ok_or(GhError::NotFound)?;

    struct Local {
        pr_id: i64,
        current: Option<(i64, String, String, String)>,
    }
    let local: Option<Local> = ctx.db.read(|c| {
        let row: Option<(i64, Option<i64>)> = c
            .query_row("SELECT id, current_revision_id FROM pull_request WHERE node_id = ?1", [&pr.id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?;
        let Some((pr_id, cur)) = row else {
            return Ok(None);
        };
        let current = match cur {
            Some(rid) => c
                .query_row(
                    "SELECT id, head_oid, base_oid, merge_base_oid FROM pr_revision WHERE id = ?1",
                    [rid],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()?,
            None => None,
        };
        Ok(Some(Local { pr_id, current }))
    })?;

    // 2. Merge base. Only a new head or base can move it.
    let merge_base = match local.as_ref().and_then(|l| l.current.as_ref()) {
        Some((_, head, base, mb)) if *head == pr.head_ref_oid && *base == pr.base_ref_oid => mb.clone(),
        _ => {
            let cmp: RestCompare = gh
                .rest_get_json(
                    &format!(
                        "repos/{}/{}/compare/{}...{}?per_page=1",
                        r.owner, r.name, pr.base_ref_oid, pr.head_ref_oid
                    ),
                    true,
                )
                .await?;
            cmp.merge_base_commit.sha
        }
    };

    let existing_rev: Option<i64> = match &local {
        Some(l) => ctx.db.read(|c| {
            Ok(c.query_row(
                "SELECT id FROM pr_revision WHERE pr_id = ?1 AND head_oid = ?2 AND merge_base_oid = ?3",
                params![l.pr_id, pr.head_ref_oid, merge_base],
                |r| r.get(0),
            )
            .optional()?)
        })?,
        None => None,
    };

    // 3–5. Files, patches and blobs (only for a revision we don't have),
    // commits, and the discussion, fetched side by side.
    let revision = async {
        if existing_rev.is_some() {
            return Ok(None);
        }
        let (files, commits) = tokio::try_join!(
            fetch_files(ctx, r, &pr.head_ref_oid, &merge_base, local.as_ref().map(|l| l.pr_id)),
            fetch_commits(gh, &pr.id),
        )?;
        Ok::<_, Error>(Some((files, commits)))
    };
    let (revision, reviews, threads, issue_comments) = tokio::try_join!(
        revision,
        fetch_reviews(gh, &pr.id),
        fetch_threads(gh, &pr.id),
        fetch_issue_comments(gh, &pr.id),
    )?;
    let new_rev = match revision {
        None => None,
        Some((FilesResult::HeadMoved, _)) => return Ok(None),
        Some((FilesResult::Done(files, partial, gitattributes), commits)) => {
            Some(NewRevision { files, commits, partial, gitattributes })
        }
    };

    // 7. Images in all rendered HTML.
    let mut html: Vec<&str> = vec![&pr.body_html];
    html.extend(reviews.iter().map(|x| x.body_html.as_str()));
    html.extend(threads.iter().flat_map(|t| t.comments.nodes.iter().map(|c| c.body_html.as_str())));
    html.extend(issue_comments.iter().map(|x| x.body_html.as_str()));
    let assets = download_images(ctx, &html).await;
    let localize = |h: &str| rewrite_images(h, &assets);

    // 8. One transaction.
    let now = rfc3339(ctx.clock.now());
    let checks = pr.last_commit.nodes.first().and_then(|n| n.commit.status_check_rollup.as_ref());
    let prev_rev = local.as_ref().and_then(|l| l.current.as_ref().map(|c| c.0));
    let prev_head = local.as_ref().and_then(|l| l.current.as_ref().map(|c| c.1.clone()));
    let prev_mb = local.as_ref().and_then(|l| l.current.as_ref().map(|c| c.3.clone()));
    let head_moved = prev_head.is_some()
        && (prev_head.as_deref() != Some(&pr.head_ref_oid) || prev_mb.as_deref() != Some(&merge_base));

    let outcome = ctx.db.write(|tx| {
        let repo_id = upsert_repo(tx, &repo)?;
        let pr_id = upsert_pr(tx, repo_id, pr, &localize(&pr.body_html), &now)?;
        let (rev_id, partial) = match (existing_rev, &new_rev) {
            (Some(id), _) => {
                tx.execute("UPDATE pr_revision SET base_oid = ?2 WHERE id = ?1", params![id, pr.base_ref_oid])?;
                let partial: bool =
                    tx.query_row("SELECT status = 'partial' FROM pr_revision WHERE id = ?1", [id], |r| r.get(0))?;
                (id, partial)
            }
            (None, Some(nr)) => {
                let is_force_push = prev_head.as_ref().map(|h| !nr.commits.iter().any(|c| &c.oid == h));
                let id = insert_revision(tx, pr_id, pr, &merge_base, &now, nr, prev_rev, is_force_push)?;
                (id, !nr.partial.is_empty())
            }
            (None, None) => unreachable!(),
        };
        if let Some(nr) = &new_rev {
            tx.execute("UPDATE pr_revision SET gitattributes = ?2 WHERE id = ?1", params![rev_id, nr.gitattributes])?;
        }
        tx.execute(
            "INSERT INTO check_snapshot (revision_id, captured_at, rollup_state, contexts) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (revision_id) DO UPDATE SET captured_at = excluded.captured_at,
               rollup_state = excluded.rollup_state, contexts = excluded.contexts",
            params![rev_id, now, checks.map(|c| c.state.clone()), checks_json(checks)],
        )?;
        tx.execute(
            "UPDATE pull_request SET current_revision_id = ?2, sync_state = ?3, sync_error = NULL,
               last_synced_at = ?4, index_head_oid = ?5 WHERE id = ?1",
            params![pr_id, rev_id, if partial { "partial" } else { "ready" }, now, pr.head_ref_oid],
        )?;
        write_discussion(tx, pr_id, &reviews, &threads, &issue_comments, &localize)?;
        for sha in assets.values() {
            tx.execute("INSERT OR IGNORE INTO pr_asset (pr_id, sha256) VALUES (?1, ?2)", params![pr_id, sha])?;
        }
        Ok(SyncOutcome { pr_id, revision_id: rev_id, head_moved, partial })
    })?;
    Ok(Some(outcome))
}

pub(crate) fn checks_json(rollup: Option<&Rollup>) -> String {
    let contexts: Vec<serde_json::Value> = rollup
        .map(|r| {
            r.contexts
                .nodes
                .iter()
                .map(|c| match c {
                    CheckContext::CheckRun { name, status, conclusion, details_url } => json!({
                        "name": name, "status": status, "conclusion": conclusion, "url": details_url,
                    }),
                    CheckContext::StatusContext { context, state, target_url, description } => json!({
                        "name": context, "status": "COMPLETED", "conclusion": state, "url": target_url,
                        "description": description,
                    }),
                })
                .collect()
        })
        .unwrap_or_default();
    serde_json::Value::Array(contexts).to_string()
}

fn upsert_repo(tx: &Transaction, repo: &RepoDetails) -> Result<i64> {
    tx.execute(
        "INSERT INTO repo (node_id, owner, name, is_private, is_archived, viewer_permission)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (node_id) DO UPDATE SET owner = excluded.owner, name = excluded.name,
           is_private = excluded.is_private, is_archived = excluded.is_archived,
           viewer_permission = excluded.viewer_permission",
        params![
            repo.id,
            repo.owner.login,
            repo.name,
            repo.is_private,
            repo.is_archived,
            repo.viewer_permission
        ],
    )?;
    Ok(tx.query_row("SELECT id FROM repo WHERE node_id = ?1", [&repo.id], |r| r.get(0))?)
}

fn upsert_pr(tx: &Transaction, repo_id: i64, pr: &PrDetails, body_html: &str, now: &str) -> Result<i64> {
    tx.execute(
        "INSERT INTO pull_request (node_id, repo_id, number, title, author_login, state, is_draft, locked,
           base_ref_name, head_ref_name, head_repo_full_name, body_md, body_html, review_decision, url,
           viewer_did_author, additions, deletions, changed_files, created_at, updated_at, sync_state,
           last_synced_at, in_inbox)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21,
           'fetching', ?22, 0)
         ON CONFLICT (node_id) DO UPDATE SET repo_id = excluded.repo_id, number = excluded.number,
           title = excluded.title, author_login = excluded.author_login, state = excluded.state,
           is_draft = excluded.is_draft, locked = excluded.locked, base_ref_name = excluded.base_ref_name,
           head_ref_name = excluded.head_ref_name, head_repo_full_name = excluded.head_repo_full_name,
           body_md = excluded.body_md, body_html = excluded.body_html,
           review_decision = excluded.review_decision, url = excluded.url,
           viewer_did_author = excluded.viewer_did_author, additions = excluded.additions,
           deletions = excluded.deletions, changed_files = excluded.changed_files,
           updated_at = excluded.updated_at",
        params![
            pr.id,
            repo_id,
            pr.number,
            pr.title,
            pr.author.as_ref().map(|a| &a.login),
            pr.state,
            pr.is_draft,
            pr.locked,
            pr.base_ref_name,
            pr.head_ref_name,
            pr.head_repository.as_ref().map(|h| &h.name_with_owner),
            pr.body.replace('\r', ""),
            body_html,
            pr.review_decision,
            pr.url,
            pr.viewer_did_author,
            pr.additions,
            pr.deletions,
            pr.changed_files,
            pr.created_at,
            pr.updated_at,
            now,
        ],
    )?;
    Ok(tx.query_row("SELECT id FROM pull_request WHERE node_id = ?1", [&pr.id], |r| r.get(0))?)
}

#[allow(clippy::too_many_arguments)]
fn insert_revision(
    tx: &Transaction,
    pr_id: i64,
    pr: &PrDetails,
    merge_base: &str,
    now: &str,
    nr: &NewRevision,
    prev: Option<i64>,
    is_force_push: Option<bool>,
) -> Result<i64> {
    let partial_json = if nr.partial.is_empty() { None } else { Some(serde_json::to_string(&nr.partial)?) };
    tx.execute(
        "INSERT INTO pr_revision (pr_id, head_oid, base_oid, merge_base_oid, fetched_at, status, partial_reasons,
           prev_revision_id, is_force_push) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            pr_id,
            pr.head_ref_oid,
            pr.base_ref_oid,
            merge_base,
            now,
            if nr.partial.is_empty() { "complete" } else { "partial" },
            partial_json,
            prev,
            is_force_push,
        ],
    )?;
    let rev_id = tx.last_insert_rowid();
    for (i, f) in nr.files.iter().enumerate() {
        tx.execute(
            "INSERT INTO revision_file (revision_id, position, path, prev_path, change_type, additions, deletions,
               base_blob_oid, head_blob_oid, patch, patch_status, content_status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                rev_id,
                i as i64,
                f.path,
                f.prev_path,
                f.change_type,
                f.additions,
                f.deletions,
                f.base_blob_oid,
                f.head_blob_oid,
                f.patch,
                f.patch_status,
                f.content_status,
            ],
        )?;
    }
    for (i, c) in nr.commits.iter().enumerate() {
        tx.execute(
            "INSERT INTO pr_commit (revision_id, position, oid, message_headline, message_body, author_login,
               author_name, authored_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                rev_id,
                i as i64,
                c.oid,
                c.message_headline,
                c.message_body,
                c.author.as_ref().and_then(|a| a.user.as_ref()).map(|u| &u.login),
                c.author.as_ref().and_then(|a| a.name.as_ref()),
                c.authored_date,
            ],
        )?;
    }
    Ok(rev_id)
}

fn write_discussion(
    tx: &Transaction,
    pr_id: i64,
    reviews: &[ReviewInfo],
    threads: &[ThreadInfo],
    issue_comments: &[IssueCommentInfo],
    localize: &dyn Fn(&str) -> String,
) -> Result<()> {
    let mut keep: HashSet<&str> = HashSet::new();
    for rv in reviews {
        keep.insert(&rv.id);
        tx.execute(
            "INSERT INTO review (node_id, pr_id, author_login, state, body_md, body_html, submitted_at, commit_oid)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT (node_id) DO UPDATE SET state = excluded.state, body_md = excluded.body_md,
               body_html = excluded.body_html, submitted_at = excluded.submitted_at, commit_oid = excluded.commit_oid",
            params![
                rv.id,
                pr_id,
                rv.author.as_ref().map(|a| &a.login),
                rv.state,
                rv.body.replace('\r', ""),
                localize(&rv.body_html),
                rv.submitted_at,
                rv.commit.as_ref().map(|c| &c.oid),
            ],
        )?;
    }
    delete_missing(tx, "review", pr_id, &keep)?;

    let mut keep_threads: HashSet<&str> = HashSet::new();
    let mut keep_comments: HashSet<&str> = HashSet::new();
    for t in threads {
        keep_threads.insert(&t.id);
        tx.execute(
            "INSERT INTO review_thread (node_id, pr_id, path, subject_type, diff_side, line, start_line,
               start_diff_side, original_line, original_start_line, is_outdated, is_resolved, viewer_can_reply)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT (node_id) DO UPDATE SET path = excluded.path, subject_type = excluded.subject_type,
               diff_side = excluded.diff_side, line = excluded.line, start_line = excluded.start_line,
               start_diff_side = excluded.start_diff_side, original_line = excluded.original_line,
               original_start_line = excluded.original_start_line, is_outdated = excluded.is_outdated,
               is_resolved = excluded.is_resolved, viewer_can_reply = excluded.viewer_can_reply",
            params![
                t.id,
                pr_id,
                t.path,
                t.subject_type,
                t.diff_side,
                t.line,
                t.start_line,
                t.start_diff_side,
                t.original_line,
                t.original_start_line,
                t.is_outdated,
                t.is_resolved,
                t.viewer_can_reply,
            ],
        )?;
        let thread_id: i64 =
            tx.query_row("SELECT id FROM review_thread WHERE node_id = ?1", [&t.id], |r| r.get(0))?;
        for (i, c) in t.comments.nodes.iter().enumerate() {
            keep_comments.insert(&c.id);
            tx.execute(
                "INSERT INTO review_comment (node_id, thread_id, position, review_node_id, author_login, body_md,
                   body_html, diff_hunk, commit_oid, original_commit_oid, state, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                 ON CONFLICT (node_id) DO UPDATE SET thread_id = excluded.thread_id, position = excluded.position,
                   body_md = excluded.body_md, body_html = excluded.body_html, diff_hunk = excluded.diff_hunk,
                   commit_oid = excluded.commit_oid, state = excluded.state, updated_at = excluded.updated_at",
                params![
                    c.id,
                    thread_id,
                    i as i64,
                    c.pull_request_review.as_ref().map(|r| &r.id),
                    c.author.as_ref().map(|a| &a.login),
                    c.body.replace('\r', ""),
                    localize(&c.body_html),
                    c.diff_hunk,
                    c.commit.as_ref().map(|x| &x.oid),
                    c.original_commit.as_ref().map(|x| &x.oid),
                    c.state,
                    c.created_at,
                    c.updated_at,
                ],
            )?;
        }
    }
    delete_missing(tx, "review_thread", pr_id, &keep_threads)?;
    let existing: Vec<String> = {
        let mut st = tx.prepare(
            "SELECT c.node_id FROM review_comment c JOIN review_thread t ON t.id = c.thread_id WHERE t.pr_id = ?1",
        )?;
        st.query_map([pr_id], |r| r.get(0))?.collect::<Result<_, _>>()?
    };
    for id in existing.iter().filter(|id| !keep_comments.contains(id.as_str())) {
        tx.execute("DELETE FROM review_comment WHERE node_id = ?1", [id])?;
    }

    let mut keep: HashSet<&str> = HashSet::new();
    for c in issue_comments {
        keep.insert(&c.id);
        tx.execute(
            "INSERT INTO issue_comment (node_id, pr_id, author_login, body_md, body_html, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (node_id) DO UPDATE SET body_md = excluded.body_md, body_html = excluded.body_html",
            params![
                c.id,
                pr_id,
                c.author.as_ref().map(|a| &a.login),
                c.body.replace('\r', ""),
                localize(&c.body_html),
                c.created_at
            ],
        )?;
    }
    delete_missing(tx, "issue_comment", pr_id, &keep)?;
    Ok(())
}

/// Deletes mirror rows GitHub no longer returns (a deleted comment, say).
fn delete_missing(tx: &Transaction, table: &str, pr_id: i64, keep: &HashSet<&str>) -> Result<()> {
    let ids: Vec<String> = {
        let mut st = tx.prepare(&format!("SELECT node_id FROM {table} WHERE pr_id = ?1"))?;
        st.query_map([pr_id], |r| r.get(0))?.collect::<Result<_, _>>()?
    };
    for id in ids.iter().filter(|id| !keep.contains(id.as_str())) {
        tx.execute(&format!("DELETE FROM {table} WHERE node_id = ?1"), [id])?;
    }
    Ok(())
}

// ─── Fetching ────────────────────────────────────────────────────────────

fn is_image_path(path: &str) -> bool {
    let lower = path.to_lowercase();
    [".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".ico"].iter().any(|e| lower.ends_with(e))
}

struct BlobNeed {
    /// Index into the files; `None` for the root `.gitattributes`.
    file: Option<usize>,
    is_head: bool,
    expr: String,
    path: String,
}

async fn fetch_files(
    ctx: &SyncCtx,
    r: &PrRef,
    head: &str,
    merge_base: &str,
    pr_id: Option<i64>,
) -> Result<FilesResult> {
    let gh = &ctx.gh;
    let rest: Vec<RestPrFile> = gh
        .rest_get_pages(&format!("repos/{}/{}/pulls/{}/files", r.owner, r.name, r.number), 100, 30, true)
        .await?;

    // Base blob ids we already know for this merge base, from earlier revisions.
    let known_base: HashMap<String, String> = match pr_id {
        Some(id) => ctx.db.read(|c| {
            let mut st = c.prepare(
                "SELECT COALESCE(f.prev_path, f.path), f.base_blob_oid FROM revision_file f
                 JOIN pr_revision v ON v.id = f.revision_id
                 WHERE v.pr_id = ?1 AND v.merge_base_oid = ?2 AND f.base_blob_oid IS NOT NULL",
            )?;
            let rows = st.query_map(params![id, merge_base], |r| Ok((r.get(0)?, r.get(1)?)))?;
            Ok(rows.collect::<Result<_, _>>()?)
        })?,
        None => HashMap::new(),
    };

    let mut files = Vec::with_capacity(rest.len());
    // The root .gitattributes rides along in the first batch, for finding
    // generated files. (`object(expression:)` is null for a missing file,
    // where `Commit.file(path:)` would add a NOT_FOUND error.)
    let mut needs = vec![BlobNeed {
        file: None,
        is_head: true,
        expr: format!("{head}:.gitattributes"),
        path: ".gitattributes".into(),
    }];
    let mut gitattributes = None;
    for (i, f) in rest.iter().enumerate() {
        let base_path = f.previous_filename.clone().unwrap_or_else(|| f.filename.clone());
        let has_base = f.status != "added";
        let has_head = f.status != "removed";
        let mut row = FileRow {
            path: f.filename.clone(),
            prev_path: f.previous_filename.clone(),
            change_type: f.status.clone(),
            additions: f.additions,
            deletions: f.deletions,
            base_blob_oid: None,
            head_blob_oid: None,
            patch: f.patch.clone(),
            patch_status: "ok",
            content_status: "ok",
        };
        if has_head {
            match &f.sha {
                Some(sha) if ctx.blobs.has(sha)? => row.head_blob_oid = Some(sha.clone()),
                _ => needs.push(BlobNeed {
                    file: Some(i),
                    is_head: true,
                    expr: format!("{head}:{}", f.filename),
                    path: f.filename.clone(),
                }),
            }
        }
        if has_base {
            match known_base.get(&base_path) {
                Some(oid) if ctx.blobs.has(oid)? => row.base_blob_oid = Some(oid.clone()),
                _ => needs.push(BlobNeed {
                    file: Some(i),
                    is_head: false,
                    expr: format!("{merge_base}:{base_path}"),
                    path: base_path,
                }),
            }
        }
        files.push(row);
    }

    let mut partial = Vec::new();
    let mut binary: HashSet<usize> = HashSet::new();
    // Batches are fetched a few at a time but handled in order, so at most a
    // few responses are held in memory.
    // (The futures are built up front: a closure inside the stream trips
    // rustc's `Send` inference for the spawned sync task.)
    let fetches: Vec<_> = needs.chunks(BLOB_BATCH).map(|chunk| fetch_blob_batch(gh, r, chunk)).collect();
    let mut batches = stream::iter(fetches).buffered(BLOB_BATCHES_IN_FLIGHT);
    let mut chunks = needs.chunks(BLOB_BATCH);
    while let Some(data) = batches.next().await {
        let data = data?;
        let chunk = chunks.next().expect("one response per chunk");
        for (i, need) in chunk.iter().enumerate() {
            let info: Option<BlobInfo> = serde_json::from_value(data["repository"][format!("b{i}")].clone())
                .map_err(|e| GhError::Protocol(format!("Blobs: {e}")))?;
            let Some(file) = need.file else {
                gitattributes = info.filter(|b| !b.is_truncated).and_then(|b| b.text);
                continue;
            };
            let row = &mut files[file];
            let Some(info) = info else {
                row.content_status = "missing";
                partial.push(PartialReason {
                    path: row.path.clone(),
                    reason: "GitHub couldn't find the file contents".into(),
                });
                continue;
            };
            if need.is_head && rest[file].sha.as_deref().is_some_and(|s| s != info.oid) {
                // The files list is from a different head than `head`.
                return Ok(FilesResult::HeadMoved);
            }
            let is_binary = info.is_binary.unwrap_or(false);
            if is_binary {
                binary.insert(file);
            }
            if need.is_head {
                row.head_blob_oid = Some(info.oid.clone());
            } else {
                row.base_blob_oid = Some(info.oid.clone());
            }
            if info.byte_size > MAX_BLOB {
                row.content_status = "too_large";
                partial.push(PartialReason {
                    path: row.path.clone(),
                    reason: "file is too large to keep offline".into(),
                });
                continue;
            }
            if is_binary && !(is_image_path(&need.path) && info.byte_size <= MAX_IMAGE_BLOB) {
                row.content_status = "binary_skipped";
                continue;
            }
            let stored = match (&info.text, info.is_truncated || is_binary) {
                (Some(text), false) => ctx.blobs.put(&info.oid, text.as_bytes())?,
                _ => false,
            };
            if !stored {
                // Truncated, binary, or the text didn't hash back to the blob
                // (non-UTF-8 or line endings): fetch the raw bytes.
                let raw = gh
                    .rest_get_bytes(
                        &format!("repos/{}/{}/git/blobs/{}", r.owner, r.name, info.oid),
                        "application/vnd.github.raw+json",
                    )
                    .await?;
                if !ctx.blobs.put(&info.oid, &raw)? {
                    row.content_status = "missing";
                    partial.push(PartialReason {
                        path: row.path.clone(),
                        reason: "file contents failed verification".into(),
                    });
                }
            }
        }
    }

    for (i, row) in files.iter_mut().enumerate() {
        if row.patch.is_none() {
            let f = &rest[i];
            let stored_binary = |oid: &Option<String>| -> Result<bool> {
                Ok(match oid {
                    Some(o) => ctx.blobs.is_binary(o)?.unwrap_or(false),
                    None => false,
                })
            };
            let is_binary = binary.contains(&i)
                || stored_binary(&row.base_blob_oid)?
                || stored_binary(&row.head_blob_oid)?;
            // Binary files report no changes, so check for them first.
            row.patch_status = if is_binary {
                "binary"
            } else if f.changes == 0 {
                "ok"
            } else {
                partial.push(PartialReason {
                    path: row.path.clone(),
                    reason: "GitHub sent no diff for this file (too large); line comments are off".into(),
                });
                "too_large"
            };
        }
    }
    Ok(FilesResult::Done(files, partial, gitattributes))
}

async fn fetch_blob_batch(gh: &GitHub, r: &PrRef, chunk: &[BlobNeed]) -> Result<serde_json::Value> {
    let mut vars = serde_json::Map::new();
    vars.insert("owner".into(), json!(r.owner));
    vars.insert("name".into(), json!(r.name));
    for (i, n) in chunk.iter().enumerate() {
        vars.insert(format!("e{i}"), json!(n.expr));
    }
    Ok(gh.graphql("Blobs", &blobs_query(chunk.len()), serde_json::Value::Object(vars), OpKind::Query).await?)
}

async fn fetch_commits(gh: &GitHub, id: &str) -> Result<Vec<CommitInfo>> {
    let mut out = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let d: NodeData<PrCommitsNode> =
            gh.graphql("PrCommits", PR_COMMITS, json!({ "id": id, "after": after }), OpKind::Query).await?;
        let conn = d.node.ok_or(GhError::NotFound)?.commits;
        out.extend(conn.nodes.into_iter().map(|n| n.commit));
        if !conn.page_info.has_next_page {
            return Ok(out);
        }
        after = conn.page_info.end_cursor;
    }
}

async fn fetch_reviews(gh: &GitHub, id: &str) -> Result<Vec<ReviewInfo>> {
    let mut out = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let d: NodeData<PrReviewsNode> =
            gh.graphql("PrReviews", PR_REVIEWS, json!({ "id": id, "after": after }), OpKind::Query).await?;
        let conn = d.node.ok_or(GhError::NotFound)?.reviews;
        out.extend(conn.nodes);
        if !conn.page_info.has_next_page {
            return Ok(out);
        }
        after = conn.page_info.end_cursor;
    }
}

async fn fetch_threads(gh: &GitHub, id: &str) -> Result<Vec<ThreadInfo>> {
    let mut out: Vec<ThreadInfo> = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let d: NodeData<PrThreadsNode> =
            gh.graphql("PrThreads", PR_THREADS, json!({ "id": id, "after": after }), OpKind::Query).await?;
        let conn = d.node.ok_or(GhError::NotFound)?.review_threads;
        out.extend(conn.nodes);
        if !conn.page_info.has_next_page {
            break;
        }
        after = conn.page_info.end_cursor;
    }
    // Threads with more comments than the first page.
    for t in out.iter_mut() {
        while t.comments.page_info.has_next_page {
            let d: NodeData<ThreadCommentsNode> = gh
                .graphql(
                    "ThreadComments",
                    THREAD_COMMENTS,
                    json!({ "id": t.id, "after": t.comments.page_info.end_cursor }),
                    OpKind::Query,
                )
                .await?;
            let conn = d.node.ok_or(GhError::NotFound)?.comments;
            t.comments.nodes.extend(conn.nodes);
            t.comments.page_info = conn.page_info;
        }
    }
    Ok(out)
}

async fn fetch_issue_comments(gh: &GitHub, id: &str) -> Result<Vec<IssueCommentInfo>> {
    let mut out = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let d: NodeData<PrIssueCommentsNode> = gh
            .graphql("PrIssueComments", PR_ISSUE_COMMENTS, json!({ "id": id, "after": after }), OpKind::Query)
            .await?;
        let conn = d.node.ok_or(GhError::NotFound)?.comments;
        out.extend(conn.nodes);
        if !conn.page_info.has_next_page {
            return Ok(out);
        }
        after = conn.page_info.end_cursor;
    }
}

// ─── Images ──────────────────────────────────────────────────────────────

fn collect_images(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let _ = lol_html::rewrite_str(
        html,
        lol_html::RewriteStrSettings {
            element_content_handlers: vec![lol_html::element!("img[src]", |el| {
                if let Some(src) = el.get_attribute("src") {
                    out.push(src);
                }
                Ok(())
            })],
            ..lol_html::RewriteStrSettings::new()
        },
    );
    out
}

/// Stable key for an image URL. Images in private repos use signed URLs that
/// change on every fetch; only their path identifies the image.
fn image_key(url: &str) -> String {
    if url.starts_with("https://private-user-images.githubusercontent.com/") {
        url.split('?').next().unwrap_or(url).to_string()
    } else {
        url.to_string()
    }
}

/// Downloads every image referenced by `html` right away (signed URLs expire
/// within minutes). Returns image URL → sha256 of the stored file. Failures
/// are skipped: the image just won't show offline.
async fn download_images(ctx: &SyncCtx, html: &[&str]) -> HashMap<String, String> {
    let mut urls: Vec<String> = html.iter().flat_map(|h| collect_images(h)).collect();
    urls.sort();
    urls.dedup();
    let mut out = HashMap::new();
    let mut budget = MAX_ASSETS_PER_PR;
    for url in urls {
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            continue;
        }
        let key = image_key(&url);
        let known: Option<String> = ctx
            .db
            .read(|c| {
                Ok(c.query_row("SELECT sha256 FROM asset WHERE source_url = ?1", [&key], |r| r.get(0))
                    .optional()?)
            })
            .ok()
            .flatten();
        if let Some(sha) = known
            && ctx.assets_dir.join(&sha).exists()
        {
            out.insert(url, sha);
            continue;
        }
        match ctx.gh.download(&url, MAX_ASSET.min(budget)).await {
            Ok((bytes, content_type)) => {
                budget = budget.saturating_sub(bytes.len());
                let sha = hex::encode(Sha256::digest(&bytes));
                if std::fs::create_dir_all(&ctx.assets_dir).is_err()
                    || std::fs::write(ctx.assets_dir.join(&sha), &bytes).is_err()
                {
                    continue;
                }
                let now = rfc3339(ctx.clock.now());
                let res = ctx.db.write(|tx| {
                    tx.execute(
                        "INSERT INTO asset (sha256, source_url, content_type, byte_size, fetched_at)
                         VALUES (?1, ?2, ?3, ?4, ?5)
                         ON CONFLICT (sha256) DO UPDATE SET source_url = excluded.source_url",
                        params![sha, key, content_type, bytes.len() as i64, now],
                    )?;
                    Ok(())
                });
                if res.is_ok() {
                    out.insert(url, sha);
                }
            }
            Err(e) => {
                tracing::warn!("couldn't download image {}: {e}", crate::github::client::redact_url(&url))
            }
        }
    }
    out
}

fn rewrite_images(html: &str, assets: &HashMap<String, String>) -> String {
    if assets.is_empty() {
        return html.to_string();
    }
    lol_html::rewrite_str(
        html,
        lol_html::RewriteStrSettings {
            element_content_handlers: vec![lol_html::element!("img[src]", |el| {
                if let Some(sha) = el.get_attribute("src").and_then(|s| assets.get(&s)) {
                    el.set_attribute("src", &format!("{ASSET_URL_PREFIX}{sha}"))?;
                    el.remove_attribute("srcset");
                }
                Ok(())
            })],
            ..lol_html::RewriteStrSettings::new()
        },
    )
    .unwrap_or_else(|_| html.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pr_references() {
        let want = PrRef { owner: "o".into(), name: "r".into(), number: 5 };
        for s in [
            "https://github.com/o/r/pull/5",
            "https://github.com/o/r/pull/5/files",
            "github.com/o/r/pull/5?w=1",
            "o/r#5",
            "o/r/pull/5",
            "  https://www.github.com/o/r/pull/5#discussion_r1  ",
        ] {
            assert_eq!(PrRef::parse(s).as_ref(), Some(&want), "{s}");
        }
        assert_eq!(PrRef::parse("https://github.com/o/r/issues/5"), None);
        assert_eq!(PrRef::parse("o/r"), None);
    }

    #[test]
    fn rewrites_images_to_local_assets() {
        let html = r#"<p><a href="x"><img src="https://camo/a.png" srcset="y 2x" alt="a"></a><img src="https://other"></p>"#;
        assert_eq!(collect_images(html), ["https://camo/a.png", "https://other"]);
        let map = HashMap::from([("https://camo/a.png".to_string(), "abc".to_string())]);
        let out = rewrite_images(html, &map);
        assert!(out.contains(r#"src="prtg://localhost/asset/abc""#));
        assert!(!out.contains("srcset"));
        assert!(out.contains(r#"src="https://other""#));
    }

    #[test]
    fn signed_image_urls_share_a_key() {
        let a = image_key("https://private-user-images.githubusercontent.com/1/2-x.png?jwt=aaa");
        let b = image_key("https://private-user-images.githubusercontent.com/1/2-x.png?jwt=bbb");
        assert_eq!(a, b);
        assert_ne!(
            image_key("https://camo.githubusercontent.com/a?x=1"),
            image_key("https://camo.githubusercontent.com/a?x=2")
        );
    }
}

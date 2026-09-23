//! Read models for the UI. Everything here reads local state only, so it works
//! offline.

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::blobstore::BlobStore;
use crate::db::Db;
use crate::diff::{Hunk, local_patch, parse_patch};
use crate::error::{Error, Result};

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PrSummary {
    pub id: i64,
    pub repo: String,
    pub number: i64,
    pub title: String,
    pub author: Option<String>,
    pub state: String,
    pub is_draft: bool,
    pub updated_at: String,
    pub sync_state: String,
    pub sync_error: Option<String>,
    pub last_synced_at: Option<String>,
    pub additions: i64,
    pub deletions: i64,
    pub changed_files: i64,
    pub review_decision: Option<String>,
    pub in_inbox: bool,
    pub pinned: bool,
    /// Status of the active draft review, if there is one.
    pub draft_status: Option<String>,
    /// The PR moved since the revision the user last viewed.
    pub updated_since_viewed: bool,
}

const SUMMARY_SELECT: &str = "
    SELECT p.id, r.owner || '/' || r.name, p.number, p.title, p.author_login, p.state, p.is_draft,
           p.updated_at, p.sync_state, p.sync_error, p.last_synced_at, p.additions, p.deletions,
           p.changed_files, p.review_decision, p.in_inbox, COALESCE(l.pinned, 0),
           (SELECT d.status FROM draft_review d WHERE d.pr_id = p.id
              AND d.status NOT IN ('submitted', 'discarded')),
           (l.last_viewed_revision_id IS NOT NULL AND l.last_viewed_revision_id != p.current_revision_id)
    FROM pull_request p JOIN repo r ON r.id = p.repo_id
    LEFT JOIN pr_local l ON l.pr_id = p.id";

fn summary_row(r: &rusqlite::Row) -> rusqlite::Result<PrSummary> {
    Ok(PrSummary {
        id: r.get(0)?,
        repo: r.get(1)?,
        number: r.get(2)?,
        title: r.get(3)?,
        author: r.get(4)?,
        state: r.get(5)?,
        is_draft: r.get(6)?,
        updated_at: r.get(7)?,
        sync_state: r.get(8)?,
        sync_error: r.get(9)?,
        last_synced_at: r.get(10)?,
        additions: r.get(11)?,
        deletions: r.get(12)?,
        changed_files: r.get(13)?,
        review_decision: r.get(14)?,
        in_inbox: r.get(15)?,
        pinned: r.get(16)?,
        draft_status: r.get(17)?,
        updated_since_viewed: r.get::<_, Option<bool>>(18)?.unwrap_or(false),
    })
}

/// PRs to show in the inbox: ones in a subscription's results, pinned ones,
/// and any with a draft or queued review.
pub fn list_prs(db: &Db) -> Result<Vec<PrSummary>> {
    db.read(|c| {
        let sql = format!(
            "{SUMMARY_SELECT}
             WHERE p.current_revision_id IS NOT NULL OR p.in_inbox = 1 OR COALESCE(l.pinned, 0) = 1
             ORDER BY p.updated_at DESC"
        );
        let mut st = c.prepare(&sql)?;
        let rows = st.query_map([], summary_row)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })
}

pub fn pr_summary(c: &Connection, pr_id: i64) -> Result<PrSummary> {
    c.query_row(&format!("{SUMMARY_SELECT} WHERE p.id = ?1"), [pr_id], summary_row)
        .optional()?
        .ok_or_else(|| Error::NotFound(format!("pull request {pr_id}")))
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RevisionInfo {
    pub id: i64,
    pub head_oid: String,
    pub base_oid: String,
    pub merge_base_oid: String,
    pub fetched_at: String,
    pub status: String,
    pub partial_reasons: Vec<PartialReasonView>,
    pub is_force_push: Option<bool>,
}

#[derive(Serialize, serde::Deserialize, Clone, Debug)]
pub struct PartialReasonView {
    pub path: String,
    pub reason: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub path: String,
    pub prev_path: Option<String>,
    pub change_type: String,
    pub additions: i64,
    pub deletions: i64,
    pub patch_status: String,
    pub content_status: String,
    pub head_blob_oid: Option<String>,
    pub viewed: bool,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CommitEntry {
    pub oid: String,
    pub headline: String,
    pub body: String,
    pub author: Option<String>,
    pub authored_at: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CheckSnapshot {
    pub captured_at: String,
    pub rollup_state: Option<String>,
    pub contexts: serde_json::Value,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ReviewEntry {
    pub node_id: String,
    pub author: Option<String>,
    pub state: String,
    pub body_html: String,
    pub submitted_at: Option<String>,
    pub commit_oid: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CommentEntry {
    pub node_id: String,
    pub author: Option<String>,
    pub body_md: String,
    pub body_html: String,
    pub created_at: String,
    pub state: Option<String>,
    pub diff_hunk: Option<String>,
    pub original_commit_oid: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ThreadEntry {
    pub node_id: String,
    pub path: String,
    pub subject_type: String,
    pub diff_side: Option<String>,
    pub line: Option<i64>,
    pub start_line: Option<i64>,
    pub start_diff_side: Option<String>,
    pub original_line: Option<i64>,
    pub is_outdated: bool,
    pub is_resolved: bool,
    pub viewer_can_reply: bool,
    pub comments: Vec<CommentEntry>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct IssueCommentEntry {
    pub node_id: String,
    pub author: Option<String>,
    pub body_html: String,
    pub created_at: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PrDetail {
    #[serde(flatten)]
    pub summary: PrSummary,
    pub url: String,
    pub body_html: String,
    pub base_ref: String,
    pub head_ref: String,
    pub locked: bool,
    pub viewer_did_author: bool,
    pub repo_archived: bool,
    pub revision: Option<RevisionInfo>,
    pub files: Vec<FileEntry>,
    pub commits: Vec<CommitEntry>,
    pub checks: Option<CheckSnapshot>,
    pub reviews: Vec<ReviewEntry>,
    pub threads: Vec<ThreadEntry>,
    pub issue_comments: Vec<IssueCommentEntry>,
    /// The active draft review, if any.
    pub draft: Option<crate::drafts::DraftView>,
    /// The most recently sent review from this app, if any.
    pub last_review: Option<crate::drafts::DraftView>,
}

pub fn revision_info(c: &Connection, id: i64) -> Result<RevisionInfo> {
    Ok(c.query_row(
        "SELECT id, head_oid, base_oid, merge_base_oid, fetched_at, status, partial_reasons, is_force_push
         FROM pr_revision WHERE id = ?1",
        [id],
        |r| {
            let reasons: Option<String> = r.get(6)?;
            Ok(RevisionInfo {
                id: r.get(0)?,
                head_oid: r.get(1)?,
                base_oid: r.get(2)?,
                merge_base_oid: r.get(3)?,
                fetched_at: r.get(4)?,
                status: r.get(5)?,
                partial_reasons: reasons.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default(),
                is_force_push: r.get(7)?,
            })
        },
    )?)
}

pub fn get_pr(db: &Db, pr_id: i64) -> Result<PrDetail> {
    db.read(|c| {
        let summary = pr_summary(c, pr_id)?;
        type Head = (String, String, String, String, bool, bool, bool, Option<i64>);
        let (url, body_html, base_ref, head_ref, locked, viewer_did_author, archived, rev_id): Head = c.query_row(
            "SELECT p.url, p.body_html, p.base_ref_name, p.head_ref_name, p.locked, p.viewer_did_author,
                    r.is_archived, p.current_revision_id
             FROM pull_request p JOIN repo r ON r.id = p.repo_id WHERE p.id = ?1",
            [pr_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?)),
        )?;
        let revision = rev_id.map(|id| revision_info(c, id)).transpose()?;
        let files = match rev_id {
            Some(id) => revision_files(c, pr_id, id)?,
            None => vec![],
        };
        let commits = match rev_id {
            Some(id) => {
                let mut st = c.prepare(
                    "SELECT oid, message_headline, message_body, COALESCE(author_login, author_name), authored_at
                     FROM pr_commit WHERE revision_id = ?1 ORDER BY position",
                )?;
                st.query_map([id], |r| {
                    Ok(CommitEntry { oid: r.get(0)?, headline: r.get(1)?, body: r.get(2)?, author: r.get(3)?, authored_at: r.get(4)? })
                })?
                .collect::<Result<Vec<_>, _>>()?
            }
            None => vec![],
        };
        let checks = match rev_id {
            Some(id) => c
                .query_row(
                    "SELECT captured_at, rollup_state, contexts FROM check_snapshot WHERE revision_id = ?1",
                    [id],
                    |r| {
                        let ctx: String = r.get(2)?;
                        Ok(CheckSnapshot {
                            captured_at: r.get(0)?,
                            rollup_state: r.get(1)?,
                            contexts: serde_json::from_str(&ctx).unwrap_or(serde_json::Value::Null),
                        })
                    },
                )
                .optional()?,
            None => None,
        };
        let reviews = {
            let mut st = c.prepare(
                "SELECT node_id, author_login, state, body_html, submitted_at, commit_oid FROM review
                 WHERE pr_id = ?1 ORDER BY COALESCE(submitted_at, '9999')",
            )?;
            st.query_map([pr_id], |r| {
                Ok(ReviewEntry {
                    node_id: r.get(0)?,
                    author: r.get(1)?,
                    state: r.get(2)?,
                    body_html: r.get(3)?,
                    submitted_at: r.get(4)?,
                    commit_oid: r.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?
        };
        let threads = threads(c, pr_id)?;
        let issue_comments = {
            let mut st = c.prepare(
                "SELECT node_id, author_login, body_html, created_at FROM issue_comment WHERE pr_id = ?1
                 ORDER BY created_at",
            )?;
            st.query_map([pr_id], |r| {
                Ok(IssueCommentEntry { node_id: r.get(0)?, author: r.get(1)?, body_html: r.get(2)?, created_at: r.get(3)? })
            })?
            .collect::<Result<Vec<_>, _>>()?
        };
        Ok(PrDetail {
            summary,
            url,
            body_html,
            base_ref,
            head_ref,
            locked,
            viewer_did_author,
            repo_archived: archived,
            revision,
            files,
            commits,
            checks,
            reviews,
            threads,
            issue_comments,
            draft: match crate::drafts::active_draft_id(c, pr_id)? {
                Some(id) => Some(crate::drafts::draft_view(c, id)?),
                None => None,
            },
            last_review: match c
                .query_row(
                    "SELECT id FROM draft_review WHERE pr_id = ?1 AND status = 'submitted'
                     ORDER BY submitted_at DESC, id DESC LIMIT 1",
                    [pr_id],
                    |r| r.get::<_, i64>(0),
                )
                .optional()?
            {
                Some(id) => Some(crate::drafts::draft_view(c, id)?),
                None => None,
            },
        })
    })
}

pub fn revision_files(c: &Connection, pr_id: i64, rev_id: i64) -> Result<Vec<FileEntry>> {
    let mut st = c.prepare(
        "SELECT f.path, f.prev_path, f.change_type, f.additions, f.deletions, f.patch_status, f.content_status,
                f.head_blob_oid, (v.pr_id IS NOT NULL AND v.head_blob_oid IS f.head_blob_oid)
         FROM revision_file f
         LEFT JOIN file_viewed v ON v.pr_id = ?1 AND v.path = f.path
         WHERE f.revision_id = ?2 ORDER BY f.path",
    )?;
    let rows = st
        .query_map(params![pr_id, rev_id], |r| {
            Ok(FileEntry {
                path: r.get(0)?,
                prev_path: r.get(1)?,
                change_type: r.get(2)?,
                additions: r.get(3)?,
                deletions: r.get(4)?,
                patch_status: r.get(5)?,
                content_status: r.get(6)?,
                head_blob_oid: r.get(7)?,
                viewed: r.get::<_, Option<bool>>(8)?.unwrap_or(false),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn threads(c: &Connection, pr_id: i64) -> Result<Vec<ThreadEntry>> {
    let mut st = c.prepare(
        "SELECT id, node_id, path, subject_type, diff_side, line, start_line, start_diff_side, original_line,
                is_outdated, is_resolved, viewer_can_reply
         FROM review_thread WHERE pr_id = ?1 ORDER BY path, COALESCE(line, original_line)",
    )?;
    let mut threads: Vec<(i64, ThreadEntry)> = st
        .query_map([pr_id], |r| {
            Ok((
                r.get(0)?,
                ThreadEntry {
                    node_id: r.get(1)?,
                    path: r.get(2)?,
                    subject_type: r.get(3)?,
                    diff_side: r.get(4)?,
                    line: r.get(5)?,
                    start_line: r.get(6)?,
                    start_diff_side: r.get(7)?,
                    original_line: r.get(8)?,
                    is_outdated: r.get(9)?,
                    is_resolved: r.get(10)?,
                    viewer_can_reply: r.get(11)?,
                    comments: vec![],
                },
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut cst = c.prepare(
        "SELECT node_id, author_login, body_md, body_html, created_at, state, diff_hunk, original_commit_oid
         FROM review_comment WHERE thread_id = ?1 ORDER BY position",
    )?;
    for (id, t) in threads.iter_mut() {
        t.comments = cst
            .query_map([*id], |r| {
                Ok(CommentEntry {
                    node_id: r.get(0)?,
                    author: r.get(1)?,
                    body_md: r.get(2)?,
                    body_html: r.get(3)?,
                    created_at: r.get(4)?,
                    state: r.get(5)?,
                    diff_hunk: r.get(6)?,
                    original_commit_oid: r.get(7)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
    }
    Ok(threads.into_iter().map(|(_, t)| t).collect())
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FileDiff {
    pub path: String,
    pub prev_path: Option<String>,
    pub change_type: String,
    pub patch_status: String,
    pub content_status: String,
    pub hunks: Vec<Hunk>,
    /// `github` (GitHub's patch), `local` (diffed here, view only) or `none`.
    pub source: &'static str,
    /// Line comments are possible (the hunks are GitHub's).
    pub commentable: bool,
    pub base_text: Option<String>,
    pub head_text: Option<String>,
    pub base_blob_oid: Option<String>,
    pub head_blob_oid: Option<String>,
    pub base_binary: bool,
    pub head_binary: bool,
}

pub fn file_diff(db: &Db, blobs: &BlobStore, revision_id: i64, path: &str) -> Result<FileDiff> {
    struct Row {
        prev_path: Option<String>,
        change_type: String,
        base_oid: Option<String>,
        head_oid: Option<String>,
        patch: Option<String>,
        patch_status: String,
        content_status: String,
    }
    let Row { prev_path, change_type, base_oid, head_oid, patch, patch_status, content_status } =
        db.read(|c| {
            c.query_row(
            "SELECT prev_path, change_type, base_blob_oid, head_blob_oid, patch, patch_status, content_status
             FROM revision_file WHERE revision_id = ?1 AND path = ?2",
            params![revision_id, path],
            |r| {
                Ok(Row {
                    prev_path: r.get(0)?,
                    change_type: r.get(1)?,
                    base_oid: r.get(2)?,
                    head_oid: r.get(3)?,
                    patch: r.get(4)?,
                    patch_status: r.get(5)?,
                    content_status: r.get(6)?,
                })
            },
        )
        .optional()?
        .ok_or_else(|| Error::NotFound(format!("{path} in revision {revision_id}")))
        })?;
    let base_bytes = base_oid.as_deref().map(|o| blobs.get(o)).transpose()?.flatten();
    let head_bytes = head_oid.as_deref().map(|o| blobs.get(o)).transpose()?.flatten();
    let is_bin = |b: &Option<Vec<u8>>| b.as_deref().is_some_and(crate::blobstore::looks_binary);
    let (base_binary, head_binary) = (is_bin(&base_bytes), is_bin(&head_bytes));
    let text = |b: Option<Vec<u8>>, binary: bool| {
        if binary { None } else { b.map(|b| String::from_utf8_lossy(&b).into_owned()) }
    };
    let base_text = text(base_bytes, base_binary);
    let head_text = text(head_bytes, head_binary);

    let (hunks, source, commentable) = match &patch {
        Some(p) => (parse_patch(p).map_err(Error::Internal)?, "github", true),
        None if !base_binary && !head_binary && (base_text.is_some() || head_text.is_some()) => {
            let old = base_text.as_deref().unwrap_or("");
            let new = head_text.as_deref().unwrap_or("");
            let hunks = parse_patch(&local_patch(old, new)).map_err(Error::Internal)?;
            if hunks.is_empty() { (hunks, "none", false) } else { (hunks, "local", false) }
        }
        None => (vec![], "none", false),
    };
    Ok(FileDiff {
        path: path.to_string(),
        prev_path,
        change_type,
        patch_status,
        content_status,
        hunks,
        source,
        commentable,
        base_text,
        head_text,
        base_blob_oid: base_oid,
        head_blob_oid: head_oid,
        base_binary,
        head_binary,
    })
}

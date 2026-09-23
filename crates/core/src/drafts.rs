//! Draft reviews: everything the user writes before it's sent. These are user
//! tables (DESIGN.md §7.1): sync never touches them, and each comment carries
//! its own anchor so it can be remapped if the PR moves.

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use crate::blobstore::BlobStore;
use crate::db::Db;
use crate::diff::{Hunk, LineKind, Side, check_range, parse_patch, split_lines};
use crate::error::{Error, Result};
use crate::service::Core;

#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CommentKind {
    Thread,
    Reply,
}

#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum SubjectType {
    Line,
    File,
}

#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Verdict {
    Comment,
    Approve,
    RequestChanges,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Comment => "COMMENT",
            Verdict::Approve => "APPROVE",
            Verdict::RequestChanges => "REQUEST_CHANGES",
        }
    }

    fn parse(s: &str) -> Option<Verdict> {
        match s {
            "COMMENT" => Some(Verdict::Comment),
            "APPROVE" => Some(Verdict::Approve),
            "REQUEST_CHANGES" => Some(Verdict::RequestChanges),
            _ => None,
        }
    }
}

/// A new comment, as the UI sends it.
#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct NewComment {
    /// The revision the user was looking at.
    pub revision_id: i64,
    pub kind: CommentKind,
    pub subject_type: SubjectType,
    pub path: Option<String>,
    pub side: Option<Side>,
    pub line: Option<u32>,
    pub start_side: Option<Side>,
    pub start_line: Option<u32>,
    pub reply_to_thread: Option<String>,
    pub body: String,
}

/// The text around one side of an anchor, kept with the comment so it can be
/// found again after the PR changes.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SideSnapshot {
    pub first: u32,
    pub last: u32,
    pub lines: Vec<String>,
    pub before: Vec<String>,
    pub after: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AnchorSnapshot {
    pub hunk_header: String,
    pub left: Option<SideSnapshot>,
    pub right: Option<SideSnapshot>,
    pub base_blob_oid: Option<String>,
    pub head_blob_oid: Option<String>,
}

const CONTEXT: usize = 3;

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DraftCommentView {
    pub id: i64,
    pub kind: CommentKind,
    pub subject_type: SubjectType,
    pub path: Option<String>,
    pub side: Option<Side>,
    pub line: Option<u32>,
    pub start_side: Option<Side>,
    pub start_line: Option<u32>,
    pub reply_to_thread: Option<String>,
    pub body_md: String,
    pub anchor_revision_id: Option<i64>,
    pub anchor: Option<AnchorSnapshot>,
    pub remap_status: String,
    pub remap_proposal: Option<serde_json::Value>,
    pub resolution: Option<String>,
    pub staged: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DraftView {
    pub id: i64,
    pub pr_id: i64,
    pub status: String,
    pub body_md: String,
    pub verdict: Option<Verdict>,
    pub basis_revision_id: i64,
    pub target_mode: String,
    pub target_revision_id: Option<i64>,
    pub attention: Option<serde_json::Value>,
    pub last_error: Option<String>,
    pub last_error_kind: Option<String>,
    pub next_attempt_at: Option<String>,
    pub queued_at: Option<String>,
    pub submitted_at: Option<String>,
    pub submitted_url: Option<String>,
    pub comments: Vec<DraftCommentView>,
}

pub const ACTIVE: &str = "status NOT IN ('submitted', 'discarded')";

pub fn active_draft_id(c: &Connection, pr_id: i64) -> Result<Option<i64>> {
    Ok(c.query_row(&format!("SELECT id FROM draft_review WHERE pr_id = ?1 AND {ACTIVE}"), [pr_id], |r| {
        r.get(0)
    })
    .optional()?)
}

pub fn draft_view(c: &Connection, id: i64) -> Result<DraftView> {
    let mut v = c
        .query_row(
            "SELECT id, pr_id, status, body_md, verdict, basis_revision_id, target_mode, target_revision_id, attention,
                    last_error, last_error_kind, next_attempt_at, queued_at, submitted_at, submitted_url
             FROM draft_review WHERE id = ?1",
            [id],
            |r| {
                let verdict: Option<String> = r.get(4)?;
                let attention: Option<String> = r.get(8)?;
                Ok(DraftView {
                    id: r.get(0)?,
                    pr_id: r.get(1)?,
                    status: r.get(2)?,
                    body_md: r.get(3)?,
                    verdict: verdict.as_deref().and_then(Verdict::parse),
                    basis_revision_id: r.get(5)?,
                    target_mode: r.get(6)?,
                    target_revision_id: r.get(7)?,
                    attention: attention.and_then(|a| serde_json::from_str(&a).ok()),
                    last_error: r.get(9)?,
                    last_error_kind: r.get(10)?,
                    next_attempt_at: r.get(11)?,
                    queued_at: r.get(12)?,
                    submitted_at: r.get(13)?,
                    submitted_url: r.get(14)?,
                    comments: vec![],
                })
            },
        )
        .optional()?
        .ok_or_else(|| Error::NotFound(format!("draft review {id}")))?;
    v.comments = draft_comments(c, id)?;
    Ok(v)
}

pub fn draft_comments(c: &Connection, review_id: i64) -> Result<Vec<DraftCommentView>> {
    let mut st = c.prepare(
        "SELECT id, kind, subject_type, path, side, line, start_side, start_line, reply_to_thread_node_id, body_md,
                anchor_revision_id, anchor_snapshot, remap_status, remap_proposal, resolution,
                staged_node_id IS NOT NULL, created_at, updated_at
         FROM draft_comment WHERE draft_review_id = ?1 ORDER BY path, line, id",
    )?;
    let rows = st
        .query_map([review_id], |r| {
            let kind: String = r.get(1)?;
            let subject: String = r.get(2)?;
            let side: Option<String> = r.get(4)?;
            let start_side: Option<String> = r.get(6)?;
            let anchor: Option<String> = r.get(11)?;
            let proposal: Option<String> = r.get(13)?;
            Ok(DraftCommentView {
                id: r.get(0)?,
                kind: if kind == "reply" { CommentKind::Reply } else { CommentKind::Thread },
                subject_type: if subject == "FILE" { SubjectType::File } else { SubjectType::Line },
                path: r.get(3)?,
                side: side.as_deref().and_then(Side::parse),
                line: r.get(5)?,
                start_side: start_side.as_deref().and_then(Side::parse),
                start_line: r.get(7)?,
                reply_to_thread: r.get(8)?,
                body_md: r.get(9)?,
                anchor_revision_id: r.get(10)?,
                anchor: anchor.and_then(|a| serde_json::from_str(&a).ok()),
                remap_status: r.get(12)?,
                remap_proposal: proposal.and_then(|p| serde_json::from_str(&p).ok()),
                resolution: r.get(14)?,
                staged: r.get(15)?,
                created_at: r.get(16)?,
                updated_at: r.get(17)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// A suggestion replaces the lines it's attached to, so it only makes sense
/// on the new code.
pub fn has_suggestion(body: &str) -> bool {
    body.lines().any(|l| l.trim_start().starts_with("```suggestion"))
}

pub(crate) struct RevisionFile {
    pub hunks: Vec<Hunk>,
    pub has_patch: bool,
    pub base_blob_oid: Option<String>,
    pub head_blob_oid: Option<String>,
}

pub(crate) fn revision_file(c: &Connection, revision_id: i64, path: &str) -> Result<Option<RevisionFile>> {
    let row: Option<(Option<String>, Option<String>, Option<String>)> = c
        .query_row(
            "SELECT patch, base_blob_oid, head_blob_oid FROM revision_file WHERE revision_id = ?1 AND path = ?2",
            params![revision_id, path],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((patch, base, head)) = row else { return Ok(None) };
    let hunks = match &patch {
        Some(p) => parse_patch(p).map_err(Error::Internal)?,
        None => vec![],
    };
    Ok(Some(RevisionFile { hunks, has_patch: patch.is_some(), base_blob_oid: base, head_blob_oid: head }))
}

fn side_snapshot(text: Option<&str>, lines_in_range: &[(u32, String)]) -> Option<SideSnapshot> {
    let (first, last) = (lines_in_range.first()?.0, lines_in_range.last()?.0);
    let all = text.map(split_lines).unwrap_or_default();
    let from = |a: usize, b: usize| -> Vec<String> {
        all.get(a.min(all.len())..b.min(all.len())).unwrap_or(&[]).iter().map(|s| s.to_string()).collect()
    };
    let f = first as usize;
    let l = last as usize;
    Some(SideSnapshot {
        first,
        last,
        lines: lines_in_range.iter().map(|(_, t)| t.clone()).collect(),
        before: from(f.saturating_sub(1 + CONTEXT), f - 1),
        after: from(l, l + CONTEXT),
    })
}

/// Captures the anchored lines (and a little context) on each side.
pub(crate) fn snapshot(
    rf: &RevisionFile,
    blobs: &BlobStore,
    start: Option<(Side, u32)>,
    end: (Side, u32),
) -> Result<AnchorSnapshot> {
    let (hi, end_idx) = crate::diff::locate(&rf.hunks, end.0, end.1)
        .ok_or_else(|| Error::Invalid("That line isn't part of the diff.".into()))?;
    let start_idx = match start {
        Some((s, l)) => crate::diff::locate(&rf.hunks, s, l).map(|(_, i)| i).unwrap_or(end_idx),
        None => end_idx,
    };
    let h = &rf.hunks[hi];
    let range = &h.lines[start_idx.min(end_idx)..=end_idx];
    let old: Vec<(u32, String)> = range
        .iter()
        .filter(|l| l.kind != LineKind::Add)
        .filter_map(|l| Some((l.old_no?, l.text.clone())))
        .collect();
    let new: Vec<(u32, String)> = range
        .iter()
        .filter(|l| l.kind != LineKind::Del)
        .filter_map(|l| Some((l.new_no?, l.text.clone())))
        .collect();
    let base_text = rf.base_blob_oid.as_deref().map(|o| blobs.get_text(o)).transpose()?.flatten();
    let head_text = rf.head_blob_oid.as_deref().map(|o| blobs.get_text(o)).transpose()?.flatten();
    let single_side = start.is_none_or(|s| s.0 == end.0);
    let (left, right) = if single_side {
        match end.0 {
            Side::Left => (side_snapshot(base_text.as_deref(), &old), None),
            Side::Right => (None, side_snapshot(head_text.as_deref(), &new)),
        }
    } else {
        (side_snapshot(base_text.as_deref(), &old), side_snapshot(head_text.as_deref(), &new))
    };
    Ok(AnchorSnapshot {
        hunk_header: format!(
            "@@ -{},{} +{},{} @@ {}",
            h.old_start, h.old_len, h.new_start, h.new_len, h.section
        )
        .trim_end()
        .to_string(),
        left,
        right,
        base_blob_oid: rf.base_blob_oid.clone(),
        head_blob_oid: rf.head_blob_oid.clone(),
    })
}

/// A file plus the range start and end still to snapshot.
type PendingAnchor = (RevisionFile, Option<(Side, u32)>, (Side, u32));

/// A comment that passed validation: its anchor and normalized range start.
pub(crate) struct Validated {
    pub anchor: Option<AnchorSnapshot>,
    pub start: Option<(Side, u32)>,
    /// The anchor still to capture. Reading blobs takes the database's read
    /// lock, so it happens after `validate` returns, never inside it.
    pending: Option<PendingAnchor>,
}

impl Validated {
    fn none() -> Self {
        Validated { anchor: None, start: None, pending: None }
    }

    pub(crate) fn capture(mut self, blobs: &BlobStore) -> Result<Self> {
        if let Some((rf, start, end)) = self.pending.take() {
            self.anchor = Some(snapshot(&rf, blobs, start, end)?);
        }
        Ok(self)
    }
}

/// Checks a new comment against GitHub's rules and captures its anchor.
pub(crate) fn validate(c: &Connection, pr_id: i64, n: &NewComment) -> Result<Validated> {
    let rev_pr: Option<i64> = c
        .query_row("SELECT pr_id FROM pr_revision WHERE id = ?1", [n.revision_id], |r| r.get(0))
        .optional()?;
    if rev_pr != Some(pr_id) {
        return Err(Error::Invalid("That revision isn't part of this pull request.".into()));
    }
    if n.body.trim().is_empty() {
        return Err(Error::Invalid("The comment is empty.".into()));
    }
    match n.kind {
        CommentKind::Reply => {
            let thread = n
                .reply_to_thread
                .as_deref()
                .ok_or_else(|| Error::Invalid("Reply to which thread?".into()))?;
            let can: Option<bool> = c
                .query_row(
                    "SELECT viewer_can_reply FROM review_thread WHERE node_id = ?1 AND pr_id = ?2",
                    params![thread, pr_id],
                    |r| r.get(0),
                )
                .optional()?;
            match can {
                None => Err(Error::Invalid("That thread no longer exists.".into())),
                Some(false) => Err(Error::Invalid("You can't reply to that thread.".into())),
                Some(true) if has_suggestion(&n.body) => Err(Error::Invalid(
                    "Suggestions can only be added to new comments on changed lines.".into(),
                )),
                Some(true) => Ok(Validated::none()),
            }
        }
        CommentKind::Thread => {
            let path = n.path.as_deref().ok_or_else(|| Error::Invalid("Which file?".into()))?;
            let rf = revision_file(c, n.revision_id, path)?
                .ok_or_else(|| Error::Invalid(format!("{path} isn't changed in this pull request.")))?;
            match n.subject_type {
                SubjectType::File => {
                    if has_suggestion(&n.body) {
                        return Err(Error::Invalid(
                            "Suggestions need a line range, not a whole file.".into(),
                        ));
                    }
                    Ok(Validated::none())
                }
                SubjectType::Line => {
                    if !rf.has_patch {
                        return Err(Error::Invalid(
                            "GitHub didn't send a diff for this file, so only file comments are possible."
                                .into(),
                        ));
                    }
                    let (side, line) =
                        n.side.zip(n.line).ok_or_else(|| Error::Invalid("Which line?".into()))?;
                    let start = match (n.start_side, n.start_line) {
                        (Some(s), Some(l)) if (s, l) != (side, line) => Some((s, l)),
                        (None, Some(l)) if l != line => Some((side, l)),
                        _ => None,
                    };
                    check_range(&rf.hunks, start, (side, line))
                        .map_err(|e| Error::Invalid(capitalize(&e)))?;
                    if has_suggestion(&n.body)
                        && (side == Side::Left || start.is_some_and(|s| s.0 == Side::Left))
                    {
                        return Err(Error::Invalid(
                            "Suggestions can only replace new or unchanged lines.".into(),
                        ));
                    }
                    Ok(Validated { anchor: None, start, pending: Some((rf, start, (side, line))) })
                }
            }
        }
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str() + ".",
        None => String::new(),
    }
}

/// The active draft for a PR, created on first use with the PR's current
/// revision as its basis.
pub(crate) fn ensure_draft(tx: &Transaction, pr_id: i64, now: &str) -> Result<i64> {
    if let Some(id) = active_draft_id(tx, pr_id)? {
        return Ok(id);
    }
    let rev: Option<i64> =
        tx.query_row("SELECT current_revision_id FROM pull_request WHERE id = ?1", [pr_id], |r| r.get(0))?;
    let rev = rev.ok_or_else(|| Error::Invalid("This pull request hasn't been synced yet.".into()))?;
    let cursor: Option<String> = tx.query_row(
        "SELECT max(submitted_at) FROM review WHERE pr_id = ?1 AND state != 'PENDING'",
        [pr_id],
        |r| r.get(0),
    )?;
    tx.execute(
        "INSERT INTO draft_review (pr_id, basis_revision_id, basis_review_cursor, status, created_at, updated_at)
         VALUES (?1, ?2, ?3, 'draft', ?4, ?4)",
        params![pr_id, rev, cursor, now],
    )?;
    Ok(tx.last_insert_rowid())
}

/// Edits are only allowed while the review is a draft.
pub(crate) fn require_editable(tx: &Connection, review_id: i64) -> Result<()> {
    let status: String =
        tx.query_row("SELECT status FROM draft_review WHERE id = ?1", [review_id], |r| r.get(0))?;
    match status.as_str() {
        "draft" => Ok(()),
        "queued" | "needs_attention" => {
            Err(Error::Invalid("This review is queued. Choose Edit to change it first.".into()))
        }
        _ => Err(Error::Invalid("This review is being sent and can't be changed.".into())),
    }
}

pub(crate) fn insert_comment(
    tx: &Transaction,
    review_id: i64,
    n: &NewComment,
    v: &Validated,
    now: &str,
) -> Result<i64> {
    let reply = n.kind == CommentKind::Reply;
    let path: Option<String> = if reply {
        tx.query_row("SELECT path FROM review_thread WHERE node_id = ?1", [&n.reply_to_thread], |r| r.get(0))
            .optional()?
    } else {
        n.path.clone()
    };
    let line_thread = !reply && n.subject_type == SubjectType::Line;
    tx.execute(
        "INSERT INTO draft_comment (draft_review_id, kind, reply_to_thread_node_id, path, subject_type, side, line,
           start_side, start_line, anchor_revision_id, anchor_snapshot, body_md, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13)",
        params![
            review_id,
            if reply { "reply" } else { "thread" },
            if reply { n.reply_to_thread.clone() } else { None },
            path,
            if !reply && n.subject_type == SubjectType::File { "FILE" } else { "LINE" },
            if line_thread { n.side.map(Side::as_str) } else { None },
            if line_thread { n.line } else { None },
            v.start.map(|s| s.0.as_str()),
            v.start.map(|s| s.1),
            if reply { None } else { Some(n.revision_id) },
            v.anchor.as_ref().map(serde_json::to_string).transpose()?,
            n.body,
            now,
        ],
    )?;
    Ok(tx.last_insert_rowid())
}

/// Checks a review before it's queued (DESIGN.md §2, edge case 22).
pub(crate) fn check_before_queue(c: &Connection, review_id: i64) -> Result<()> {
    let (pr_id, body, verdict): (i64, String, Option<String>) =
        c.query_row("SELECT pr_id, body_md, verdict FROM draft_review WHERE id = ?1", [review_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?;
    let verdict = verdict.as_deref().and_then(Verdict::parse).unwrap_or(Verdict::Comment);
    let own: bool =
        c.query_row("SELECT viewer_did_author FROM pull_request WHERE id = ?1", [pr_id], |r| r.get(0))?;
    let comments: i64 =
        c.query_row("SELECT count(*) FROM draft_comment WHERE draft_review_id = ?1", [review_id], |r| {
            r.get(0)
        })?;
    let empty_body = body.trim().is_empty();
    if own && verdict != Verdict::Comment {
        return Err(Error::Invalid("You can't approve or request changes on your own pull request.".into()));
    }
    if verdict == Verdict::RequestChanges && empty_body {
        return Err(Error::Invalid("Say what needs to change: requesting changes needs a summary.".into()));
    }
    if verdict == Verdict::Comment && empty_body && comments == 0 {
        return Err(Error::Invalid("The review is empty. Add a comment or a summary.".into()));
    }
    let blank: i64 = c.query_row(
        "SELECT count(*) FROM draft_comment WHERE draft_review_id = ?1 AND trim(body_md) = ''",
        [review_id],
        |r| r.get(0),
    )?;
    if blank > 0 {
        return Err(Error::Invalid("One of your comments is empty. Write something or delete it.".into()));
    }
    Ok(())
}

/// Draft editing, for the UI.
impl Core {
    pub fn draft(&self, pr_id: i64) -> Result<Option<DraftView>> {
        get_draft(&self.db, pr_id)
    }

    pub fn add_draft_comment(&self, pr_id: i64, n: NewComment) -> Result<DraftView> {
        let now = self.now();
        let v = self.db.read(|c| validate(c, pr_id, &n))?.capture(&self.blobs)?;
        let id = self.db.write(|tx| {
            let id = ensure_draft(tx, pr_id, &now)?;
            require_editable(tx, id)?;
            insert_comment(tx, id, &n, &v, &now)?;
            touch(tx, id, &now)?;
            Ok(id)
        })?;
        self.db.read(|c| draft_view(c, id))
    }

    pub fn update_draft_comment(&self, comment_id: i64, body: &str) -> Result<DraftView> {
        let now = self.now();
        let id = self.db.write(|tx| {
            let (id, side, start_side): (i64, Option<String>, Option<String>) = tx
                .query_row(
                    "SELECT draft_review_id, side, start_side FROM draft_comment WHERE id = ?1",
                    [comment_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?
                .ok_or_else(|| Error::NotFound(format!("comment {comment_id}")))?;
            require_editable(tx, id)?;
            if has_suggestion(body)
                && (side.as_deref() != Some("RIGHT") || start_side.as_deref() == Some("LEFT"))
            {
                return Err(Error::Invalid("Suggestions can only replace new or unchanged lines.".into()));
            }
            tx.execute(
                "UPDATE draft_comment SET body_md = ?2, updated_at = ?3 WHERE id = ?1",
                params![comment_id, body, now],
            )?;
            touch(tx, id, &now)?;
            Ok(id)
        })?;
        self.db.read(|c| draft_view(c, id))
    }

    pub fn delete_draft_comment(&self, comment_id: i64) -> Result<Option<DraftView>> {
        let now = self.now();
        let id = self.db.write(|tx| {
            let id: i64 = tx
                .query_row("SELECT draft_review_id FROM draft_comment WHERE id = ?1", [comment_id], |r| {
                    r.get(0)
                })
                .optional()?
                .ok_or_else(|| Error::NotFound(format!("comment {comment_id}")))?;
            require_editable(tx, id)?;
            tx.execute("DELETE FROM draft_comment WHERE id = ?1", [comment_id])?;
            touch(tx, id, &now)?;
            Ok(id)
        })?;
        self.db.read(|c| draft_view(c, id)).map(Some)
    }

    /// Sets the summary and/or verdict. `verdict: Some(None)` clears it.
    pub fn update_draft_review(
        &self,
        pr_id: i64,
        body: Option<&str>,
        verdict: Option<Option<Verdict>>,
    ) -> Result<DraftView> {
        let now = self.now();
        let id = self.db.write(|tx| {
            let id = ensure_draft(tx, pr_id, &now)?;
            require_editable(tx, id)?;
            if let Some(b) = body {
                tx.execute("UPDATE draft_review SET body_md = ?2 WHERE id = ?1", params![id, b])?;
            }
            if let Some(v) = verdict {
                tx.execute(
                    "UPDATE draft_review SET verdict = ?2 WHERE id = ?1",
                    params![id, v.map(Verdict::as_str)],
                )?;
            }
            touch(tx, id, &now)?;
            Ok(id)
        })?;
        self.db.read(|c| draft_view(c, id))
    }

    /// Queues the draft for sending (the outbox takes it from here).
    pub fn queue_review(&self, pr_id: i64) -> Result<DraftView> {
        let now = self.now();
        let id = self.db.write(|tx| {
            let id = active_draft_id(tx, pr_id)?.ok_or_else(|| Error::Invalid("There's no draft review.".into()))?;
            require_editable(tx, id)?;
            check_before_queue(tx, id)?;
            tx.execute(
                "UPDATE draft_review SET status = 'queued', queued_at = ?2, attempts = 0, next_attempt_at = NULL,
                   last_error = NULL, last_error_kind = NULL, updated_at = ?2 WHERE id = ?1",
                params![id, now],
            )?;
            crate::outbox::log(tx, id, &now, "queue", "ok", None)?;
            Ok(id)
        })?;
        self.emit(crate::Event::OutboxChanged { pr_id, draft_review_id: id, status: "queued".into() });
        self.kick_outbox();
        self.db.read(|c| draft_view(c, id))
    }

    /// Takes a queued review back to draft so it can be edited. If part of it
    /// was already staged on GitHub, that pending review is deleted later and
    /// staged again from scratch.
    pub fn unqueue_review(&self, pr_id: i64) -> Result<DraftView> {
        let now = self.now();
        let id = self.db.write(|tx| {
            let id = active_draft_id(tx, pr_id)?.ok_or_else(|| Error::Invalid("There's no draft review.".into()))?;
            let status: String = tx.query_row("SELECT status FROM draft_review WHERE id = ?1", [id], |r| r.get(0))?;
            if !matches!(status.as_str(), "queued" | "needs_attention" | "draft") {
                return Err(Error::Invalid("This review is being sent right now and can't be edited.".into()));
            }
            crate::outbox::release_pending_review(tx, id)?;
            tx.execute(
                "UPDATE draft_review SET status = 'draft', queued_at = NULL, next_attempt_at = NULL, updated_at = ?2
                 WHERE id = ?1",
                params![id, now],
            )?;
            crate::outbox::log(tx, id, &now, "unqueue", "ok", None)?;
            Ok(id)
        })?;
        self.emit(crate::Event::OutboxChanged { pr_id, draft_review_id: id, status: "draft".into() });
        self.db.read(|c| draft_view(c, id))
    }

    /// Throws the draft away. A pending review we staged on GitHub is deleted
    /// once online.
    pub fn discard_review(&self, pr_id: i64) -> Result<()> {
        let now = self.now();
        let id = self.db.write(|tx| {
            let Some(id) = active_draft_id(tx, pr_id)? else { return Ok(None) };
            let status: String =
                tx.query_row("SELECT status FROM draft_review WHERE id = ?1", [id], |r| r.get(0))?;
            if matches!(status.as_str(), "staging" | "submitting" | "preflight") {
                return Err(Error::Invalid(
                    "This review is being sent right now and can't be discarded.".into(),
                ));
            }
            crate::outbox::release_pending_review(tx, id)?;
            tx.execute(
                "UPDATE draft_review SET status = 'discarded', updated_at = ?2 WHERE id = ?1",
                params![id, now],
            )?;
            crate::outbox::log(tx, id, &now, "discard", "ok", None)?;
            Ok(Some(id))
        })?;
        if let Some(id) = id {
            self.emit(crate::Event::OutboxChanged { pr_id, draft_review_id: id, status: "discarded".into() });
            self.kick_outbox();
        }
        Ok(())
    }
}

fn touch(tx: &Transaction, review_id: i64, now: &str) -> Result<()> {
    tx.execute("UPDATE draft_review SET updated_at = ?2 WHERE id = ?1", params![review_id, now])?;
    Ok(())
}

pub fn get_draft(db: &Db, pr_id: i64) -> Result<Option<DraftView>> {
    db.read(|c| match active_draft_id(c, pr_id)? {
        Some(id) => Ok(Some(draft_view(c, id)?)),
        None => Ok(None),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_suggestions() {
        assert!(has_suggestion("nit:\n```suggestion\nlet x = 1;\n```"));
        assert!(has_suggestion("  ```suggestion"));
        assert!(!has_suggestion("```rust\nlet x = 1;\n```"));
    }
}

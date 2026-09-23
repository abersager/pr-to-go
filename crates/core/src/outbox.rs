//! The outbox: queued reviews on their way to GitHub (DESIGN.md §10).
//!
//! Each queued review moves through `queued → preflight → staging →
//! submitting → submitted`, one persisted step at a time, with
//! `needs_attention` when the user has to decide something.
//!
//! No duplicates, even across crashes and lost responses:
//! - Everything is first built as a *pending* review, which only the user can
//!   see. Only the final submit is visible to anyone else.
//! - Before a mutation is sent, `inflight` records what it is; once its result
//!   is stored, `inflight` is cleared in the same transaction. If `inflight`
//!   is set when a step starts, the previous attempt's outcome is unknown, so
//!   the server is checked (and matching work adopted) before anything is
//!   sent again.

use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::clock::parse_rfc3339;
use crate::drafts::Verdict;
use crate::error::{Error, Result};
use crate::github::{GhError, OpKind, queries};
use crate::remap::Proposal;
use crate::service::{Core, Event};

// ─── Attention ───────────────────────────────────────────────────────────

/// Why a review stopped and needs the user (DESIGN.md §10.3).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Reason {
    /// The PR's head or merge base moved: comments written on another
    /// revision need a new home, and an approval needs confirming.
    HeadMoved {
        from_revision: i64,
        to_revision: i64,
        comments: Vec<i64>,
        verdict_stale: bool,
    },
    PrStateChanged {
        state: String,
    },
    PrLocked,
    RepoArchived,
    /// Someone requested changes after you drafted an approval (Hubtty's
    /// "held" rule).
    NewBlockingReview {
        review: String,
        author: Option<String>,
    },
    /// You have a pending review on this PR from github.com.
    ExistingPendingReview {
        review: String,
    },
    ReplyTargetGone {
        comment: i64,
    },
    CommentRejected {
        comment: Option<i64>,
        message: String,
    },
    /// Submitting against the reviewed commit isn't possible any more.
    ReviewedCommitUnavailable {
        message: String,
    },
    Auth {
        message: String,
    },
    Permission {
        message: String,
        sso_url: Option<String>,
    },
    PrGone,
}

impl Reason {
    /// Reasons the user can accept and carry on have a key; the rest need an
    /// edit, a new token, or a different target.
    pub fn ack_key(&self) -> Option<String> {
        match self {
            Reason::PrStateChanged { state } => Some(format!("pr_state:{state}")),
            Reason::PrLocked => Some("pr_locked".into()),
            Reason::NewBlockingReview { review, .. } => Some(format!("blocking:{review}")),
            Reason::ExistingPendingReview { review } => Some(format!("merge_pending:{review}")),
            _ => None,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Attention {
    pub reasons: Vec<Reason>,
    #[serde(default)]
    pub acknowledged: Vec<String>,
}

fn attention(c: &Connection, id: i64) -> Result<Attention> {
    let raw: Option<String> =
        c.query_row("SELECT attention FROM draft_review WHERE id = ?1", [id], |r| r.get(0))?;
    Ok(raw.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default())
}

// ─── Small helpers ───────────────────────────────────────────────────────

/// Appends to the review's submission log (kept for debugging and support).
pub(crate) fn log(
    tx: &Transaction,
    review_id: i64,
    now: &str,
    step: &str,
    outcome: &str,
    detail: Option<&str>,
) -> Result<()> {
    tracing::info!(
        "outbox review {review_id}: {step} {outcome}{}",
        detail.map(|d| format!(" ({d})")).unwrap_or_default()
    );
    tx.execute(
        "INSERT INTO outbox_log (draft_review_id, at, step, outcome, detail) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![review_id, now, step, outcome, detail],
    )?;
    Ok(())
}

/// Forgets the pending review staged on GitHub for this draft (the draft is
/// being edited or discarded). If we created it, it's scheduled for deletion;
/// a pending review the user started on github.com is never deleted.
pub(crate) fn release_pending_review(tx: &Transaction, review_id: i64) -> Result<()> {
    let row: Option<(Option<String>, bool)> = tx
        .query_row(
            "SELECT server_pending_review_id, pending_review_owned FROM draft_review WHERE id = ?1",
            [review_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((Some(pending), owned)) = row {
        if owned {
            tx.execute(
                "UPDATE draft_review SET cleanup_review_id = ?2 WHERE id = ?1",
                params![review_id, pending],
            )?;
        }
        tx.execute(
            "UPDATE draft_review SET server_pending_review_id = NULL, pending_review_owned = 0,
               preflight_pending_seen = NULL, inflight = NULL WHERE id = ?1",
            [review_id],
        )?;
        tx.execute(
            "UPDATE draft_comment SET staged_node_id = NULL, staged_thread_node_id = NULL WHERE draft_review_id = ?1",
            [review_id],
        )?;
    }
    Ok(())
}

/// Retry delay after the n-th consecutive failure: 5 s doubling to 5 min.
fn backoff(attempts: i64) -> Duration {
    Duration::from_secs((5u64 << (attempts.clamp(1, 7) - 1)).min(300))
}

/// Result of one outbox step.
enum Step {
    /// Take the next step right away.
    Continue,
    /// Nothing more to do for now (waiting, blocked, or finished).
    Stop,
}

/// One draft comment as the outbox sees it.
#[derive(Clone, Debug)]
struct Staged {
    id: i64,
    kind: String,
    subject: String,
    path: Option<String>,
    side: Option<String>,
    line: Option<i64>,
    start_side: Option<String>,
    start_line: Option<i64>,
    reply_to: Option<String>,
    body: String,
    resolution: Option<String>,
    staged: Option<String>,
}

impl Staged {
    /// A file-level comment, or a line comment converted to one.
    fn is_file(&self) -> bool {
        self.subject == "FILE" || self.resolution.as_deref() == Some("to_file")
    }

    fn is_reply(&self) -> bool {
        self.kind == "reply"
    }

    /// Goes into the review as a thread or reply (not dropped or folded into
    /// the summary).
    fn is_sent(&self) -> bool {
        !matches!(self.resolution.as_deref(), Some("drop") | Some("to_summary"))
    }
}

fn comments(c: &Connection, review_id: i64) -> Result<Vec<Staged>> {
    let mut st = c.prepare(
        "SELECT id, kind, subject_type, path, side, line, start_side, start_line, reply_to_thread_node_id, body_md,
                resolution, staged_node_id
         FROM draft_comment WHERE draft_review_id = ?1 ORDER BY id",
    )?;
    let rows = st
        .query_map([review_id], |r| {
            Ok(Staged {
                id: r.get(0)?,
                kind: r.get(1)?,
                subject: r.get(2)?,
                path: r.get(3)?,
                side: r.get(4)?,
                line: r.get(5)?,
                start_side: r.get(6)?,
                start_line: r.get(7)?,
                reply_to: r.get(8)?,
                body: r.get(9)?,
                resolution: r.get(10)?,
                staged: r.get(11)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// The review's row, as the outbox needs it.
#[derive(Debug)]
struct Row {
    pr_id: i64,
    pr_node_id: String,
    body: String,
    verdict: Option<String>,
    basis_revision_id: i64,
    target_mode: String,
    target_revision_id: Option<i64>,
    pending: Option<String>,
    owned: bool,
    preflight_seen: Option<String>,
    inflight: Option<String>,
}

fn row(c: &Connection, id: i64) -> Result<Row> {
    Ok(c.query_row(
        "SELECT d.pr_id, p.node_id, d.body_md, d.verdict, d.basis_revision_id, d.target_mode,
                d.target_revision_id, d.server_pending_review_id, d.pending_review_owned,
                d.preflight_pending_seen, d.inflight
         FROM draft_review d JOIN pull_request p ON p.id = d.pr_id WHERE d.id = ?1",
        [id],
        |r| {
            Ok(Row {
                pr_id: r.get(0)?,
                pr_node_id: r.get(1)?,
                body: r.get(2)?,
                verdict: r.get(3)?,
                basis_revision_id: r.get(4)?,
                target_mode: r.get(5)?,
                target_revision_id: r.get(6)?,
                pending: r.get(7)?,
                owned: r.get(8)?,
                preflight_seen: r.get(9)?,
                inflight: r.get(10)?,
            })
        },
    )?)
}

/// Normalizes a comment body for matching drafts against what GitHub stored.
fn norm(body: &str) -> String {
    body.replace("\r\n", "\n").trim().to_string()
}

/// Server-side comments of our pending review.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServerComment {
    id: String,
    path: String,
    line: Option<i64>,
    start_line: Option<i64>,
    body: String,
    #[serde(default)]
    subject_type: Option<String>,
    #[serde(default)]
    reply_to: Option<queries::Id>,
}

/// Matches unstaged drafts to server comments nobody has claimed yet.
/// Returns (draft id, server comment id) pairs.
fn match_comments(
    c: &Connection,
    drafts: &[Staged],
    server: &[ServerComment],
    claimed: &[String],
) -> Result<Vec<(i64, String)>> {
    let mut taken: Vec<&str> = claimed.iter().map(String::as_str).collect();
    let mut out = Vec::new();
    for d in drafts.iter().filter(|d| d.staged.is_none() && d.is_sent()) {
        let want = norm(&d.body);
        let hit = if d.is_reply() {
            // A reply's `replyTo` is a comment in the target thread.
            let thread_comments: Vec<String> = match &d.reply_to {
                Some(t) => {
                    let mut st = c.prepare(
                        "SELECT c.node_id FROM review_comment c JOIN review_thread t ON t.id = c.thread_id
                         WHERE t.node_id = ?1",
                    )?;
                    st.query_map([t], |r| r.get(0))?.collect::<Result<_, _>>()?
                }
                None => vec![],
            };
            server.iter().find(|s| {
                !taken.contains(&s.id.as_str())
                    && norm(&s.body) == want
                    && s.reply_to.as_ref().is_some_and(|r| thread_comments.contains(&r.id))
            })
        } else {
            server.iter().find(|s| {
                !taken.contains(&s.id.as_str())
                    && s.reply_to.is_none()
                    && norm(&s.body) == want
                    && Some(s.path.as_str()) == d.path.as_deref()
                    && (d.is_file() && s.subject_type.as_deref().is_none_or(|t| t == "FILE")
                        || (!d.is_file() && s.line == d.line && s.start_line == d.start_line))
            })
        };
        if let Some(s) = hit {
            taken.push(&s.id);
            out.push((d.id, s.id.clone()));
        }
    }
    Ok(out)
}

// ─── Preflight checks (pure over local state) ────────────────────────────

/// Reasons the review can't go as it is (DESIGN.md §10.3), minus the ones
/// the user already accepted. `pending` is the viewer's pending reviews on
/// GitHub.
pub(crate) fn evaluate(c: &Connection, id: i64, pending: &[String]) -> Result<Vec<Reason>> {
    let d = row(c, id)?;
    let (state, locked, archived, current): (String, bool, bool, Option<i64>) = c.query_row(
        "SELECT p.state, p.locked, r.is_archived, p.current_revision_id
         FROM pull_request p JOIN repo r ON r.id = p.repo_id WHERE p.id = ?1",
        [d.pr_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    let current = current.unwrap_or(d.basis_revision_id);
    let ack = attention(c, id)?.acknowledged;
    let drafts = comments(c, id)?;
    let mut reasons = Vec::new();

    if archived {
        reasons.push(Reason::RepoArchived);
    }
    if state != "OPEN" {
        reasons.push(Reason::PrStateChanged { state });
    }
    if locked {
        reasons.push(Reason::PrLocked);
    }

    // Where the review will go, and which comments aren't anchored there.
    let target = if d.target_mode == "reviewed_commit" {
        d.basis_revision_id
    } else {
        d.target_revision_id.unwrap_or(d.basis_revision_id)
    };
    let destination = if d.target_mode == "reviewed_commit" { target } else { current };
    let mut stray = Vec::new();
    {
        let mut st = c.prepare(
            "SELECT id FROM draft_comment WHERE draft_review_id = ?1 AND kind = 'thread'
               AND (resolution IS NULL OR resolution = 'remap')
               AND anchor_revision_id IS NOT ?2",
        )?;
        for r in st.query_map(params![id, destination], |r| r.get::<_, i64>(0))? {
            stray.push(r?);
        }
    }
    let judged: bool = d.verdict.as_deref().is_some_and(|v| v == "APPROVE" || v == "REQUEST_CHANGES");
    // A verdict was given on the code as it was; comments carry their own
    // anchors. A plain summary doesn't care where the head is.
    let verdict_stale = d.target_mode == "current_head" && judged && target != current;
    if !stray.is_empty() || verdict_stale {
        reasons.push(Reason::HeadMoved {
            from_revision: target,
            to_revision: destination,
            comments: stray,
            verdict_stale,
        });
    }

    if d.verdict.as_deref() == Some("APPROVE") {
        let viewer: Option<String> =
            c.query_row("SELECT login FROM account LIMIT 1", [], |r| r.get(0)).optional()?;
        let cursor: Option<String> =
            c.query_row("SELECT basis_review_cursor FROM draft_review WHERE id = ?1", [id], |r| r.get(0))?;
        let mut st = c.prepare(
            "SELECT node_id, author_login FROM review WHERE pr_id = ?1 AND state = 'CHANGES_REQUESTED'
               AND (?2 IS NULL OR author_login IS NOT ?2) AND (?3 IS NULL OR submitted_at > ?3)",
        )?;
        for r in st.query_map(params![d.pr_id, viewer, cursor], |r| Ok((r.get(0)?, r.get(1)?)))? {
            let (review, author) = r?;
            reasons.push(Reason::NewBlockingReview { review, author });
        }
    }

    for p in pending {
        if Some(p) != d.pending.as_ref() {
            reasons.push(Reason::ExistingPendingReview { review: p.clone() });
        }
    }

    for r in drafts.iter().filter(|r| r.is_reply() && r.is_sent()) {
        let can: Option<bool> = c
            .query_row("SELECT viewer_can_reply FROM review_thread WHERE node_id = ?1", [&r.reply_to], |x| {
                x.get(0)
            })
            .optional()?;
        if can != Some(true) {
            reasons.push(Reason::ReplyTargetGone { comment: r.id });
        }
    }

    reasons.retain(|r| r.ack_key().is_none_or(|k| !ack.contains(&k)));
    Ok(reasons)
}

// ─── The engine ──────────────────────────────────────────────────────────

/// What `run_outbox_once` did, for tests and the UI.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboxRun {
    pub worked_on: Vec<i64>,
    /// When the next waiting review is due (local clock), if any.
    pub next_due: Option<String>,
}

impl Core {
    /// Wakes the outbox worker (after queueing, or when coming online).
    pub fn kick_outbox(&self) {
        self.outbox_wake.notify_one();
    }

    /// Test hook: fail with a simulated crash right after the named step's
    /// request succeeded, before its result is stored.
    #[doc(hidden)]
    pub fn set_crash_point(&self, point: Option<&str>) {
        *self.crash_point.lock().unwrap() = point.map(str::to_owned);
    }

    fn crash_if(&self, point: &str) -> Result<()> {
        let mut p = self.crash_point.lock().unwrap();
        if p.as_deref() == Some(point) {
            *p = None;
            return Err(Error::Internal(format!("simulated crash after {point}")));
        }
        Ok(())
    }

    /// Runs the background worker: the outbox whenever it's kicked or
    /// something is due, and a periodic sync of open PRs.
    /// The background worker: the outbox, and the inbox every 15 minutes.
    /// Spawn this on the host's runtime; it never returns.
    pub async fn run_background(self: std::sync::Arc<Self>) {
        tokio::join!(self.outbox_loop(), self.inbox_loop(Duration::from_secs(15 * 60)));
    }

    async fn outbox_loop(&self) {
        loop {
            let wait = match self.run_outbox_once().await {
                Ok(run) => run
                    .next_due
                    .and_then(|t| parse_rfc3339(&t))
                    .and_then(|t| t.duration_since(self.clock.now()).ok())
                    .unwrap_or(Duration::from_secs(60))
                    .clamp(Duration::from_millis(200), Duration::from_secs(60)),
                Err(e) => {
                    tracing::error!("outbox: {e}");
                    Duration::from_secs(30)
                }
            };
            tokio::select! {
                _ = self.outbox_wake.notified() => {}
                _ = tokio::time::sleep(wait) => {}
            }
        }
    }

    /// Advances every review that's due as far as it can go right now.
    pub async fn run_outbox_once(&self) -> Result<OutboxRun> {
        let mut run = OutboxRun::default();
        if self.gh.is_work_offline() || !self.gh.has_token() {
            return Ok(run);
        }
        self.cleanup_pending_reviews().await?;
        let now = self.now();
        let due: Vec<i64> = self.db.read(|c| {
            let mut st = c.prepare(
                "SELECT id FROM draft_review
                 WHERE status IN ('queued', 'preflight', 'staging', 'submitting')
                   AND (next_attempt_at IS NULL OR next_attempt_at <= ?1)
                 ORDER BY queued_at, id",
            )?;
            let ids = st.query_map([&now], |r| r.get(0))?.collect::<Result<Vec<_>, _>>()?;
            Ok(ids)
        })?;
        for id in due {
            run.worked_on.push(id);
            self.outbox_busy.lock().unwrap().insert(id);
            let result = async {
                for _ in 0..500 {
                    match self.advance(id).await? {
                        Step::Continue => continue,
                        Step::Stop => break,
                    }
                }
                Ok::<(), Error>(())
            }
            .await;
            self.outbox_busy.lock().unwrap().remove(&id);
            result?;
            // Just sent: sync the PR so the posted review shows up as
            // GitHub has it. Best effort; the next sync would catch it too.
            let (status, pr_id): (String, i64) = self.db.read(|c| {
                Ok(c.query_row("SELECT status, pr_id FROM draft_review WHERE id = ?1", [id], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })?)
            })?;
            if status == "submitted" {
                let _ = self.sync_pr(pr_id).await;
            }
        }
        run.next_due = self.db.read(|c| {
            Ok(c.query_row(
                "SELECT min(next_attempt_at) FROM draft_review
                 WHERE status IN ('queued', 'preflight', 'staging', 'submitting') AND next_attempt_at IS NOT NULL",
                [],
                |r| r.get(0),
            )?)
        })?;
        Ok(run)
    }

    /// Test hook: takes exactly one outbox step for a review. Returns whether
    /// another step would follow right away.
    #[doc(hidden)]
    pub async fn step_review(&self, draft_review_id: i64) -> Result<bool> {
        Ok(matches!(self.advance(draft_review_id).await?, Step::Continue))
    }

    /// Tries again now, ignoring the backoff.
    pub fn retry_review(&self, pr_id: i64) -> Result<()> {
        self.db.write(|tx| {
            tx.execute(
                "UPDATE draft_review SET next_attempt_at = NULL WHERE pr_id = ?1 AND status IN
                   ('queued', 'preflight', 'staging', 'submitting')",
                [pr_id],
            )?;
            Ok(())
        })?;
        self.kick_outbox();
        Ok(())
    }

    fn set_status(&self, id: i64, status: &str, step: &str, detail: Option<&str>) -> Result<()> {
        let now = self.now();
        let pr_id = self.db.write(|tx| {
            tx.execute(
                "UPDATE draft_review SET status = ?2, updated_at = ?3, attempts = 0, next_attempt_at = NULL,
                   last_error = NULL, last_error_kind = NULL WHERE id = ?1",
                params![id, status, now],
            )?;
            log(tx, id, &now, step, status, detail)?;
            Ok(tx.query_row("SELECT pr_id FROM draft_review WHERE id = ?1", [id], |r| r.get::<_, i64>(0))?)
        })?;
        self.emit(Event::OutboxChanged { pr_id, draft_review_id: id, status: status.into() });
        Ok(())
    }

    fn needs_attention(&self, id: i64, reasons: Vec<Reason>) -> Result<Step> {
        let now = self.now();
        let pr_id = self.db.write(|tx| {
            let mut a = attention(tx, id)?;
            a.reasons = reasons;
            let detail = serde_json::to_string(&a.reasons)?;
            tx.execute(
                "UPDATE draft_review SET status = 'needs_attention', attention = ?2, updated_at = ?3,
                   next_attempt_at = NULL, inflight = NULL WHERE id = ?1",
                params![id, serde_json::to_string(&a)?, now],
            )?;
            log(tx, id, &now, "attention", "needs_attention", Some(&detail))?;
            Ok(tx.query_row("SELECT pr_id FROM draft_review WHERE id = ?1", [id], |r| r.get::<_, i64>(0))?)
        })?;
        self.emit(Event::OutboxChanged { pr_id, draft_review_id: id, status: "needs_attention".into() });
        Ok(Step::Stop)
    }

    /// Handles a GitHub error during a step: waits and retries for transient
    /// ones, asks the user for the rest.
    fn on_error(&self, id: i64, step: &str, e: GhError) -> Result<Step> {
        match e {
            GhError::Unauthorized | GhError::NoToken => {
                self.needs_attention(id, vec![Reason::Auth { message: e.to_string() }])
            }
            GhError::Forbidden { message, sso_url } => {
                self.needs_attention(id, vec![Reason::Permission { message, sso_url }])
            }
            GhError::NotFound => self.needs_attention(id, vec![Reason::PrGone]),
            GhError::Unprocessable(message) => {
                self.needs_attention(id, vec![Reason::CommentRejected { comment: None, message }])
            }
            GhError::Protocol(_)
            | GhError::Offline(_)
            | GhError::Ambiguous(_)
            | GhError::Server(_)
            | GhError::RateLimited { .. } => {
                let now = self.clock.now();
                let wait = match &e {
                    GhError::RateLimited { retry_after_s, .. } => Duration::from_secs(*retry_after_s),
                    _ => Duration::ZERO,
                };
                let msg = e.to_string();
                let kind = match &e {
                    GhError::Offline(_) => "offline",
                    GhError::Ambiguous(_) => "ambiguous",
                    GhError::RateLimited { .. } => "rate_limited",
                    _ => "server",
                };
                // `inflight` is left alone: only the mutation's own call site
                // knows whether its request provably never left (see
                // `mutation_failed`). A failed reconcile query proves nothing.
                self.db.write(|tx| {
                    let attempts: i64 =
                        tx.query_row("SELECT attempts FROM draft_review WHERE id = ?1", [id], |r| r.get(0))?;
                    let attempts = attempts + 1;
                    let next = now + backoff(attempts).max(wait);
                    tx.execute(
                        "UPDATE draft_review SET attempts = ?2, next_attempt_at = ?3, last_error = ?4,
                           last_error_kind = ?5 WHERE id = ?1",
                        params![id, attempts, crate::clock::rfc3339(next), msg, kind],
                    )?;
                    log(tx, id, &crate::clock::rfc3339(now), step, kind, Some(&msg))
                })?;
                if let GhError::Offline(_) = e {
                    self.observe::<()>(&Err(e.clone()));
                }
                Ok(Step::Stop)
            }
        }
    }

    async fn advance(&self, id: i64) -> Result<Step> {
        let status: String = self.db.read(|c| {
            Ok(c.query_row("SELECT status FROM draft_review WHERE id = ?1", [id], |r| r.get(0))?)
        })?;
        match status.as_str() {
            "queued" => {
                self.set_status(id, "preflight", "start", None)?;
                Ok(Step::Continue)
            }
            "preflight" => self.preflight(id).await,
            "staging" => self.stage(id).await,
            "submitting" => self.submit(id).await,
            _ => Ok(Step::Stop),
        }
    }

    async fn pending_reviews(&self, pr_node_id: &str) -> std::result::Result<Vec<String>, GhError> {
        let v: Value = self
            .gh
            .graphql(
                "PendingReviews",
                include_str!("github/graphql/pending_reviews.graphql"),
                json!({ "prId": pr_node_id }),
                OpKind::Query,
            )
            .await?;
        Ok(v["node"]["reviews"]["nodes"]
            .as_array()
            .map(|a| a.iter().filter_map(|n| n["id"].as_str().map(str::to_owned)).collect())
            .unwrap_or_default())
    }

    async fn server_comments(
        &self,
        review: &str,
    ) -> std::result::Result<Option<Vec<ServerComment>>, GhError> {
        let mut out = Vec::new();
        let mut after: Option<String> = None;
        loop {
            let v: Value = match self
                .gh
                .graphql(
                    "ReviewComments",
                    include_str!("github/graphql/review_comments.graphql"),
                    json!({ "reviewId": review, "after": after }),
                    OpKind::Query,
                )
                .await
            {
                Err(GhError::NotFound) => return Ok(None),
                other => other?,
            };
            let conn = &v["node"]["comments"];
            let nodes: Vec<ServerComment> = serde_json::from_value(conn["nodes"].clone())
                .map_err(|e| GhError::Protocol(format!("ReviewComments: {e}")))?;
            out.extend(nodes);
            if conn["pageInfo"]["hasNextPage"].as_bool() != Some(true) {
                return Ok(Some(out));
            }
            after = conn["pageInfo"]["endCursor"].as_str().map(str::to_owned);
        }
    }

    // ── Preflight ──

    async fn preflight(&self, id: i64) -> Result<Step> {
        let d = self.db.read(|c| row(c, id))?;
        let pr_ref = self.pr_ref(d.pr_id)?;
        match self.sync_ref(&pr_ref, Some(d.pr_id)).await {
            Ok(_) => {}
            Err(Error::GitHub(e)) => return self.on_error(id, "preflight-sync", e),
            Err(e) => return Err(e),
        }
        let pending = match self.pending_reviews(&d.pr_node_id).await {
            Ok(p) => p,
            Err(e) => return self.on_error(id, "preflight-pending", e),
        };
        // Our own pending review from an earlier attempt vanished (deleted on
        // github.com, say): stage everything again.
        if let Some(ours) = &d.pending
            && !pending.contains(ours)
        {
            self.db.write(|tx| release_pending_review(tx, id))?;
        }
        let reasons = self.db.read(|c| evaluate(c, id, &pending))?;
        if !reasons.is_empty() {
            for r in &reasons {
                if let Reason::HeadMoved { comments, to_revision, .. } = r {
                    self.store_proposals(comments, *to_revision)?;
                }
            }
            return self.needs_attention(id, reasons);
        }
        let ack = self.db.read(|c| attention(c, id))?.acknowledged;
        let now = self.now();
        self.db.write(|tx| {
            let d = row(tx, id)?;
            let current: Option<i64> =
                tx.query_row("SELECT current_revision_id FROM pull_request WHERE id = ?1", [d.pr_id], |r| r.get(0))?;
            let target = match d.target_mode.as_str() {
                "reviewed_commit" => d.basis_revision_id,
                _ => current.unwrap_or(d.basis_revision_id),
            };
            // Adding to the user's own pending review from github.com, if
            // they chose that.
            let adopt = pending.iter().find(|p| ack.contains(&format!("merge_pending:{p}")));
            if let (None, Some(p)) = (&d.pending, adopt) {
                tx.execute(
                    "UPDATE draft_review SET server_pending_review_id = ?2, pending_review_owned = 0 WHERE id = ?1",
                    params![id, p],
                )?;
            }
            let seen = pending.first().cloned().unwrap_or_default();
            tx.execute(
                "UPDATE draft_review SET status = 'staging', target_revision_id = ?2, preflight_pending_seen = ?3,
                   attempts = 0, next_attempt_at = NULL, last_error = NULL, last_error_kind = NULL, updated_at = ?4
                 WHERE id = ?1",
                params![id, target, seen, now],
            )?;
            log(tx, id, &now, "preflight", "staging", None)
        })?;
        self.emit_status(id, "staging")?;
        Ok(Step::Continue)
    }

    fn emit_status(&self, id: i64, status: &str) -> Result<()> {
        let pr_id: i64 = self
            .db
            .read(|c| Ok(c.query_row("SELECT pr_id FROM draft_review WHERE id = ?1", [id], |r| r.get(0))?))?;
        self.emit(Event::OutboxChanged { pr_id, draft_review_id: id, status: status.into() });
        Ok(())
    }

    fn set_inflight(&self, id: i64, what: &str) -> Result<()> {
        self.db.write(|tx| {
            tx.execute("UPDATE draft_review SET inflight = ?2 WHERE id = ?1", params![id, what])?;
            Ok(())
        })
    }

    // ── Staging ──

    async fn stage(&self, id: i64) -> Result<Step> {
        let d = self.db.read(|c| row(c, id))?;
        if let Some(what) = &d.inflight {
            return self.reconcile(id, &d, what.clone()).await;
        }
        let drafts = self.db.read(|c| comments(c, id))?;
        let Some(review) = d.pending.clone() else {
            return self.create_pending_review(id, &d, &drafts).await;
        };
        let Some(next) = drafts.iter().find(|c| c.staged.is_none() && c.is_sent()).cloned() else {
            self.set_status(id, "submitting", "staged", None)?;
            return Ok(Step::Continue);
        };
        self.set_inflight(id, &format!("comment:{}", next.id))?;
        let res: std::result::Result<Value, GhError> = if next.is_reply() {
            self.gh
                .graphql(
                    "AddReply",
                    include_str!("github/graphql/add_reply.graphql"),
                    json!({ "threadId": next.reply_to, "reviewId": review, "body": next.body }),
                    OpKind::Mutation,
                )
                .await
        } else {
            let file = next.is_file();
            self.gh
                .graphql(
                    "AddThread",
                    include_str!("github/graphql/add_thread.graphql"),
                    json!({
                        "reviewId": review,
                        "path": next.path,
                        "body": next.body,
                        "subjectType": if file { "FILE" } else { "LINE" },
                        "line": if file { None } else { next.line },
                        "side": if file { None } else { next.side.clone() },
                        "startLine": if file { None } else { next.start_line },
                        "startSide": if file { None } else { next.start_side.clone() },
                    }),
                    OpKind::Mutation,
                )
                .await
        };
        match res {
            Ok(v) => {
                self.crash_if(if next.is_reply() { "after_add_reply" } else { "after_add_thread" })?;
                let (comment, thread) = if next.is_reply() {
                    (v["addPullRequestReviewThreadReply"]["comment"]["id"].as_str().map(str::to_owned), None)
                } else {
                    let t = &v["addPullRequestReviewThread"]["thread"];
                    (
                        t["comments"]["nodes"][0]["id"].as_str().map(str::to_owned),
                        t["id"].as_str().map(str::to_owned),
                    )
                };
                let now = self.now();
                self.db.write(|tx| {
                    tx.execute(
                        "UPDATE draft_comment SET staged_node_id = ?2, staged_thread_node_id = ?3 WHERE id = ?1",
                        params![next.id, comment.clone().unwrap_or_default(), thread],
                    )?;
                    tx.execute("UPDATE draft_review SET inflight = NULL, attempts = 0 WHERE id = ?1", [id])?;
                    log(tx, id, &now, "stage-comment", "ok", Some(&next.id.to_string()))
                })?;
                Ok(Step::Continue)
            }
            Err(GhError::Unprocessable(message)) => {
                self.db.write(|tx| {
                    tx.execute("UPDATE draft_review SET inflight = NULL WHERE id = ?1", [id])?;
                    Ok(())
                })?;
                self.needs_attention(id, vec![Reason::CommentRejected { comment: Some(next.id), message }])
            }
            Err(GhError::NotFound) => {
                // Either our pending review or the thread we reply to is gone.
                match self.pending_reviews(&d.pr_node_id).await {
                    Ok(p) if !p.contains(&review) => {
                        self.db.write(|tx| release_pending_review(tx, id))?;
                        Ok(Step::Continue)
                    }
                    Ok(_) if next.is_reply() => {
                        self.db.write(|tx| {
                            tx.execute("UPDATE draft_review SET inflight = NULL WHERE id = ?1", [id])?;
                            Ok(())
                        })?;
                        self.needs_attention(id, vec![Reason::ReplyTargetGone { comment: next.id }])
                    }
                    Ok(_) => self.mutation_failed(id, "stage-comment", GhError::NotFound),
                    Err(e) => self.on_error(id, "stage-comment", e),
                }
            }
            Err(e) => self.mutation_failed(id, "stage-comment", e),
        }
    }

    /// A mutation failed. If its request never reached GitHub (a connect
    /// error) or GitHub answered with a definite refusal, nothing needs
    /// reconciling; otherwise `inflight` stays set so the next attempt checks
    /// GitHub first.
    fn mutation_failed(&self, id: i64, step: &str, e: GhError) -> Result<Step> {
        let definite = !matches!(e, GhError::Ambiguous(_) | GhError::Protocol(_));
        if definite {
            self.db.write(|tx| {
                tx.execute("UPDATE draft_review SET inflight = NULL WHERE id = ?1", [id])?;
                Ok(())
            })?;
        }
        self.on_error(id, step, e)
    }

    async fn create_pending_review(&self, id: i64, d: &Row, drafts: &[Staged]) -> Result<Step> {
        let commit: String = self.db.read(|c| {
            Ok(c.query_row(
                "SELECT head_oid FROM pr_revision WHERE id = ?1",
                [d.target_revision_id.unwrap_or(d.basis_revision_id)],
                |r| r.get(0),
            )?)
        })?;
        // Line threads go in the create call itself; everything else follows
        // one by one.
        let batch: Vec<&Staged> =
            drafts.iter().filter(|c| c.is_sent() && !c.is_reply() && !c.is_file()).collect();
        let threads: Vec<Value> = batch
            .iter()
            .map(|c| {
                json!({
                    "path": c.path, "line": c.line, "side": c.side,
                    "startLine": c.start_line, "startSide": c.start_side, "body": c.body,
                })
            })
            .collect();
        self.set_inflight(id, "create")?;
        let res: std::result::Result<Value, GhError> = self
            .gh
            .graphql(
                "AddReview",
                include_str!("github/graphql/add_review.graphql"),
                json!({ "prId": d.pr_node_id, "commit": commit, "threads": threads }),
                OpKind::Mutation,
            )
            .await;
        match res {
            Ok(v) => {
                self.crash_if("after_create_review")?;
                let review = &v["addPullRequestReview"]["pullRequestReview"];
                let review_id = review["id"].as_str().unwrap_or_default().to_string();
                let server: Vec<ServerComment> =
                    serde_json::from_value(review["comments"]["nodes"].clone()).unwrap_or_default();
                self.adopt(id, &review_id, true, &server)?;
                Ok(Step::Continue)
            }
            Err(GhError::Unprocessable(message)) => {
                self.db.write(|tx| {
                    tx.execute("UPDATE draft_review SET inflight = NULL WHERE id = ?1", [id])?;
                    Ok(())
                })?;
                // A pending review appeared since preflight: the "one pending
                // review per user" rule.
                match self.pending_reviews(&d.pr_node_id).await {
                    Ok(p) if !p.is_empty() => {
                        return self.needs_attention(
                            id,
                            vec![Reason::ExistingPendingReview { review: p[0].clone() }],
                        );
                    }
                    Ok(_) => {}
                    Err(e) => return self.on_error(id, "create", e),
                }
                if message.to_lowercase().contains("commit") {
                    return self.needs_attention(id, vec![Reason::ReviewedCommitUnavailable { message }]);
                }
                match batch.len() {
                    0 => self.needs_attention(id, vec![Reason::CommentRejected { comment: None, message }]),
                    1 => self.needs_attention(
                        id,
                        vec![Reason::CommentRejected { comment: Some(batch[0].id), message }],
                    ),
                    // One of several threads was refused. Start an empty
                    // review and add them one at a time to find which.
                    _ => {
                        self.set_inflight(id, "create")?;
                        match self
                            .gh
                            .graphql::<Value>(
                                "AddReview",
                                include_str!("github/graphql/add_review.graphql"),
                                json!({ "prId": d.pr_node_id, "commit": commit, "threads": [] }),
                                OpKind::Mutation,
                            )
                            .await
                        {
                            Ok(v) => {
                                self.crash_if("after_create_review")?;
                                let rid = v["addPullRequestReview"]["pullRequestReview"]["id"]
                                    .as_str()
                                    .unwrap_or_default();
                                self.adopt(id, rid, true, &[])?;
                                Ok(Step::Continue)
                            }
                            Err(e) => self.mutation_failed(id, "create", e),
                        }
                    }
                }
            }
            Err(e) => self.mutation_failed(id, "create", e),
        }
    }

    /// Records `review_id` as this draft's pending review and marks drafts
    /// that match `server` comments as staged.
    fn adopt(&self, id: i64, review_id: &str, owned: bool, server: &[ServerComment]) -> Result<()> {
        let now = self.now();
        self.db.write(|tx| {
            let drafts = comments(tx, id)?;
            let claimed: Vec<String> = drafts.iter().filter_map(|d| d.staged.clone()).collect();
            let matches = match_comments(tx, &drafts, server, &claimed)?;
            tx.execute(
                "UPDATE draft_review SET server_pending_review_id = ?2, pending_review_owned = ?3, inflight = NULL,
                   attempts = 0 WHERE id = ?1",
                params![id, review_id, owned],
            )?;
            for (draft, comment) in &matches {
                tx.execute("UPDATE draft_comment SET staged_node_id = ?2 WHERE id = ?1", params![draft, comment])?;
            }
            log(tx, id, &now, "create", "ok", Some(&format!("{review_id}, {} comments", matches.len())))
        })
    }

    /// The last mutation's outcome is unknown: look at GitHub, adopt what
    /// happened, and only then carry on.
    async fn reconcile(&self, id: i64, d: &Row, what: String) -> Result<Step> {
        let now = self.now();
        self.db.write(|tx| log(tx, id, &now, "reconcile", &what, None))?;
        if what == "create" {
            let pending = match self.pending_reviews(&d.pr_node_id).await {
                Ok(p) => p,
                Err(e) => return self.on_error(id, "reconcile", e),
            };
            let seen = d.preflight_seen.clone().unwrap_or_default();
            if let Some(p) = pending.iter().find(|p| **p != seen) {
                let server = match self.server_comments(p).await {
                    Ok(s) => s.unwrap_or_default(),
                    Err(e) => return self.on_error(id, "reconcile", e),
                };
                self.adopt(id, p, true, &server)?;
            } else {
                self.db.write(|tx| {
                    tx.execute("UPDATE draft_review SET inflight = NULL WHERE id = ?1", [id])?;
                    Ok(())
                })?;
            }
            return Ok(Step::Continue);
        }
        if what == "submit" {
            let review = d.pending.clone().unwrap_or_default();
            return match self.review_state(&review).await {
                Ok(Some((state, url))) if state != "PENDING" => {
                    self.mark_submitted(id, &review, url.as_deref())?;
                    Ok(Step::Stop)
                }
                Ok(Some(_)) => {
                    self.db.write(|tx| {
                        tx.execute("UPDATE draft_review SET inflight = NULL WHERE id = ?1", [id])?;
                        Ok(())
                    })?;
                    Ok(Step::Continue)
                }
                Ok(None) => {
                    // The pending review is gone: stage again.
                    self.db.write(|tx| release_pending_review(tx, id))?;
                    self.db.write(|tx| {
                        tx.execute("UPDATE draft_review SET status = 'staging' WHERE id = ?1", [id])?;
                        Ok(())
                    })?;
                    Ok(Step::Continue)
                }
                Err(e) => self.on_error(id, "reconcile", e),
            };
        }
        // A comment: adopt it if GitHub has it.
        let Some(review) = d.pending.clone() else {
            self.db.write(|tx| {
                tx.execute("UPDATE draft_review SET inflight = NULL WHERE id = ?1", [id])?;
                Ok(())
            })?;
            return Ok(Step::Continue);
        };
        let server = match self.server_comments(&review).await {
            Ok(Some(s)) => s,
            Ok(None) => {
                self.db.write(|tx| release_pending_review(tx, id))?;
                return Ok(Step::Continue);
            }
            Err(e) => return self.on_error(id, "reconcile", e),
        };
        self.adopt(id, &review, d.owned, &server)?;
        Ok(Step::Continue)
    }

    async fn review_state(
        &self,
        review: &str,
    ) -> std::result::Result<Option<(String, Option<String>)>, GhError> {
        match self
            .gh
            .graphql::<Value>(
                "ReviewState",
                include_str!("github/graphql/review_state.graphql"),
                json!({ "reviewId": review }),
                OpKind::Query,
            )
            .await
        {
            Ok(v) => Ok(v["node"]["state"]
                .as_str()
                .map(|s| (s.to_string(), v["node"]["url"].as_str().map(str::to_owned)))),
            Err(GhError::NotFound) => Ok(None),
            Err(e) => Err(e),
        }
    }

    // ── Submitting ──

    async fn submit(&self, id: i64) -> Result<Step> {
        let d = self.db.read(|c| row(c, id))?;
        if let Some(what) = &d.inflight {
            return self.reconcile(id, &d, what.clone()).await;
        }
        let Some(review) = d.pending.clone() else {
            self.db.write(|tx| {
                tx.execute("UPDATE draft_review SET status = 'staging' WHERE id = ?1", [id])?;
                Ok(())
            })?;
            return Ok(Step::Continue);
        };
        let body = self.db.read(|c| summary_with_folded_comments(c, id, &d.body))?;
        let event = d.verdict.clone().unwrap_or_else(|| Verdict::Comment.as_str().into());
        self.set_inflight(id, "submit")?;
        let res: std::result::Result<Value, GhError> = self
            .gh
            .graphql(
                "SubmitReview",
                include_str!("github/graphql/submit_review.graphql"),
                json!({ "reviewId": review, "event": event, "body": body }),
                OpKind::Mutation,
            )
            .await;
        match res {
            Ok(v) => {
                self.crash_if("after_submit")?;
                let url =
                    v["submitPullRequestReview"]["pullRequestReview"]["url"].as_str().map(str::to_owned);
                self.mark_submitted(id, &review, url.as_deref())?;
                Ok(Step::Stop)
            }
            Err(GhError::Unprocessable(message)) => {
                // Maybe it already went through (a retry after a lost reply).
                match self.review_state(&review).await {
                    Ok(Some((state, url))) if state != "PENDING" => {
                        self.mark_submitted(id, &review, url.as_deref())?;
                        Ok(Step::Stop)
                    }
                    _ => {
                        self.db.write(|tx| {
                            tx.execute("UPDATE draft_review SET inflight = NULL WHERE id = ?1", [id])?;
                            Ok(())
                        })?;
                        self.needs_attention(id, vec![Reason::CommentRejected { comment: None, message }])
                    }
                }
            }
            Err(GhError::NotFound) => {
                self.db.write(|tx| release_pending_review(tx, id))?;
                self.db.write(|tx| {
                    tx.execute("UPDATE draft_review SET status = 'staging' WHERE id = ?1", [id])?;
                    Ok(())
                })?;
                Ok(Step::Continue)
            }
            Err(e) => self.mutation_failed(id, "submit", e),
        }
    }

    fn mark_submitted(&self, id: i64, review: &str, url: Option<&str>) -> Result<()> {
        let now = self.now();
        self.db.write(|tx| {
            tx.execute(
                "UPDATE draft_review SET status = 'submitted', submitted_at = ?2, submitted_review_node_id = ?3,
                   submitted_url = ?4, inflight = NULL, next_attempt_at = NULL, last_error = NULL,
                   last_error_kind = NULL, updated_at = ?2 WHERE id = ?1",
                params![id, now, review, url],
            )?;
            log(tx, id, &now, "submit", "submitted", url)
        })?;
        self.emit_status(id, "submitted")?;
        Ok(())
    }

    /// Deletes pending reviews we created for drafts that were then edited or
    /// discarded. Never touches one the user started on github.com.
    async fn cleanup_pending_reviews(&self) -> Result<()> {
        let todo: Vec<(i64, String)> = self.db.read(|c| {
            let mut st = c.prepare(
                "SELECT id, cleanup_review_id FROM draft_review WHERE cleanup_review_id IS NOT NULL",
            )?;
            let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })?;
        for (id, review) in todo {
            let res = self
                .gh
                .graphql::<Value>(
                    "DeleteReview",
                    include_str!("github/graphql/delete_review.graphql"),
                    json!({ "reviewId": review }),
                    OpKind::Mutation,
                )
                .await;
            let done = match &res {
                Ok(_) | Err(GhError::NotFound) => true,
                // Already submitted or otherwise not deletable: nothing to do.
                Err(GhError::Unprocessable(_)) => true,
                Err(_) => false,
            };
            if done {
                let now = self.now();
                self.db.write(|tx| {
                    tx.execute("UPDATE draft_review SET cleanup_review_id = NULL WHERE id = ?1", [id])?;
                    log(tx, id, &now, "cleanup", "ok", Some(&review))
                })?;
            } else if let Err(GhError::Offline(_)) = res {
                break;
            }
        }
        Ok(())
    }
}

/// The review body, plus any comments the user chose to fold into it.
fn summary_with_folded_comments(c: &Connection, id: i64, body: &str) -> Result<String> {
    struct Folded {
        path: Option<String>,
        line: Option<i64>,
        text: String,
        head: Option<String>,
    }
    let mut out = body.trim_end().to_string();
    let folded: Vec<Folded> = {
        let mut st = c.prepare(
            "SELECT d.path, d.line, d.body_md, v.head_oid FROM draft_comment d
             LEFT JOIN pr_revision v ON v.id = d.anchor_revision_id
             WHERE d.draft_review_id = ?1 AND d.resolution = 'to_summary' ORDER BY d.id",
        )?;
        st.query_map([id], |r| {
            Ok(Folded { path: r.get(0)?, line: r.get(1)?, text: r.get(2)?, head: r.get(3)? })
        })?
        .collect::<Result<_, _>>()?
    };
    for Folded { path, line, text, head } in folded {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        let at = match (path, line) {
            (Some(p), Some(l)) => format!("`{p}` line {l}"),
            (Some(p), None) => format!("`{p}`"),
            _ => "a comment".into(),
        };
        let on = head.map(|h| format!(" (on {})", &h[..7.min(h.len())])).unwrap_or_default();
        out.push_str(&format!("**On {at}{on}:**\n\n{text}"));
    }
    Ok(out)
}

/// A plain Markdown export of a review, so the text is never lost even if it
/// can't be sent (DESIGN.md §2, edge case 25).
pub fn export_markdown(c: &Connection, id: i64) -> Result<String> {
    let (body, verdict, pr): (String, Option<String>, i64) =
        c.query_row("SELECT body_md, verdict, pr_id FROM draft_review WHERE id = ?1", [id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?;
    let (repo, number, title): (String, i64, String) = c.query_row(
        "SELECT r.owner || '/' || r.name, p.number, p.title FROM pull_request p JOIN repo r ON r.id = p.repo_id
         WHERE p.id = ?1",
        [pr],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let mut out = format!("# Review of {repo}#{number}: {title}\n\n");
    if let Some(v) = verdict {
        out.push_str(&format!("Verdict: {v}\n\n"));
    }
    if !body.trim().is_empty() {
        out.push_str(&format!("{}\n\n", body.trim()));
    }
    for c in comments(c, id)? {
        let at = match (&c.path, c.line, c.kind.as_str()) {
            (_, _, "reply") => format!("Reply in thread on `{}`", c.path.clone().unwrap_or_default()),
            (Some(p), Some(l), _) => format!("`{p}` line {l}"),
            (Some(p), None, _) => format!("`{p}`"),
            _ => "Comment".into(),
        };
        out.push_str(&format!("## {at}\n\n{}\n\n", c.body.trim()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(backoff(1), Duration::from_secs(5));
        assert_eq!(backoff(2), Duration::from_secs(10));
        assert_eq!(backoff(6), Duration::from_secs(160));
        assert_eq!(backoff(7), Duration::from_secs(300));
        assert_eq!(backoff(50), Duration::from_secs(300));
    }

    #[test]
    fn only_some_reasons_can_be_accepted() {
        assert_eq!(
            Reason::PrStateChanged { state: "MERGED".into() }.ack_key().as_deref(),
            Some("pr_state:MERGED")
        );
        assert!(Reason::PrGone.ack_key().is_none());
        assert!(
            Reason::HeadMoved { from_revision: 1, to_revision: 2, comments: vec![], verdict_stale: false }
                .ack_key()
                .is_none()
        );
    }
}

// ─── Resolving "needs attention" ─────────────────────────────────────────

/// What the user decided for one comment (DESIGN.md §10.4).
#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CommentResolution {
    pub id: i64,
    /// `remap` (to `side`/`line` on the current revision), `to_file`,
    /// `to_summary`, `drop`, or `keep` (leave as is).
    pub action: String,
    pub side: Option<crate::diff::Side>,
    pub line: Option<u32>,
    pub start_side: Option<crate::diff::Side>,
    pub start_line: Option<u32>,
    /// For `remap` and `to_file`: the file on the current revision (after a
    /// rename). Defaults to the comment's path.
    pub path: Option<String>,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Resolution {
    /// Ack keys (see `Reason::ack_key`) the user accepted.
    #[serde(default)]
    pub acknowledge: Vec<String>,
    /// `current_head` or `reviewed_commit`.
    pub target_mode: Option<String>,
    #[serde(default)]
    pub comments: Vec<CommentResolution>,
    /// Change the verdict (for example downgrade an approval to a comment).
    #[serde(default, deserialize_with = "crate::commands::double_option")]
    pub verdict: Option<Option<Verdict>>,
}

/// An active review, for the outbox list.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct OutboxItem {
    pub draft_review_id: i64,
    pub pr_id: i64,
    pub repo: String,
    pub number: i64,
    pub title: String,
    pub status: String,
    pub verdict: Option<String>,
    pub comments: i64,
    pub queued_at: Option<String>,
    pub submitted_at: Option<String>,
    pub submitted_url: Option<String>,
    pub last_error: Option<String>,
    pub last_error_kind: Option<String>,
    pub next_attempt_at: Option<String>,
    pub attention: Option<Value>,
}

impl Core {
    /// Queued, in-flight, blocked and recently sent reviews, across PRs.
    pub fn outbox(&self) -> Result<Vec<OutboxItem>> {
        self.db.read(|c| {
            let mut st = c.prepare(
                "SELECT d.id, d.pr_id, r.owner || '/' || r.name, p.number, p.title, d.status, d.verdict,
                        (SELECT count(*) FROM draft_comment x WHERE x.draft_review_id = d.id),
                        d.queued_at, d.submitted_at, d.submitted_url, d.last_error, d.last_error_kind,
                        d.next_attempt_at, d.attention
                 FROM draft_review d JOIN pull_request p ON p.id = d.pr_id JOIN repo r ON r.id = p.repo_id
                 WHERE d.status NOT IN ('draft', 'discarded')
                 ORDER BY CASE d.status WHEN 'submitted' THEN 1 ELSE 0 END, d.queued_at DESC
                 LIMIT 50",
            )?;
            let rows = st
                .query_map([], |r| {
                    let attention: Option<String> = r.get(14)?;
                    Ok(OutboxItem {
                        draft_review_id: r.get(0)?,
                        pr_id: r.get(1)?,
                        repo: r.get(2)?,
                        number: r.get(3)?,
                        title: r.get(4)?,
                        status: r.get(5)?,
                        verdict: r.get(6)?,
                        comments: r.get(7)?,
                        queued_at: r.get(8)?,
                        submitted_at: r.get(9)?,
                        submitted_url: r.get(10)?,
                        last_error: r.get(11)?,
                        last_error_kind: r.get(12)?,
                        next_attempt_at: r.get(13)?,
                        attention: attention.and_then(|a| serde_json::from_str(&a).ok()),
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    pub fn export_review_markdown(&self, pr_id: i64) -> Result<String> {
        self.db.read(|c| {
            let id = crate::drafts::active_draft_id(c, pr_id)?
                .ok_or_else(|| Error::Invalid("There's no review to export.".into()))?;
            export_markdown(c, id)
        })
    }

    fn current_revision(&self, pr_id: i64) -> Result<i64> {
        self.db.read(|c| {
            c.query_row("SELECT current_revision_id FROM pull_request WHERE id = ?1", [pr_id], |r| {
                r.get::<_, Option<i64>>(0)
            })?
            .ok_or_else(|| Error::Invalid("This pull request hasn't been synced.".into()))
        })
    }

    /// Computes and stores where each comment could go in `to_revision`
    /// (shown to the user in the "PR changed" prompt).
    pub(crate) fn store_proposals(&self, comments: &[i64], to_revision: i64) -> Result<Vec<(i64, Proposal)>> {
        let mut out = Vec::new();
        for &cid in comments {
            if let Some(p) = crate::remap::propose_for_comment(&self.db, &self.blobs, cid, to_revision)? {
                out.push((cid, p));
            }
        }
        self.db.write(|tx| {
            for (cid, p) in &out {
                let status = serde_json::to_value(p.status)?.as_str().unwrap_or("orphaned").to_string();
                tx.execute(
                    "UPDATE draft_comment SET remap_status = ?2, remap_proposal = ?3 WHERE id = ?1",
                    params![cid, status, serde_json::to_string(p)?],
                )?;
            }
            Ok(())
        })?;
        Ok(out)
    }

    /// Validates the remaps and file conversions in `resolutions` against the
    /// current revision. A remap without a line takes the stored proposal.
    /// Runs outside any transaction (it reads blobs).
    fn prepare_remaps(
        &self,
        pr_id: i64,
        current: i64,
        resolutions: &[CommentResolution],
    ) -> Result<Vec<PreparedRemap>> {
        let mut remaps = Vec::new();
        for cr in resolutions.iter().filter(|c| c.action == "remap" || c.action == "to_file") {
            let (path, body, kind, proposal): (Option<String>, String, String, Option<String>) =
                self.db.read(|c| {
                    Ok(c.query_row(
                        "SELECT path, body_md, kind, remap_proposal FROM draft_comment WHERE id = ?1",
                        [cr.id],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                    )?)
                })?;
            if kind == "reply" {
                return Err(Error::Invalid(
                    "Replies can't be moved; drop them or fold them into the summary.".into(),
                ));
            }
            let proposal: Option<Proposal> = proposal.and_then(|p| serde_json::from_str(&p).ok());
            let mut cr = cr.clone();
            if cr.action == "remap" && cr.line.is_none() {
                let p = proposal.as_ref().filter(|p| p.line.is_some()).ok_or_else(|| {
                    Error::Invalid(
                        "There's no proposed line for this comment; pick one or choose another action."
                            .into(),
                    )
                })?;
                cr.side = p.side;
                cr.line = p.line;
                cr.start_side = p.start_side;
                cr.start_line = p.start_line;
                cr.path = cr.path.or(p.path.clone());
            }
            if cr.action == "to_file" && cr.path.is_none() {
                cr.path = proposal.and_then(|p| p.path);
            }
            let n = crate::drafts::NewComment {
                revision_id: current,
                kind: crate::drafts::CommentKind::Thread,
                subject_type: if cr.action == "to_file" {
                    crate::drafts::SubjectType::File
                } else {
                    crate::drafts::SubjectType::Line
                },
                path: cr.path.clone().or(path),
                side: cr.side,
                line: cr.line,
                start_side: cr.start_side,
                start_line: cr.start_line,
                reply_to_thread: None,
                body: if cr.action == "to_file" { crate::drafts::strip_suggestions(&body) } else { body },
            };
            let v = self.db.read(|c| crate::drafts::validate(c, pr_id, &n))?.capture(&self.blobs)?;
            remaps.push(PreparedRemap { id: cr.id, comment: n, validated: v });
        }
        Ok(remaps)
    }

    /// Applies per-comment decisions inside a write transaction.
    fn apply_comment_resolutions(
        tx: &Transaction,
        review_id: i64,
        resolutions: &[CommentResolution],
        remaps: &[PreparedRemap],
        now: &str,
    ) -> Result<()> {
        for cr in resolutions {
            let owned: Option<i64> = tx
                .query_row("SELECT draft_review_id FROM draft_comment WHERE id = ?1", [cr.id], |r| r.get(0))
                .optional()?;
            if owned != Some(review_id) {
                return Err(Error::Invalid(format!("comment {} isn't part of this review", cr.id)));
            }
            let resolution = match cr.action.as_str() {
                "drop" | "to_summary" => Some(cr.action.as_str()),
                "keep" | "remap" | "to_file" => None,
                other => return Err(Error::Invalid(format!("unknown action {other}"))),
            };
            tx.execute(
                "UPDATE draft_comment SET resolution = ?2, staged_node_id = NULL, staged_thread_node_id = NULL,
                   remap_status = 'ok', remap_proposal = NULL, updated_at = ?3 WHERE id = ?1",
                params![cr.id, resolution, now],
            )?;
        }
        for r in remaps {
            let (n, v) = (&r.comment, &r.validated);
            let file = n.subject_type == crate::drafts::SubjectType::File;
            tx.execute(
                "UPDATE draft_comment SET path = ?2, subject_type = ?3, side = ?4, line = ?5, start_side = ?6,
                   start_line = ?7, anchor_revision_id = ?8, anchor_snapshot = ?9, body_md = ?10,
                   resolution = NULL, updated_at = ?11 WHERE id = ?1",
                params![
                    r.id,
                    n.path,
                    if file { "FILE" } else { "LINE" },
                    if file { None } else { n.side.map(|s| s.as_str()) },
                    if file { None } else { n.line },
                    v.start.map(|s| s.0.as_str()),
                    v.start.map(|s| s.1),
                    n.revision_id,
                    v.anchor.as_ref().map(serde_json::to_string).transpose()?,
                    n.body,
                    now,
                ],
            )?;
        }
        Ok(())
    }

    /// Applies the user's decisions for a review that needs attention, and
    /// queues it again. Preflight re-checks everything before sending.
    pub fn resolve_review(&self, pr_id: i64, res: Resolution) -> Result<crate::drafts::DraftView> {
        let now = self.now();
        let current = self.current_revision(pr_id)?;
        let remaps = self.prepare_remaps(pr_id, current, &res.comments)?;
        let id = self.db.write(|tx| {
            let id = crate::drafts::active_draft_id(tx, pr_id)?
                .ok_or_else(|| Error::Invalid("There's no review to resolve.".into()))?;
            let status: String = tx.query_row("SELECT status FROM draft_review WHERE id = ?1", [id], |r| r.get(0))?;
            if status != "needs_attention" {
                return Err(Error::Invalid("This review doesn't need attention.".into()));
            }
            let mut a = attention(tx, id)?;
            for k in &res.acknowledge {
                if !a.acknowledged.contains(k) {
                    a.acknowledged.push(k.clone());
                }
            }
            a.reasons.clear();
            if let Some(mode) = &res.target_mode {
                if mode != "current_head" && mode != "reviewed_commit" {
                    return Err(Error::Invalid(format!("unknown target {mode}")));
                }
                tx.execute("UPDATE draft_review SET target_mode = ?2 WHERE id = ?1", params![id, mode])?;
            }
            if let Some(v) = res.verdict {
                tx.execute("UPDATE draft_review SET verdict = ?2 WHERE id = ?1", params![id, v.map(Verdict::as_str)])?;
            }
            let mode: String = tx.query_row("SELECT target_mode FROM draft_review WHERE id = ?1", [id], |r| r.get(0))?;
            if mode == "current_head" {
                // The user has seen what moved: the review now targets the
                // current revision (preflight asks again if it moves again).
                tx.execute("UPDATE draft_review SET target_revision_id = ?2 WHERE id = ?1", params![id, current])?;
            }
            Self::apply_comment_resolutions(tx, id, &res.comments, &remaps, &now)?;
            // Comments changed, so anything already staged must be rebuilt.
            if !res.comments.is_empty() {
                release_pending_review(tx, id)?;
            }
            tx.execute(
                "UPDATE draft_review SET status = 'queued', attention = ?2, attempts = 0, next_attempt_at = NULL,
                   last_error = NULL, last_error_kind = NULL, updated_at = ?3 WHERE id = ?1",
                params![id, serde_json::to_string(&a)?, now],
            )?;
            log(tx, id, &now, "resolve", "queued", None)?;
            Ok(id)
        })?;
        self.emit(Event::OutboxChanged { pr_id, draft_review_id: id, status: "queued".into() });
        self.kick_outbox();
        self.db.read(|c| crate::drafts::draft_view(c, id))
    }

    /// For a draft still being written: where its comments from earlier
    /// versions of the PR could go now. Stored on the comments too.
    pub fn draft_proposals(&self, pr_id: i64) -> Result<Vec<(i64, Proposal)>> {
        let current = self.current_revision(pr_id)?;
        let comments: Vec<i64> = self.db.read(|c| {
            let Some(id) = crate::drafts::active_draft_id(c, pr_id)? else { return Ok(vec![]) };
            let mut st = c.prepare(
                "SELECT id FROM draft_comment WHERE draft_review_id = ?1 AND kind = 'thread'
                   AND (resolution IS NULL OR resolution = 'remap') AND anchor_revision_id IS NOT ?2",
            )?;
            let ids = st.query_map(params![id, current], |r| r.get(0))?.collect::<Result<Vec<_>, _>>()?;
            Ok(ids)
        })?;
        self.store_proposals(&comments, current)
    }

    /// Moves a draft onto the PR's current version: applies the decisions
    /// for its comments from earlier versions, and makes the current
    /// version the review's target.
    pub fn rebase_draft(
        &self,
        pr_id: i64,
        comments: Vec<CommentResolution>,
    ) -> Result<crate::drafts::DraftView> {
        let now = self.now();
        let current = self.current_revision(pr_id)?;
        let remaps = self.prepare_remaps(pr_id, current, &comments)?;
        let id = self.db.write(|tx| {
            let id = crate::drafts::active_draft_id(tx, pr_id)?
                .ok_or_else(|| Error::Invalid("There's no draft review.".into()))?;
            crate::drafts::require_editable(tx, id)?;
            Self::apply_comment_resolutions(tx, id, &comments, &remaps, &now)?;
            let cursor: Option<String> = tx.query_row(
                "SELECT max(submitted_at) FROM review WHERE pr_id = ?1 AND state != 'PENDING'",
                [pr_id],
                |r| r.get(0),
            )?;
            tx.execute(
                "UPDATE draft_review SET target_revision_id = ?2, basis_revision_id = ?2, basis_review_cursor = ?3,
                   updated_at = ?4 WHERE id = ?1",
                params![id, current, cursor, now],
            )?;
            log(tx, id, &now, "rebase", "ok", None)?;
            Ok(id)
        })?;
        self.db.read(|c| crate::drafts::draft_view(c, id))
    }
}

/// A remap or file conversion, checked against the current revision.
struct PreparedRemap {
    id: i64,
    comment: crate::drafts::NewComment,
    validated: crate::drafts::Validated,
}

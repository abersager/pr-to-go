//! The outbox: queued reviews on their way to GitHub (DESIGN.md §10).

use rusqlite::{OptionalExtension, Transaction, params};

use crate::error::Result;
use crate::service::Core;

/// Appends to the review's submission log (kept for debugging and support).
pub(crate) fn log(
    tx: &Transaction,
    review_id: i64,
    now: &str,
    step: &str,
    outcome: &str,
    detail: Option<&str>,
) -> Result<()> {
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
               preflight_pending_seen = NULL WHERE id = ?1",
            [review_id],
        )?;
        tx.execute(
            "UPDATE draft_comment SET staged_node_id = NULL, staged_thread_node_id = NULL WHERE draft_review_id = ?1",
            [review_id],
        )?;
    }
    Ok(())
}

impl Core {
    /// Wakes the outbox worker (after queueing, or when coming online).
    pub fn kick_outbox(&self) {
        self.outbox_wake.notify_one();
    }
}

//! Re-polling CI checks that are still running. Finishing checks don't
//! change a PR's `updatedAt`, so the inbox index never notices them; instead
//! PRs whose snapshot says PENDING get a small checks-only query, backing off
//! from 30 s to 5 min, at most 20 times per head (Hubtty's schedule).

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use rusqlite::params;
use serde_json::json;

use crate::clock::rfc3339;
use crate::github::queries::{NodeData, PR_CHECKS, PrChecksNode};
use crate::github::{GhError, OpKind};
use crate::service::{Core, Event};
use crate::sync::checks_json;
use crate::{Error, Result};

const FIRST_POLL: Duration = Duration::from_secs(30);
const MAX_INTERVAL: Duration = Duration::from_secs(300);
const MAX_POLLS: u32 = 20;
/// How often to look for newly pending PRs when nothing is scheduled.
const IDLE: Duration = Duration::from_secs(60);

/// Per-revision poll schedule. Kept in memory: after a restart the next full
/// sync or this poller simply starts over.
#[derive(Default)]
pub struct CheckPoller {
    due: HashMap<i64, (u32, SystemTime)>,
}

struct Pending {
    pr_id: i64,
    node_id: String,
    revision_id: i64,
    head_oid: String,
}

impl Core {
    /// One pass: polls every pending PR that is due and returns how long to
    /// wait before the next pass.
    pub async fn poll_pending_checks(&self, poller: &mut CheckPoller) -> Result<Duration> {
        let pending: Vec<Pending> = self.db.read(|c| {
            let mut st = c.prepare(
                "SELECT p.id, p.node_id, v.id, v.head_oid FROM pull_request p
                 JOIN pr_revision v ON v.id = p.current_revision_id
                 JOIN check_snapshot s ON s.revision_id = v.id
                 LEFT JOIN pr_local l ON l.pr_id = p.id
                 WHERE (p.in_inbox = 1 OR COALESCE(l.pinned, 0) = 1) AND p.state = 'OPEN'
                   AND s.rollup_state IN ('PENDING', 'EXPECTED')",
            )?;
            let rows = st.query_map([], |r| {
                Ok(Pending {
                    pr_id: r.get(0)?,
                    node_id: r.get(1)?,
                    revision_id: r.get(2)?,
                    head_oid: r.get(3)?,
                })
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })?;
        let now = self.clock.now();
        poller.due.retain(|rev, _| pending.iter().any(|p| p.revision_id == *rev));
        let mut next = IDLE;
        for p in pending {
            let (polls, due) = *poller.due.entry(p.revision_id).or_insert((0, now + FIRST_POLL));
            if polls >= MAX_POLLS {
                continue;
            }
            if due > now {
                next = next.min(due.duration_since(now).unwrap_or_default());
                continue;
            }
            match self.refresh_checks(&p).await {
                Ok(()) => {}
                // Not a failed poll: try again when we're back.
                Err(Error::GitHub(GhError::Offline(_))) => return Ok(IDLE),
                Err(e) => tracing::info!("checks for PR {}: {e}", p.pr_id),
            }
            let interval = (FIRST_POLL * 2u32.pow(polls.min(4))).min(MAX_INTERVAL);
            poller.due.insert(p.revision_id, (polls + 1, now + interval));
            next = next.min(interval);
        }
        Ok(next)
    }

    /// Fetches the head's checks and stores them, or marks the PR stale if
    /// the head moved (the inbox sync then fetches the new version).
    async fn refresh_checks(&self, p: &Pending) -> Result<()> {
        let res = self
            .gh
            .graphql::<NodeData<PrChecksNode>>(
                "PrChecks",
                PR_CHECKS,
                json!({ "id": p.node_id }),
                OpKind::Query,
            )
            .await;
        self.observe(&res);
        let node = res?.node;
        let commit = node.as_ref().and_then(|n| n.last_commit.nodes.first()).map(|c| &c.commit);
        let now = rfc3339(self.clock.now());
        match commit {
            Some(c) if c.oid == p.head_oid => {
                let rollup = c.status_check_rollup.as_ref();
                let (state, contexts) = (rollup.map(|r| r.state.clone()), checks_json(rollup));
                let changed = self.db.write(|tx| {
                    let changed = tx.execute(
                        "UPDATE check_snapshot SET rollup_state = ?2, contexts = ?3
                         WHERE revision_id = ?1 AND (rollup_state IS NOT ?2 OR contexts IS NOT ?3)",
                        params![p.revision_id, state, contexts],
                    )? > 0;
                    tx.execute(
                        "UPDATE check_snapshot SET captured_at = ?2 WHERE revision_id = ?1",
                        params![p.revision_id, now],
                    )?;
                    Ok(changed)
                })?;
                if changed {
                    self.emit(Event::PrUpdated { pr_id: p.pr_id, head_moved: false });
                }
            }
            _ => {
                self.db.write(|tx| {
                    tx.execute(
                        "UPDATE pull_request SET sync_state = 'stale'
                         WHERE id = ?1 AND sync_state IN ('ready', 'partial')",
                        [p.pr_id],
                    )?;
                    Ok(())
                })?;
                self.kick_inbox();
            }
        }
        Ok(())
    }

    /// The checks part of the background worker.
    pub(crate) async fn checks_loop(&self) {
        let mut poller = CheckPoller::default();
        loop {
            let wait = if self.gh.has_token() && !self.gh.is_work_offline() {
                self.poll_pending_checks(&mut poller).await.unwrap_or_else(|e| {
                    tracing::error!("checks: {e}");
                    IDLE
                })
            } else {
                IDLE
            };
            tokio::time::sleep(wait.max(Duration::from_secs(1))).await;
        }
    }
}

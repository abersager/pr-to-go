//! The inbox: subscriptions (repos and searches), a cheap index poll, deep
//! sync of what changed, "pack for the trip", and garbage collection
//! (DESIGN.md §9).

use std::path::Path;
use std::time::Duration;

use futures_util::StreamExt;
use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::clock::{parse_rfc3339, rfc3339};
use crate::error::{Error, Result};
use crate::github::{GhError, OpKind};
use crate::service::{Core, Event};
use crate::sync::PrRef;

const SEARCH_PRS: &str = include_str!("github/graphql/search_prs.graphql");
const REPO_PRS: &str = include_str!("github/graphql/repo_prs.graphql");

/// GitHub's search returns at most this many results.
const SEARCH_CAP: i64 = 1000;
/// Deep syncs running at once.
const SYNC_CONCURRENCY: usize = 3;
/// PRs out of the inbox for this long, with nothing of the user's attached,
/// are removed.
pub const RETENTION: Duration = Duration::from_secs(14 * 24 * 3600);

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Subscription {
    pub id: i64,
    pub kind: String,
    pub repo: Option<String>,
    pub query: Option<String>,
    pub label: String,
    pub enabled: bool,
    pub last_polled_at: Option<String>,
    pub last_error: Option<String>,
    pub prs: i64,
}

/// Ready-made searches for the settings screen.
pub const PRESETS: &[(&str, &str)] = &[
    ("Review requested from me", "is:open is:pr review-requested:@me"),
    ("My open pull requests", "is:open is:pr author:@me"),
    ("Assigned to me", "is:open is:pr assignee:@me"),
];

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct IndexPr {
    id: String,
    number: i64,
    title: String,
    url: String,
    state: String,
    is_draft: bool,
    created_at: String,
    updated_at: String,
    head_ref_oid: String,
    base_ref_name: String,
    head_ref_name: String,
    author: Option<crate::github::queries::Login>,
    repository: IndexRepo,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct IndexRepo {
    id: String,
    name: String,
    owner: crate::github::queries::Login,
    is_private: bool,
    is_archived: bool,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct InboxSync {
    pub polled: usize,
    pub synced: usize,
    pub failed: usize,
    pub errors: Vec<String>,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Readiness {
    /// PRs in the inbox or added by hand.
    pub total: i64,
    pub ready: i64,
    pub partial: i64,
    pub not_synced: i64,
    pub failed: i64,
    pub bytes: u64,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct GcReport {
    pub revisions: usize,
    pub blobs: usize,
    pub prs: usize,
    pub assets: usize,
}

fn dir_size(p: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(p) else { return 0 };
    rd.flatten()
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => dir_size(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

fn upsert_index_pr(tx: &Transaction, pr: &IndexPr) -> Result<i64> {
    let r = &pr.repository;
    tx.execute(
        "INSERT INTO repo (node_id, owner, name, is_private, is_archived) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (node_id) DO UPDATE SET owner = excluded.owner, name = excluded.name,
           is_private = excluded.is_private, is_archived = excluded.is_archived",
        params![r.id, r.owner.login, r.name, r.is_private, r.is_archived],
    )?;
    let repo_id: i64 = tx.query_row("SELECT id FROM repo WHERE node_id = ?1", [&r.id], |x| x.get(0))?;
    // `updated_at` belongs to the deep sync: comparing it with the index's
    // value is how a synced PR is found to be stale.
    tx.execute(
        "INSERT INTO pull_request (node_id, repo_id, number, title, author_login, state, is_draft, base_ref_name,
           head_ref_name, url, created_at, updated_at, sync_state, index_head_oid, index_updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 'indexed', ?13, ?12)
         ON CONFLICT (node_id) DO UPDATE SET title = excluded.title, state = excluded.state,
           is_draft = excluded.is_draft, index_head_oid = excluded.index_head_oid,
           index_updated_at = excluded.index_updated_at,
           sync_state = CASE
             WHEN pull_request.sync_state IN ('ready', 'partial') AND pull_request.updated_at != excluded.index_updated_at
             THEN 'stale' ELSE pull_request.sync_state END",
        params![
            pr.id,
            repo_id,
            pr.number,
            pr.title,
            pr.author.as_ref().map(|a| &a.login),
            pr.state,
            pr.is_draft,
            pr.base_ref_name,
            pr.head_ref_name,
            pr.url,
            pr.created_at,
            pr.updated_at,
            pr.head_ref_oid,
        ],
    )?;
    Ok(tx.query_row("SELECT id FROM pull_request WHERE node_id = ?1", [&pr.id], |x| x.get(0))?)
}

impl Core {
    pub fn subscriptions(&self) -> Result<Vec<Subscription>> {
        self.db.read(|c| {
            let mut st = c.prepare(
                "SELECT s.id, s.kind, s.repo_full_name, s.query, s.label, s.enabled, s.last_polled_at, s.last_error,
                        (SELECT count(*) FROM subscription_member m WHERE m.subscription_id = s.id)
                 FROM subscription s ORDER BY s.id",
            )?;
            let rows = st
                .query_map([], |r| {
                    Ok(Subscription {
                        id: r.get(0)?,
                        kind: r.get(1)?,
                        repo: r.get(2)?,
                        query: r.get(3)?,
                        label: r.get(4)?,
                        enabled: r.get(5)?,
                        last_polled_at: r.get(6)?,
                        last_error: r.get(7)?,
                        prs: r.get(8)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    /// Adds a repo (`owner/name`) or a search. Searches get `is:pr` added.
    pub fn add_subscription(&self, kind: &str, value: &str, label: Option<&str>) -> Result<Subscription> {
        let value = value.trim();
        let now = self.now();
        let (repo, query, default_label) = match kind {
            "repo" => {
                let v = value.trim_start_matches("https://github.com/").trim_end_matches('/');
                let parts: Vec<&str> = v.split('/').collect();
                if parts.len() != 2 || parts.iter().any(|p| p.is_empty()) {
                    return Err(Error::Invalid(format!("\"{value}\" isn't a repository (owner/name).")));
                }
                (Some(v.to_string()), None, v.to_string())
            }
            "search" => {
                if value.is_empty() {
                    return Err(Error::Invalid("The search is empty.".into()));
                }
                let q = if value.split_whitespace().any(|t| t == "is:pr" || t == "type:pr") {
                    value.to_string()
                } else {
                    format!("{value} is:pr")
                };
                (None, Some(q.clone()), q)
            }
            _ => return Err(Error::Invalid(format!("unknown subscription kind {kind}"))),
        };
        let id = self.db.write(|tx| {
            let dup: Option<i64> = tx
                .query_row(
                    "SELECT id FROM subscription WHERE repo_full_name IS ?1 AND query IS ?2",
                    params![repo, query],
                    |r| r.get(0),
                )
                .optional()?;
            if dup.is_some() {
                return Err(Error::Invalid("You're already following that.".into()));
            }
            tx.execute(
                "INSERT INTO subscription (kind, repo_full_name, query, label, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![kind, repo, query, label.map(str::to_owned).unwrap_or(default_label), now],
            )?;
            Ok(tx.last_insert_rowid())
        })?;
        self.kick_inbox();
        self.subscriptions()?
            .into_iter()
            .find(|s| s.id == id)
            .ok_or_else(|| Error::Internal("subscription vanished".into()))
    }

    pub fn remove_subscription(&self, id: i64) -> Result<()> {
        let now = self.now();
        self.db.write(|tx| {
            tx.execute("DELETE FROM subscription WHERE id = ?1", [id])?;
            recompute_inbox(tx, &now)
        })?;
        self.emit(Event::InboxChanged);
        Ok(())
    }

    /// Wakes the inbox loop (after a subscription change, or "Sync all").
    pub fn kick_inbox(&self) {
        self.inbox_wake.notify_one();
    }

    /// Polls every enabled subscription's index (cheap: a few pages of PR
    /// ids and timestamps), updating inbox membership and marking what's
    /// stale.
    pub async fn poll_subscriptions(&self) -> Result<InboxSync> {
        let subs: Vec<Subscription> = self.subscriptions()?.into_iter().filter(|s| s.enabled).collect();
        let mut out = InboxSync::default();
        for s in subs {
            let res = match s.kind.as_str() {
                "search" => self.poll_search(&s).await,
                _ => self.poll_repo(&s).await,
            };
            let now = self.now();
            let (warning, failed) = match &res {
                Ok(w) => (w.clone(), false),
                Err(e) => (Some(e.to_string()), true),
            };
            self.db.write(|tx| {
                tx.execute(
                    "UPDATE subscription SET last_polled_at = ?2, last_error = ?3 WHERE id = ?1",
                    params![s.id, now, warning],
                )?;
                Ok(())
            })?;
            match res {
                Ok(_) => out.polled += 1,
                Err(Error::GitHub(e)) if e.is_transient() => {
                    self.observe::<()>(&Err(e.clone()));
                    return Err(Error::GitHub(e));
                }
                Err(_) if failed => out.errors.push(format!("{}: {}", s.label, warning.unwrap_or_default())),
                Err(_) => {}
            }
        }
        let now = self.now();
        self.db.write(|tx| recompute_inbox(tx, &now))?;
        self.emit(Event::InboxChanged);
        Ok(out)
    }

    async fn poll_search(&self, s: &Subscription) -> Result<Option<String>> {
        let q = s.query.clone().unwrap_or_default();
        let mut after: Option<String> = None;
        let mut members = Vec::new();
        let mut count = 0i64;
        loop {
            let v: Value = self
                .gh
                .graphql("SearchPrs", SEARCH_PRS, json!({ "q": q, "after": after }), OpKind::Query)
                .await?;
            let search = &v["search"];
            count = search["issueCount"].as_i64().unwrap_or(count);
            let nodes: Vec<Value> = search["nodes"].as_array().cloned().unwrap_or_default();
            let prs: Vec<IndexPr> = nodes
                .into_iter()
                // Search can return issues; our fragment only fills PRs.
                .filter(|n| n.get("id").is_some())
                .map(serde_json::from_value)
                .collect::<Result<_, _>>()
                .map_err(|e| GhError::Protocol(format!("SearchPrs: {e}")))?;
            let ids =
                self.db.write(|tx| prs.iter().map(|p| upsert_index_pr(tx, p)).collect::<Result<Vec<_>>>())?;
            members.extend(ids);
            if search["pageInfo"]["hasNextPage"].as_bool() != Some(true) || members.len() as i64 >= SEARCH_CAP
            {
                break;
            }
            after = search["pageInfo"]["endCursor"].as_str().map(str::to_owned);
        }
        // Search results are complete (up to the cap): replace membership.
        self.db.write(|tx| {
            tx.execute("DELETE FROM subscription_member WHERE subscription_id = ?1", [s.id])?;
            for m in &members {
                tx.execute(
                    "INSERT OR IGNORE INTO subscription_member (subscription_id, pr_id) VALUES (?1, ?2)",
                    params![s.id, m],
                )?;
            }
            Ok(())
        })?;
        Ok((count > members.len() as i64).then(|| {
            format!(
                "GitHub returned only the first {} of {count} results; narrow the search to see the rest.",
                members.len()
            )
        }))
    }

    async fn poll_repo(&self, s: &Subscription) -> Result<Option<String>> {
        let full = s.repo.clone().unwrap_or_default();
        let (owner, name) = full.split_once('/').ok_or_else(|| Error::Invalid(format!("bad repo {full}")))?;
        let cursor: Option<String> = self.db.read(|c| {
            Ok(c.query_row("SELECT cursor_updated_at FROM subscription WHERE id = ?1", [s.id], |r| r.get(0))?)
        })?;
        let mut after: Option<String> = None;
        let mut newest: Option<String> = None;
        'pages: loop {
            let v: Value = self
                .gh
                .graphql(
                    "RepoPrs",
                    REPO_PRS,
                    json!({ "owner": owner, "name": name, "after": after }),
                    OpKind::Query,
                )
                .await?;
            let conn = &v["repository"]["pullRequests"];
            if v["repository"].is_null() {
                return Err(Error::GitHub(GhError::NotFound));
            }
            let prs: Vec<IndexPr> = serde_json::from_value(conn["nodes"].clone())
                .map_err(|e| GhError::Protocol(format!("RepoPrs: {e}")))?;
            for p in &prs {
                // Newest first: stop at what the last poll already saw
                // (server timestamps compared with server timestamps).
                if cursor.as_ref().is_some_and(|c| p.updated_at.as_str() <= c.as_str()) {
                    break 'pages;
                }
                if newest.is_none() {
                    newest = Some(p.updated_at.clone());
                }
                // On the first poll, only open PRs are interesting.
                if cursor.is_none() && p.state != "OPEN" {
                    continue;
                }
                self.db.write(|tx| {
                    let id = upsert_index_pr(tx, p)?;
                    if p.state == "OPEN" {
                        tx.execute(
                            "INSERT OR IGNORE INTO subscription_member (subscription_id, pr_id) VALUES (?1, ?2)",
                            params![s.id, id],
                        )?;
                    } else {
                        tx.execute("DELETE FROM subscription_member WHERE subscription_id = ?1 AND pr_id = ?2", params![s.id, id])?;
                    }
                    Ok(())
                })?;
            }
            if conn["pageInfo"]["hasNextPage"].as_bool() != Some(true) {
                break;
            }
            after = conn["pageInfo"]["endCursor"].as_str().map(str::to_owned);
        }
        if let Some(n) = newest {
            self.db.write(|tx| {
                tx.execute("UPDATE subscription SET cursor_updated_at = ?2 WHERE id = ?1", params![s.id, n])?;
                Ok(())
            })?;
        }
        Ok(None)
    }

    /// Polls subscriptions, then deep-syncs every inbox PR that's new or
    /// stale (PRs with drafts first). This is also "pack for the trip".
    pub async fn sync_inbox(&self) -> Result<InboxSync> {
        let mut out = self.poll_subscriptions().await?;
        let todo: Vec<(i64, PrRef)> = self.db.read(|c| {
            let mut st = c.prepare(
                "SELECT p.id, r.owner, r.name, p.number FROM pull_request p JOIN repo r ON r.id = p.repo_id
                 LEFT JOIN pr_local l ON l.pr_id = p.id
                 WHERE (p.in_inbox = 1 OR COALESCE(l.pinned, 0) = 1)
                   AND (p.current_revision_id IS NULL OR p.sync_state IN ('indexed', 'stale', 'error'))
                 ORDER BY EXISTS (SELECT 1 FROM draft_review d WHERE d.pr_id = p.id
                                    AND d.status NOT IN ('submitted', 'discarded')) DESC,
                          p.index_updated_at DESC",
            )?;
            let rows = st
                .query_map([], |r| {
                    Ok((r.get(0)?, PrRef { owner: r.get(1)?, name: r.get(2)?, number: r.get(3)? }))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })?;
        let total = todo.len();
        let done = std::sync::atomic::AtomicUsize::new(0);
        let results: Vec<(String, Result<()>)> = futures_util::stream::iter(todo)
            .map(|(id, r)| {
                let done = &done;
                async move {
                    let res = self.sync_ref(&r, Some(id)).await.map(|_| ());
                    let n = done.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                    self.emit(Event::SyncProgress { done: n, total });
                    (r.display(), res)
                }
            })
            .buffer_unordered(SYNC_CONCURRENCY)
            .collect()
            .await;
        for (label, r) in results {
            match r {
                Ok(()) => out.synced += 1,
                Err(e) => {
                    out.failed += 1;
                    out.errors.push(format!("{label}: {e}"));
                }
            }
        }
        let _ = self.gc();
        self.db.set_setting("inbox_synced_at", &self.now())?;
        Ok(out)
    }

    /// How much of the inbox is available offline.
    pub fn readiness(&self) -> Result<Readiness> {
        let mut r = self.db.read(|c| {
            Ok(c.query_row(
                "SELECT count(*),
                        count(*) FILTER (WHERE p.sync_state = 'ready'),
                        count(*) FILTER (WHERE p.sync_state = 'partial'),
                        count(*) FILTER (WHERE p.current_revision_id IS NULL),
                        count(*) FILTER (WHERE p.sync_state = 'error')
                 FROM pull_request p LEFT JOIN pr_local l ON l.pr_id = p.id
                 WHERE p.in_inbox = 1 OR COALESCE(l.pinned, 0) = 1",
                [],
                |r| {
                    Ok(Readiness {
                        total: r.get(0)?,
                        ready: r.get(1)?,
                        partial: r.get(2)?,
                        not_synced: r.get(3)?,
                        failed: r.get(4)?,
                        bytes: 0,
                    })
                },
            )?)
        })?;
        r.bytes = self.storage_bytes();
        Ok(r)
    }

    pub fn storage_bytes(&self) -> u64 {
        let file = |n: &str| std::fs::metadata(self.data_dir.join(n)).map(|m| m.len()).unwrap_or(0);
        file("prtogo.sqlite")
            + file("prtogo.sqlite-wal")
            + dir_size(&self.data_dir.join("blobs"))
            + dir_size(&self.data_dir.join("assets"))
    }

    /// Removes what nothing needs any more: revisions that aren't current,
    /// last viewed or used by a draft; file contents only they used; PRs out
    /// of the inbox for `RETENTION` with nothing of the user's attached; and
    /// unused images. Drafts and outbox items are never touched.
    pub fn gc(&self) -> Result<GcReport> {
        let cutoff = rfc3339(self.clock.now() - RETENTION);
        let mut report = GcReport::default();
        self.db.write(|tx| {
            let prs: Vec<i64> = {
                let mut st = tx.prepare(
                    "SELECT p.id FROM pull_request p
                     WHERE p.in_inbox = 0 AND p.left_inbox_at IS NOT NULL AND p.left_inbox_at < ?1
                       AND NOT EXISTS (SELECT 1 FROM draft_review d WHERE d.pr_id = p.id)
                       AND NOT EXISTS (SELECT 1 FROM pr_local l WHERE l.pr_id = p.id AND (l.pinned = 1 OR l.starred = 1))",
                )?;
                st.query_map([&cutoff], |r| r.get(0))?.collect::<Result<_, _>>()?
            };
            for id in &prs {
                // Viewing state for a PR that's going away.
                tx.execute("DELETE FROM pr_local WHERE pr_id = ?1", [id])?;
                tx.execute("DELETE FROM file_viewed WHERE pr_id = ?1", [id])?;
                tx.execute("UPDATE pull_request SET current_revision_id = NULL WHERE id = ?1", [id])?;
                tx.execute("DELETE FROM pull_request WHERE id = ?1", [id])?;
            }
            report.prs = prs.len();
            report.revisions = tx.execute(
                "DELETE FROM pr_revision WHERE id NOT IN (
                   SELECT current_revision_id FROM pull_request WHERE current_revision_id IS NOT NULL
                   UNION SELECT last_viewed_revision_id FROM pr_local WHERE last_viewed_revision_id IS NOT NULL
                   UNION SELECT basis_revision_id FROM draft_review
                   UNION SELECT target_revision_id FROM draft_review WHERE target_revision_id IS NOT NULL
                   UNION SELECT anchor_revision_id FROM draft_comment WHERE anchor_revision_id IS NOT NULL)",
                [],
            )?;
            Ok(())
        })?;
        report.blobs = self.blobs.gc()?;
        let unused: Vec<String> = self.db.read(|c| {
            let mut st = c.prepare("SELECT sha256 FROM asset a WHERE NOT EXISTS (SELECT 1 FROM pr_asset p WHERE p.sha256 = a.sha256)")?;
            let rows = st.query_map([], |r| r.get(0))?.collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })?;
        for sha in &unused {
            self.db.write(|tx| {
                tx.execute("DELETE FROM asset WHERE sha256 = ?1", [sha])?;
                Ok(())
            })?;
            let _ = std::fs::remove_file(self.data_dir.join("assets").join(sha));
        }
        report.assets = unused.len();
        Ok(report)
    }

    /// The inbox half of the background worker: syncs every `every`, or
    /// when kicked.
    pub(crate) async fn inbox_loop(&self, every: Duration) {
        loop {
            let has_subs = self.subscriptions().map(|s| s.iter().any(|x| x.enabled)).unwrap_or(false);
            if has_subs
                && self.gh.has_token()
                && !self.gh.is_work_offline()
                && let Err(e) = self.sync_inbox().await
            {
                tracing::info!("inbox sync: {e}");
            }
            let last = self.db.get_setting("inbox_synced_at").ok().flatten().and_then(|t| parse_rfc3339(&t));
            let wait = last
                .and_then(|l| (l + every).duration_since(self.clock.now()).ok())
                .unwrap_or(every)
                .max(Duration::from_secs(30));
            tokio::select! {
                _ = self.inbox_wake.notified() => {}
                _ = tokio::time::sleep(wait) => {}
            }
        }
    }
}

/// Recomputes `in_inbox` from subscription membership and records when a PR
/// left.
fn recompute_inbox(tx: &Transaction, now: &str) -> Result<()> {
    tx.execute(
        "UPDATE pull_request SET in_inbox = 0, left_inbox_at = ?1, sync_state = CASE
           WHEN sync_state IN ('ready', 'partial', 'stale') THEN 'dormant' ELSE sync_state END
         WHERE in_inbox = 1 AND id NOT IN (SELECT pr_id FROM subscription_member)",
        [now],
    )?;
    tx.execute(
        "UPDATE pull_request SET in_inbox = 1, left_inbox_at = NULL,
           sync_state = CASE WHEN sync_state = 'dormant' THEN 'stale' ELSE sync_state END
         WHERE in_inbox = 0 AND id IN (SELECT pr_id FROM subscription_member)",
        [],
    )?;
    Ok(())
}

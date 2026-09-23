//! The core's public API: one `Core` per app, used by the Tauri shell (and a
//! future iPad app). Commands are async methods; changes are broadcast as
//! [`Event`]s.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use tokio::sync::broadcast;

use crate::auth::SecretStore;
use crate::blobstore::BlobStore;
use crate::clock::{Clock, SystemClock, rfc3339};
use crate::db::Db;
use crate::error::{Error, Result};
use crate::github::{GhError, GitHub, GitHubConfig};
use crate::sync::{PrRef, SyncCtx, SyncOutcome, sync_pr};
use crate::views;

#[derive(Serialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Event {
    #[serde(rename_all = "camelCase")]
    PrUpdated {
        pr_id: i64,
        head_moved: bool,
    },
    #[serde(rename_all = "camelCase")]
    SyncStarted {
        pr_id: Option<i64>,
        label: String,
    },
    #[serde(rename_all = "camelCase")]
    SyncFailed {
        pr_id: Option<i64>,
        label: String,
        message: String,
    },
    #[serde(rename_all = "camelCase")]
    Connectivity {
        online: bool,
        work_offline: bool,
        detail: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    OutboxChanged {
        pr_id: i64,
        draft_review_id: i64,
        status: String,
    },
    InboxChanged,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AuthStatus {
    pub signed_in: bool,
    pub login: Option<String>,
    pub source: Option<String>,
    pub scopes: Option<Vec<String>>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Connectivity {
    pub online: bool,
    pub work_offline: bool,
    pub detail: Option<String>,
    pub rate_remaining: Option<u64>,
}

pub struct CoreOptions {
    pub data_dir: PathBuf,
    pub github: GitHubConfig,
    pub secrets: Arc<dyn SecretStore>,
    pub clock: Arc<dyn Clock>,
}

impl CoreOptions {
    pub fn new(data_dir: PathBuf, secrets: Arc<dyn SecretStore>) -> Self {
        CoreOptions { data_dir, github: GitHubConfig::default(), secrets, clock: Arc::new(SystemClock) }
    }
}

pub struct Core {
    pub(crate) db: Arc<Db>,
    pub(crate) gh: Arc<GitHub>,
    pub(crate) blobs: BlobStore,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) secrets: Arc<dyn SecretStore>,
    pub(crate) events: broadcast::Sender<Event>,
    pub(crate) data_dir: PathBuf,
    online: Mutex<Option<bool>>,
    syncing: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl Core {
    pub fn open(opts: CoreOptions) -> Result<Arc<Core>> {
        std::fs::create_dir_all(&opts.data_dir)?;
        let db = Arc::new(Db::open(&opts.data_dir.join("prtogo.sqlite"))?);
        let gh = Arc::new(GitHub::new(opts.github, Some(db.clone())));
        gh.set_token(opts.secrets.get()?);
        gh.set_work_offline(db.get_setting("work_offline")?.as_deref() == Some("1"));
        let blobs = BlobStore::new(db.clone(), opts.data_dir.join("blobs"));
        let (events, _) = broadcast::channel(256);
        Ok(Arc::new(Core {
            db,
            gh,
            blobs,
            clock: opts.clock,
            secrets: opts.secrets,
            events,
            data_dir: opts.data_dir,
            online: Mutex::new(None),
            syncing: Mutex::new(HashMap::new()),
        }))
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    pub(crate) fn emit(&self, e: Event) {
        let _ = self.events.send(e);
    }

    pub(crate) fn now(&self) -> String {
        rfc3339(self.clock.now())
    }

    pub fn db(&self) -> &Db {
        &self.db
    }

    pub fn github(&self) -> &GitHub {
        &self.gh
    }

    pub(crate) fn sync_ctx(&self) -> SyncCtx {
        SyncCtx {
            db: self.db.clone(),
            gh: self.gh.clone(),
            blobs: self.blobs.clone(),
            clock: self.clock.clone(),
            assets_dir: self.data_dir.join("assets"),
        }
    }

    // ─── Auth ────────────────────────────────────────────────────────────

    pub fn auth_status(&self) -> Result<AuthStatus> {
        let row: Option<(String, String, Option<String>)> = self.db.read(|c| {
            Ok(c.query_row("SELECT login, token_source, scopes FROM account LIMIT 1", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .optional()?)
        })?;
        let signed_in = self.gh.has_token() && row.is_some();
        Ok(AuthStatus {
            signed_in,
            login: row.as_ref().map(|r| r.0.clone()),
            source: row.as_ref().map(|r| r.1.clone()),
            scopes: row.and_then(|r| r.2).and_then(|s| serde_json::from_str(&s).ok()),
        })
    }

    /// Verifies `token` with GitHub, then stores it in the keychain.
    pub async fn sign_in(&self, token: &str, source: &str) -> Result<AuthStatus> {
        let token = token.trim();
        if token.is_empty() {
            return Err(Error::Invalid("The token is empty.".into()));
        }
        let previous = self.secrets.get()?;
        self.gh.set_token(Some(token.to_string()));
        let viewer = match self.gh.viewer().await {
            Ok(v) => v,
            Err(e) => {
                self.gh.set_token(previous);
                return Err(match e {
                    GhError::Unauthorized => Error::Invalid("GitHub rejected this token.".into()),
                    other => Error::GitHub(other),
                });
            }
        };
        if let Some(scopes) = &viewer.scopes {
            // Classic tokens report scopes; fine-grained ones don't.
            if !scopes.iter().any(|s| s == "repo" || s == "public_repo") {
                self.gh.set_token(previous);
                return Err(Error::Invalid(format!(
                    "This token can't read or review pull requests. It needs the \"repo\" scope (it has: {}).",
                    if scopes.is_empty() { "none".into() } else { scopes.join(", ") }
                )));
            }
        }
        self.secrets.set(token)?;
        let now = self.now();
        let switched = self.db.write(|tx| {
            let old: Option<String> =
                tx.query_row("SELECT node_id FROM account LIMIT 1", [], |r| r.get(0)).optional()?;
            tx.execute("DELETE FROM account", [])?;
            tx.execute(
                "INSERT INTO account (node_id, login, token_source, scopes, verified_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![viewer.node_id, viewer.login, source, viewer.scopes.as_ref().map(|s| serde_json::to_string(s).unwrap()), now],
            )?;
            Ok(old.is_some_and(|o| o != viewer.node_id))
        })?;
        if switched {
            self.gh.clear_etags();
        }
        self.set_online(Some(true), None);
        self.auth_status()
    }

    pub async fn sign_in_with_gh(&self) -> Result<AuthStatus> {
        let token = tokio::task::spawn_blocking(crate::auth::gh_cli_token)
            .await
            .map_err(|e| Error::Internal(e.to_string()))??;
        self.sign_in(&token, "gh").await
    }

    pub fn sign_out(&self) -> Result<()> {
        self.secrets.clear()?;
        self.gh.set_token(None);
        self.gh.clear_etags();
        self.db.write(|tx| {
            tx.execute("DELETE FROM account", [])?;
            Ok(())
        })
    }

    // ─── Connectivity ────────────────────────────────────────────────────

    fn set_online(&self, online: Option<bool>, detail: Option<String>) {
        let changed = {
            let mut o = self.online.lock().unwrap();
            let changed = *o != online;
            *o = online;
            changed
        };
        if changed || detail.is_some() {
            self.emit(Event::Connectivity {
                online: online.unwrap_or(false),
                work_offline: self.gh.is_work_offline(),
                detail,
            });
        }
    }

    /// Records the connectivity implied by a request's outcome.
    pub(crate) fn observe<T>(&self, res: &std::result::Result<T, GhError>) {
        match res {
            Ok(_) => self.set_online(Some(true), None),
            Err(GhError::Offline(m)) => self.set_online(Some(false), Some(m.clone())),
            Err(_) => {}
        }
    }

    pub async fn check_connectivity(&self) -> Connectivity {
        let res = self.gh.probe().await;
        self.observe(&res);
        let detail = res.as_ref().err().map(|e| e.to_string());
        Connectivity {
            online: res.is_ok(),
            work_offline: self.gh.is_work_offline(),
            detail,
            rate_remaining: res.ok().and_then(|b| b.remaining),
        }
    }

    pub fn connectivity(&self) -> Connectivity {
        Connectivity {
            online: self.online.lock().unwrap().unwrap_or(false) && !self.gh.is_work_offline(),
            work_offline: self.gh.is_work_offline(),
            detail: None,
            rate_remaining: self.gh.budget().remaining,
        }
    }

    pub fn set_work_offline(&self, offline: bool) -> Result<()> {
        self.db.set_setting("work_offline", if offline { "1" } else { "0" })?;
        self.gh.set_work_offline(offline);
        let online = if offline { Some(false) } else { None };
        self.set_online(online, None);
        self.emit(Event::Connectivity { online: false, work_offline: offline, detail: None });
        Ok(())
    }

    // ─── PRs ─────────────────────────────────────────────────────────────

    /// Adds a PR by URL or `owner/repo#n`, syncs it, and pins it so it's kept.
    pub async fn add_pr(&self, input: &str) -> Result<i64> {
        let pr = PrRef::parse(input).ok_or_else(|| {
            Error::Invalid(format!("\"{input}\" isn't a pull request URL or owner/repo#number."))
        })?;
        let outcome = self.sync_ref(&pr, None).await?;
        self.db.write(|tx| {
            tx.execute(
                "INSERT INTO pr_local (pr_id, pinned) VALUES (?1, 1) ON CONFLICT (pr_id) DO UPDATE SET pinned = 1",
                [outcome.pr_id],
            )?;
            Ok(())
        })?;
        self.emit(Event::InboxChanged);
        Ok(outcome.pr_id)
    }

    pub fn pr_ref(&self, pr_id: i64) -> Result<PrRef> {
        self.db.read(|c| {
            c.query_row(
                "SELECT r.owner, r.name, p.number FROM pull_request p JOIN repo r ON r.id = p.repo_id WHERE p.id = ?1",
                [pr_id],
                |r| Ok(PrRef { owner: r.get(0)?, name: r.get(1)?, number: r.get(2)? }),
            )
            .optional()?
            .ok_or_else(|| Error::NotFound(format!("pull request {pr_id}")))
        })
    }

    pub async fn sync_pr(&self, pr_id: i64) -> Result<SyncOutcome> {
        let r = self.pr_ref(pr_id)?;
        self.sync_ref(&r, Some(pr_id)).await
    }

    /// Syncs one PR. Concurrent syncs of the same PR are serialised.
    pub(crate) async fn sync_ref(&self, r: &PrRef, pr_id: Option<i64>) -> Result<SyncOutcome> {
        let key = r.display();
        let lock = self.syncing.lock().unwrap().entry(key.clone()).or_default().clone();
        let _guard = lock.lock().await;
        self.emit(Event::SyncStarted { pr_id, label: key.clone() });
        if let Some(id) = pr_id {
            let _ = self.db.write(|tx| {
                tx.execute("UPDATE pull_request SET sync_state = 'fetching' WHERE id = ?1 AND sync_state != 'fetching'", [id])?;
                Ok(())
            });
        }
        let res = sync_pr(&self.sync_ctx(), r).await;
        match &res {
            Ok(o) => {
                self.set_online(Some(true), None);
                self.emit(Event::PrUpdated { pr_id: o.pr_id, head_moved: o.head_moved });
            }
            Err(e) => {
                if let Error::GitHub(g) = e {
                    self.observe::<()>(&Err(g.clone()));
                }
                if let Some(id) = pr_id {
                    // Keep showing the last complete revision; just note the error.
                    let msg = e.to_string();
                    let transient = matches!(e, Error::GitHub(g) if g.is_transient());
                    let _ = self.db.write(|tx| {
                        tx.execute(
                            "UPDATE pull_request SET sync_state = CASE
                               WHEN current_revision_id IS NULL THEN ?3
                               WHEN ?4 THEN 'stale' ELSE 'error' END,
                             sync_error = ?2 WHERE id = ?1",
                            params![id, msg, if transient { "indexed" } else { "error" }, transient],
                        )?;
                        Ok(())
                    });
                }
                self.emit(Event::SyncFailed { pr_id, label: key, message: e.to_string() });
            }
        }
        res
    }

    pub fn list_prs(&self) -> Result<Vec<views::PrSummary>> {
        views::list_prs(&self.db)
    }

    pub fn get_pr(&self, pr_id: i64) -> Result<views::PrDetail> {
        views::get_pr(&self.db, pr_id)
    }

    pub fn file_diff(&self, revision_id: i64, path: &str) -> Result<views::FileDiff> {
        views::file_diff(&self.db, &self.blobs, revision_id, path)
    }

    /// Records that the user looked at this revision (for "updated since you
    /// last looked").
    pub fn mark_seen(&self, pr_id: i64) -> Result<()> {
        self.db.write(|tx| {
            tx.execute(
                "INSERT INTO pr_local (pr_id, last_viewed_revision_id)
                 SELECT id, current_revision_id FROM pull_request WHERE id = ?1
                 ON CONFLICT (pr_id) DO UPDATE SET last_viewed_revision_id = excluded.last_viewed_revision_id",
                [pr_id],
            )?;
            Ok(())
        })
    }

    pub fn set_file_viewed(
        &self,
        pr_id: i64,
        path: &str,
        head_blob_oid: Option<&str>,
        viewed: bool,
    ) -> Result<()> {
        self.db.write(|tx| {
            if viewed {
                tx.execute(
                    "INSERT INTO file_viewed (pr_id, path, head_blob_oid) VALUES (?1, ?2, ?3)
                     ON CONFLICT (pr_id, path) DO UPDATE SET head_blob_oid = excluded.head_blob_oid",
                    params![pr_id, path, head_blob_oid],
                )?;
            } else {
                tx.execute("DELETE FROM file_viewed WHERE pr_id = ?1 AND path = ?2", params![pr_id, path])?;
            }
            Ok(())
        })
    }

    /// A cached image, for the UI's `prtg://localhost/asset/<sha256>` URLs.
    pub fn asset(&self, sha256: &str) -> Result<Option<(Vec<u8>, String)>> {
        if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Ok(None);
        }
        let ct: Option<Option<String>> = self.db.read(|c| {
            Ok(c.query_row("SELECT content_type FROM asset WHERE sha256 = ?1", [sha256], |r| r.get(0))
                .optional()?)
        })?;
        let Some(ct) = ct else { return Ok(None) };
        match std::fs::read(self.data_dir.join("assets").join(sha256)) {
            Ok(bytes) => Ok(Some((bytes, ct.unwrap_or_else(|| "application/octet-stream".into())))),
            Err(_) => Ok(None),
        }
    }

    /// File contents by blob id, for image previews (`prtg://localhost/blob/<oid>`).
    pub fn blob(&self, oid: &str) -> Result<Option<Vec<u8>>> {
        if oid.len() != 40 || !oid.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Ok(None);
        }
        self.blobs.get(oid)
    }
}

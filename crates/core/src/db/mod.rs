//! SQLite storage. One writer connection and one reader connection over the
//! same WAL-mode database file.
//!
//! Rules (see docs/DESIGN.md §5 and §7):
//! - Never hold a write transaction across a network call. Fetch first, then
//!   write in a short transaction. `std::sync::MutexGuard` isn't `Send`, so
//!   holding one across an `.await` in a spawned task fails to compile.
//! - `synchronous=FULL`: the outbox saves a server id before its next step, and
//!   WAL's default of NORMAL can lose the last commits on power loss.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior};
use rusqlite_migration::{M, Migrations};

use crate::error::{Error, Result};

thread_local! {
    static IN_READ: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static IN_WRITE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The connection mutexes aren't reentrant: calling `read` inside `read` (or
/// `write` inside `write`) on one thread would deadlock. Fail loudly instead.
struct NestGuard(&'static std::thread::LocalKey<std::cell::Cell<bool>>);

impl NestGuard {
    fn enter(key: &'static std::thread::LocalKey<std::cell::Cell<bool>>, what: &str) -> NestGuard {
        if key.with(|k| k.replace(true)) {
            panic!("{what} called while this thread is already inside {what}; this would deadlock");
        }
        NestGuard(key)
    }
}

impl Drop for NestGuard {
    fn drop(&mut self) {
        self.0.with(|k| k.set(false));
    }
}

pub struct Db {
    writer: Mutex<Connection>,
    reader: Mutex<Connection>,
    path: PathBuf,
}

fn migrations() -> Migrations<'static> {
    Migrations::new(vec![M::up(include_str!("migrations/001_initial.sql"))])
}

fn configure(conn: &Connection) -> rusqlite::Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "FULL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(())
}

impl Db {
    pub fn open(path: &Path) -> Result<Db> {
        let mut writer = Connection::open(path)?;
        configure(&writer)?;
        migrations().to_latest(&mut writer).map_err(|e| Error::Internal(format!("migration failed: {e}")))?;
        let reader = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        reader.pragma_update(None, "foreign_keys", "ON")?;
        reader.busy_timeout(std::time::Duration::from_secs(5))?;
        Ok(Db { writer: Mutex::new(writer), reader: Mutex::new(reader), path: path.to_owned() })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Runs `f` in an IMMEDIATE transaction on the writer connection and
    /// commits if it returns `Ok`.
    pub fn write<T>(&self, f: impl FnOnce(&Transaction) -> Result<T>) -> Result<T> {
        let _nest = NestGuard::enter(&IN_WRITE, "Db::write");
        let mut conn = self.writer.lock().expect("db writer lock poisoned");
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }

    /// Runs `f` on the read-only connection. Sees the latest committed state.
    pub fn read<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let _nest = NestGuard::enter(&IN_READ, "Db::read");
        let conn = self.reader.lock().expect("db reader lock poisoned");
        f(&conn)
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        use rusqlite::OptionalExtension;
        self.read(|c| {
            Ok(c.query_row("SELECT value FROM setting WHERE key = ?1", [key], |r| r.get(0)).optional()?)
        })
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "INSERT INTO setting (key, value) VALUES (?1, ?2)
                 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                [key, value],
            )?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_are_valid() {
        migrations().validate().unwrap();
    }

    #[test]
    fn opens_and_applies_pragmas() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.sqlite")).unwrap();
        db.read(|c| {
            let mode: String = c.pragma_query_value(None, "journal_mode", |r| r.get(0))?;
            assert_eq!(mode, "wal");
            let fk: i64 = c.pragma_query_value(None, "foreign_keys", |r| r.get(0))?;
            assert_eq!(fk, 1);
            Ok(())
        })
        .unwrap();
        db.set_setting("k", "v").unwrap();
        assert_eq!(db.get_setting("k").unwrap().as_deref(), Some("v"));
    }

    /// The safety net from the schema header: a draft pins its revision, so
    /// deleting the revision (or the PR) fails instead of losing the draft.
    #[test]
    fn drafts_pin_revisions_and_prs() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.sqlite")).unwrap();
        db.write(|tx| {
            tx.execute_batch(
                "INSERT INTO repo (id, node_id, owner, name) VALUES (1, 'R', 'o', 'r');
                 INSERT INTO pull_request (id, node_id, repo_id, number, title, state,
                   base_ref_name, head_ref_name, url, created_at, updated_at)
                   VALUES (1, 'P', 1, 1, 't', 'OPEN', 'main', 'f', 'u', 'x', 'x');
                 INSERT INTO pr_revision (id, pr_id, head_oid, base_oid, merge_base_oid,
                   fetched_at, status) VALUES (1, 1, 'h', 'b', 'm', 'x', 'complete');
                 INSERT INTO draft_review (id, pr_id, basis_revision_id, status, created_at,
                   updated_at) VALUES (1, 1, 1, 'draft', 'x', 'x');",
            )?;
            Ok(())
        })
        .unwrap();
        let err = db.write(|tx| {
            tx.execute("DELETE FROM pr_revision WHERE id = 1", [])?;
            Ok(())
        });
        assert!(err.is_err(), "deleting a pinned revision must fail");
        let err = db.write(|tx| {
            tx.execute("DELETE FROM pull_request WHERE id = 1", [])?;
            Ok(())
        });
        assert!(err.is_err(), "deleting a PR with drafts must fail");
        let n: i64 =
            db.read(|c| Ok(c.query_row("SELECT count(*) FROM draft_review", [], |r| r.get(0))?)).unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn one_active_draft_per_pr() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.sqlite")).unwrap();
        let res = db.write(|tx| {
            tx.execute_batch(
                "INSERT INTO repo (id, node_id, owner, name) VALUES (1, 'R', 'o', 'r');
                 INSERT INTO pull_request (id, node_id, repo_id, number, title, state,
                   base_ref_name, head_ref_name, url, created_at, updated_at)
                   VALUES (1, 'P', 1, 1, 't', 'OPEN', 'main', 'f', 'u', 'x', 'x');
                 INSERT INTO pr_revision (id, pr_id, head_oid, base_oid, merge_base_oid,
                   fetched_at, status) VALUES (1, 1, 'h', 'b', 'm', 'x', 'complete');
                 INSERT INTO draft_review (pr_id, basis_revision_id, status, created_at,
                   updated_at) VALUES (1, 1, 'submitted', 'x', 'x');
                 INSERT INTO draft_review (pr_id, basis_revision_id, status, created_at,
                   updated_at) VALUES (1, 1, 'draft', 'x', 'x');",
            )?;
            Ok(())
        });
        assert!(res.is_ok());
        let res = db.write(|tx| {
            tx.execute(
                "INSERT INTO draft_review (pr_id, basis_revision_id, status, created_at,
                   updated_at) VALUES (1, 1, 'queued', 'x', 'x')",
                [],
            )?;
            Ok(())
        });
        assert!(res.is_err());
    }
}

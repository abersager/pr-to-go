//! File contents, keyed by git blob id. Blobs up to `INLINE_MAX` bytes live in
//! SQLite; bigger ones are files under `<data>/blobs/ab/cdef…`. Every blob is
//! checked against its git hash before it's stored, which catches truncated
//! downloads and lossy text decoding (non-UTF-8 files, CRLF normalisation).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use rusqlite::OptionalExtension;
use sha1::{Digest, Sha1};

use crate::clock::rfc3339;
use crate::db::Db;
use crate::error::{Error, Result};

pub const INLINE_MAX: usize = 1024 * 1024;

pub fn git_blob_oid(bytes: &[u8]) -> String {
    let mut h = Sha1::new();
    h.update(format!("blob {}\0", bytes.len()).as_bytes());
    h.update(bytes);
    hex::encode(h.finalize())
}

/// Git's heuristic: a NUL byte in the first 8000 bytes means binary.
pub fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8000).any(|&b| b == 0)
}

#[derive(Clone)]
pub struct BlobStore {
    db: Arc<Db>,
    dir: PathBuf,
}

impl BlobStore {
    pub fn new(db: Arc<Db>, dir: PathBuf) -> BlobStore {
        BlobStore { db, dir }
    }

    fn file_path(&self, oid: &str) -> PathBuf {
        self.dir.join(&oid[..2]).join(&oid[2..])
    }

    pub fn has(&self, oid: &str) -> Result<bool> {
        self.db.read(|c| {
            Ok(c.query_row("SELECT 1 FROM blob WHERE oid = ?1", [oid], |_| Ok(())).optional()?.is_some())
        })
    }

    pub fn is_binary(&self, oid: &str) -> Result<Option<bool>> {
        self.db.read(|c| {
            Ok(c.query_row("SELECT is_binary FROM blob WHERE oid = ?1", [oid], |r| r.get(0)).optional()?)
        })
    }

    /// Stores `bytes` if they hash to `oid`. Returns `Ok(false)` on a hash
    /// mismatch, so the caller can fall back to fetching raw bytes.
    pub fn put(&self, oid: &str, bytes: &[u8]) -> Result<bool> {
        if git_blob_oid(bytes) != oid {
            return Ok(false);
        }
        if self.has(oid)? {
            return Ok(true);
        }
        let is_binary = looks_binary(bytes);
        let now = rfc3339(SystemTime::now());
        if bytes.len() > INLINE_MAX {
            let path = self.file_path(oid);
            std::fs::create_dir_all(path.parent().unwrap())?;
            let tmp = path.with_extension("tmp");
            std::fs::write(&tmp, bytes)?;
            std::fs::rename(&tmp, &path)?;
            self.db.write(|tx| {
                tx.execute(
                    "INSERT OR IGNORE INTO blob (oid, byte_size, is_binary, location, content, fetched_at)
                     VALUES (?1, ?2, ?3, 'file', NULL, ?4)",
                    rusqlite::params![oid, bytes.len() as i64, is_binary, now],
                )?;
                Ok(())
            })?;
        } else {
            self.db.write(|tx| {
                tx.execute(
                    "INSERT OR IGNORE INTO blob (oid, byte_size, is_binary, location, content, fetched_at)
                     VALUES (?1, ?2, ?3, 'inline', ?4, ?5)",
                    rusqlite::params![oid, bytes.len() as i64, is_binary, bytes, now],
                )?;
                Ok(())
            })?;
        }
        Ok(true)
    }

    pub fn get(&self, oid: &str) -> Result<Option<Vec<u8>>> {
        let row: Option<(String, Option<Vec<u8>>)> = self.db.read(|c| {
            Ok(c.query_row("SELECT location, content FROM blob WHERE oid = ?1", [oid], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?)
        })?;
        match row {
            None => Ok(None),
            Some((loc, Some(content))) if loc == "inline" => Ok(Some(content)),
            Some(_) => match std::fs::read(self.file_path(oid)) {
                Ok(b) => Ok(Some(b)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(Error::Io(e)),
            },
        }
    }

    /// Text of a blob, if it's stored and isn't binary.
    pub fn get_text(&self, oid: &str) -> Result<Option<String>> {
        Ok(self.get(oid)?.filter(|b| !looks_binary(b)).map(|b| String::from_utf8_lossy(&b).into_owned()))
    }

    /// Deletes blobs that no revision refers to. Returns how many were removed.
    pub fn gc(&self) -> Result<usize> {
        let orphans: Vec<(String, String)> = self.db.read(|c| {
            let mut st = c.prepare(
                "SELECT oid, location FROM blob b WHERE NOT EXISTS (
                   SELECT 1 FROM revision_file f WHERE f.base_blob_oid = b.oid OR f.head_blob_oid = b.oid)",
            )?;
            let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })?;
        for (oid, loc) in &orphans {
            self.db.write(|tx| {
                tx.execute("DELETE FROM blob WHERE oid = ?1", [oid])?;
                Ok(())
            })?;
            if loc == "file" {
                let _ = std::fs::remove_file(self.file_path(oid));
            }
        }
        Ok(orphans.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_hash_matches_git() {
        // `printf 'hello\n' | git hash-object --stdin`
        assert_eq!(git_blob_oid(b"hello\n"), "ce013625030ba8dba906f756967f9e9ca394464a");
        assert_eq!(git_blob_oid(b""), "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391");
    }

    #[test]
    fn stores_inline_and_on_disk_and_rejects_bad_hashes() {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Db::open(&dir.path().join("t.sqlite")).unwrap());
        let store = BlobStore::new(db, dir.path().join("blobs"));
        let small = b"fn main() {}\n".to_vec();
        let big = vec![b'x'; INLINE_MAX + 10];
        let (so, bo) = (git_blob_oid(&small), git_blob_oid(&big));
        assert!(store.put(&so, &small).unwrap());
        assert!(store.put(&bo, &big).unwrap());
        assert!(!store.put(&so, b"tampered").unwrap());
        assert_eq!(store.get(&so).unwrap().unwrap(), small);
        assert_eq!(store.get(&bo).unwrap().unwrap(), big);
        assert!(dir.path().join("blobs").join(&bo[..2]).join(&bo[2..]).exists());
        assert_eq!(store.gc().unwrap(), 2);
        assert!(store.get(&bo).unwrap().is_none());
    }
}

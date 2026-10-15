//! The index: one SQLite file with the files, their chunks, a `vec0` table of vectors (cosine) and an FTS5 table
//! over chunk text and file names. WAL mode, so the daemon writes while a search reads.
//!
//! Vectors and keywords are two views of the same `chunks` rows: `vec_chunks` and `fts_chunks` use the chunk id as
//! their rowid, and FTS5 is an external-content table over `chunks.text`, kept in step by triggers.

use std::path::{Path, PathBuf};
use std::sync::Once;

use anyhow::{Context, Result, bail};
use dig_core::{ChunkKind, FileKind, Modality};
use rusqlite::{Connection, OptionalExtension, params};

pub const SCHEMA_VERSION: u32 = 1;

static VEC_EXTENSION: Once = Once::new();

/// Registers sqlite-vec with every connection this process opens.
fn register_vec() {
    VEC_EXTENSION.call_once(|| unsafe {
        type Init = unsafe extern "C" fn(
            *mut rusqlite::ffi::sqlite3,
            *mut *mut std::os::raw::c_char,
            *const rusqlite::ffi::sqlite3_api_routines,
        ) -> std::os::raw::c_int;
        let init: Init = std::mem::transmute(sqlite_vec::sqlite3_vec_init as *const ());
        rusqlite::ffi::sqlite3_auto_extension(Some(init));
    });
}

/// A file as the index knows it.
#[derive(Debug, Clone)]
pub struct FileRow {
    pub id: i64,
    pub path: PathBuf,
    pub mtime: i64,
    pub size: i64,
    pub kind: FileKind,
    pub status: FileStatus,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    /// Seen by the crawler, not read yet.
    Pending,
    /// Read and embedded.
    Indexed,
    /// Read, but something failed (`error` says what); tried again when the file changes.
    Failed,
}

impl FileStatus {
    fn parse(s: &str) -> Self {
        match s {
            "indexed" => Self::Indexed,
            "failed" => Self::Failed,
            _ => Self::Pending,
        }
    }
}

/// A chunk to write.
#[derive(Debug, Clone)]
pub struct NewChunk {
    pub kind: ChunkKind,
    pub modality: Modality,
    pub locator: Option<f64>,
    pub locator_end: Option<f64>,
    /// What keyword search sees and what a snippet is cut from. Empty for a plain image.
    pub text: String,
    /// The vector, when the modality has one.
    pub vector: Option<Vec<f32>>,
}

/// A chunk as read back for a result.
#[derive(Debug, Clone)]
pub struct ChunkRow {
    pub id: i64,
    pub file_id: i64,
    pub kind: ChunkKind,
    pub modality: Modality,
    pub locator: Option<f64>,
    pub locator_end: Option<f64>,
    pub text: String,
    pub path: PathBuf,
    pub file_kind: FileKind,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Stats {
    pub files: i64,
    pub indexed: i64,
    pub pending: i64,
    pub failed: i64,
    pub chunks: i64,
    pub vectors: i64,
    pub by_kind: Vec<(String, i64)>,
}

pub struct Store {
    conn: Connection,
}

impl Store {
    /// Opens (or creates) the index at `path`. The parent directory is created.
    pub fn open(path: &Path) -> Result<Self> {
        register_vec();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        Self::setup(conn)
    }

    /// An index in memory, for tests.
    pub fn open_in_memory() -> Result<Self> {
        register_vec();
        Self::setup(Connection::open_in_memory()?)
    }

    fn setup(conn: Connection) -> Result<Self> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;",
        )?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        let dim = dig_core::model::DIM;
        self.conn.execute_batch(&format!(
            "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS roots (
               path TEXT PRIMARY KEY,
               code INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE IF NOT EXISTS files (
               id INTEGER PRIMARY KEY,
               path TEXT NOT NULL UNIQUE,
               mtime INTEGER NOT NULL,
               size INTEGER NOT NULL,
               kind TEXT NOT NULL,
               status TEXT NOT NULL DEFAULT 'pending',
               error TEXT,
               indexed_at INTEGER
             );
             CREATE INDEX IF NOT EXISTS files_status ON files(status, kind);
             CREATE TABLE IF NOT EXISTS chunks (
               id INTEGER PRIMARY KEY,
               file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
               kind TEXT NOT NULL,
               modality TEXT NOT NULL,
               locator REAL,
               locator_end REAL,
               text TEXT NOT NULL DEFAULT ''
             );
             CREATE INDEX IF NOT EXISTS chunks_file ON chunks(file_id);
             CREATE VIRTUAL TABLE IF NOT EXISTS vec_chunks USING vec0(
               embedding float[{dim}] distance_metric=cosine
             );
             CREATE VIRTUAL TABLE IF NOT EXISTS fts_chunks USING fts5(
               text, content='chunks', content_rowid='id',
               tokenize='unicode61 remove_diacritics 2'
             );
             CREATE TRIGGER IF NOT EXISTS chunks_ai AFTER INSERT ON chunks BEGIN
               INSERT INTO fts_chunks(rowid, text) VALUES (new.id, new.text);
             END;
             CREATE TRIGGER IF NOT EXISTS chunks_ad AFTER DELETE ON chunks BEGIN
               INSERT INTO fts_chunks(fts_chunks, rowid, text) VALUES ('delete', old.id, old.text);
               DELETE FROM vec_chunks WHERE rowid = old.id;
             END;
             CREATE TRIGGER IF NOT EXISTS chunks_au AFTER UPDATE OF text ON chunks BEGIN
               INSERT INTO fts_chunks(fts_chunks, rowid, text) VALUES ('delete', old.id, old.text);
               INSERT INTO fts_chunks(rowid, text) VALUES (new.id, new.text);
             END;"
        ))?;
        let version: Option<String> = self.get_meta("schema_version")?;
        match version.as_deref().map(|v| v.parse::<u32>().unwrap_or(0)) {
            None => {
                self.set_meta("schema_version", &SCHEMA_VERSION.to_string())?;
                self.set_meta("model_id", dig_core::model::REPOSITORY)?;
                self.set_meta("model_revision", dig_core::model::REVISION)?;
                self.set_meta("vector_version", &dig_core::model::VECTOR_VERSION.to_string())?;
            }
            Some(v) if v > SCHEMA_VERSION => bail!("this index was written by a newer Dig (schema {v})"),
            Some(_) => {}
        }
        Ok(())
    }

    pub fn get_meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0)).optional()?)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute("INSERT INTO meta(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value", params![key, value])?;
        Ok(())
    }

    /// The vector version this index holds. A mismatch with the running pipeline means every file re-embeds.
    pub fn vector_version(&self) -> Result<u32> {
        Ok(self.get_meta("vector_version")?.and_then(|v| v.parse().ok()).unwrap_or(0))
    }

    // ---- roots ------------------------------------------------------------------------------------------------

    pub fn add_root(&self, path: &Path, code: bool) -> Result<()> {
        self.conn.execute(
            "INSERT INTO roots(path, code) VALUES (?1, ?2) ON CONFLICT(path) DO UPDATE SET code = excluded.code",
            params![path.to_string_lossy(), code as i64],
        )?;
        Ok(())
    }

    pub fn remove_root(&self, path: &Path) -> Result<()> {
        self.conn.execute("DELETE FROM roots WHERE path = ?1", [path.to_string_lossy()])?;
        Ok(())
    }

    /// (path, is code root)
    pub fn roots(&self) -> Result<Vec<(PathBuf, bool)>> {
        let mut stmt = self.conn.prepare("SELECT path, code FROM roots ORDER BY path")?;
        let rows = stmt.query_map([], |r| Ok((PathBuf::from(r.get::<_, String>(0)?), r.get::<_, i64>(1)? != 0)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    // ---- files ------------------------------------------------------------------------------------------------

    /// Records a file the crawler saw. Returns its id and whether it needs (re)indexing: new, or changed in mtime
    /// or size, or failed before and changed since. An unchanged indexed file is left alone.
    pub fn see_file(&self, path: &Path, mtime: i64, size: i64, kind: FileKind) -> Result<(i64, bool)> {
        let path_s = path.to_string_lossy();
        let existing: Option<(i64, i64, i64, String, String)> = self
            .conn
            .query_row("SELECT id, mtime, size, kind, status FROM files WHERE path = ?1", [&path_s], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })
            .optional()?;
        match existing {
            Some((id, old_mtime, old_size, old_kind, status)) => {
                let unchanged = old_mtime == mtime && old_size == size && old_kind == kind.as_str();
                if unchanged && status == "indexed" {
                    return Ok((id, false));
                }
                if unchanged && status == "failed" {
                    return Ok((id, false));
                }
                self.conn.execute(
                    "UPDATE files SET mtime = ?2, size = ?3, kind = ?4, status = 'pending', error = NULL WHERE id = ?1",
                    params![id, mtime, size, kind.as_str()],
                )?;
                Ok((id, true))
            }
            None => {
                self.conn.execute(
                    "INSERT INTO files(path, mtime, size, kind, status) VALUES (?1, ?2, ?3, ?4, 'pending')",
                    params![path_s, mtime, size, kind.as_str()],
                )?;
                Ok((self.conn.last_insert_rowid(), true))
            }
        }
    }

    pub fn file(&self, id: i64) -> Result<Option<FileRow>> {
        Ok(self
            .conn
            .query_row("SELECT id, path, mtime, size, kind, status, error FROM files WHERE id = ?1", [id], Self::file_row)
            .optional()?)
    }

    pub fn file_by_path(&self, path: &Path) -> Result<Option<FileRow>> {
        Ok(self
            .conn
            .query_row("SELECT id, path, mtime, size, kind, status, error FROM files WHERE path = ?1", [path.to_string_lossy()], Self::file_row)
            .optional()?)
    }

    fn file_row(r: &rusqlite::Row) -> rusqlite::Result<FileRow> {
        Ok(FileRow {
            id: r.get(0)?,
            path: PathBuf::from(r.get::<_, String>(1)?),
            mtime: r.get(2)?,
            size: r.get(3)?,
            kind: FileKind::parse(&r.get::<_, String>(4)?).unwrap_or(FileKind::Doc),
            status: FileStatus::parse(&r.get::<_, String>(5)?),
            error: r.get(6)?,
        })
    }

    /// Files waiting to be read, cheap kinds first, then oldest first.
    pub fn pending_files(&self, limit: usize) -> Result<Vec<FileRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, mtime, size, kind, status, error FROM files WHERE status = 'pending'
             ORDER BY CASE kind WHEN 'screenshot' THEN 0 WHEN 'image' THEN 1 WHEN 'doc' THEN 2 WHEN 'pdf' THEN 3
                                WHEN 'audio' THEN 4 WHEN 'video' THEN 5 ELSE 6 END, size, id
             LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], Self::file_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Replaces a file's chunks with `chunks` and marks it indexed, in one transaction.
    pub fn write_chunks(&mut self, file_id: i64, chunks: &[NewChunk]) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM chunks WHERE file_id = ?1", [file_id])?;
        {
            let mut insert = tx.prepare_cached(
                "INSERT INTO chunks(file_id, kind, modality, locator, locator_end, text) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            let mut insert_vec = tx.prepare_cached("INSERT INTO vec_chunks(rowid, embedding) VALUES (?1, ?2)")?;
            for chunk in chunks {
                insert.execute(params![file_id, chunk.kind.as_str(), chunk.modality.as_str(), chunk.locator, chunk.locator_end, chunk.text])?;
                let id = tx.last_insert_rowid();
                if let Some(vector) = &chunk.vector {
                    if vector.len() != dig_core::model::DIM {
                        bail!("vector of {} values, expected {}", vector.len(), dig_core::model::DIM);
                    }
                    insert_vec.execute(params![id, as_bytes(vector)])?;
                }
            }
        }
        tx.execute(
            "UPDATE files SET status = 'indexed', error = NULL, indexed_at = unixepoch() WHERE id = ?1",
            [file_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn mark_failed(&self, file_id: i64, error: &str) -> Result<()> {
        let mut error = error.to_owned();
        error.truncate(500);
        self.conn.execute("UPDATE files SET status = 'failed', error = ?2 WHERE id = ?1", params![file_id, error])?;
        Ok(())
    }

    /// Drops every file under `root` whose path isn't in `keep` (the crawler's current view). Returns how many.
    pub fn remove_missing(&self, root: &Path, keep: &std::collections::HashSet<PathBuf>) -> Result<usize> {
        let prefix = format!("{}/", root.to_string_lossy().trim_end_matches('/'));
        let mut stmt = self.conn.prepare("SELECT id, path FROM files WHERE path LIKE ?1 ESCAPE '\\'")?;
        let like = format!("{}%", prefix.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
        let gone: Vec<i64> = stmt
            .query_map([like], |r| Ok((r.get::<_, i64>(0)?, PathBuf::from(r.get::<_, String>(1)?))))?
            .filter_map(|r| r.ok())
            .filter(|(_, path)| !keep.contains(path))
            .map(|(id, _)| id)
            .collect();
        for id in &gone {
            self.conn.execute("DELETE FROM files WHERE id = ?1", [id])?;
        }
        Ok(gone.len())
    }

    pub fn remove_file(&self, path: &Path) -> Result<bool> {
        Ok(self.conn.execute("DELETE FROM files WHERE path = ?1", [path.to_string_lossy()])? > 0)
    }

    /// Marks every file pending, for a re-embed after a vector version change.
    pub fn reset_all(&self) -> Result<()> {
        self.conn.execute_batch("DELETE FROM chunks; UPDATE files SET status = 'pending', error = NULL;")?;
        Ok(())
    }

    // ---- search -----------------------------------------------------------------------------------------------

    /// The `k` nearest chunks to `vector`, as (chunk id, cosine similarity, modality), best first.
    pub fn knn(&self, vector: &[f32], k: usize) -> Result<Vec<(i64, f32, Modality)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT v.rowid, v.distance, c.modality FROM vec_chunks v JOIN chunks c ON c.id = v.rowid
             WHERE v.embedding MATCH ?1 AND k = ?2 ORDER BY v.distance",
        )?;
        let rows = stmt.query_map(params![as_bytes(vector), k as i64], |r| {
            Ok((r.get::<_, i64>(0)?, 1.0 - r.get::<_, f64>(1)? as f32, r.get::<_, String>(2)?))
        })?;
        Ok(rows
            .filter_map(|r| r.ok())
            .map(|(id, sim, modality)| (id, sim, Modality::parse(&modality).unwrap_or(Modality::Text)))
            .collect())
    }

    /// Keyword hits for the query's words (any of them, BM25 order), as chunk ids, best first.
    pub fn keyword_search(&self, words: &[String], k: usize) -> Result<Vec<i64>> {
        if words.is_empty() {
            return Ok(Vec::new());
        }
        let query = words.iter().map(|w| format!("\"{}\"", w.replace('"', "\"\""))).collect::<Vec<_>>().join(" OR ");
        let mut stmt = self.conn.prepare_cached("SELECT rowid FROM fts_chunks WHERE fts_chunks MATCH ?1 ORDER BY rank LIMIT ?2")?;
        let rows = stmt.query_map(params![query, k as i64], |r| r.get::<_, i64>(0))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// The chunks with these ids, joined with their files, in no particular order.
    pub fn chunks(&self, ids: &[i64]) -> Result<Vec<ChunkRow>> {
        let mut out = Vec::with_capacity(ids.len());
        let mut stmt = self.conn.prepare_cached(
            "SELECT c.id, c.file_id, c.kind, c.modality, c.locator, c.locator_end, c.text, f.path, f.kind
             FROM chunks c JOIN files f ON f.id = c.file_id WHERE c.id = ?1",
        )?;
        for id in ids {
            if let Some(row) = stmt
                .query_row([id], |r| {
                    Ok(ChunkRow {
                        id: r.get(0)?,
                        file_id: r.get(1)?,
                        kind: ChunkKind::parse(&r.get::<_, String>(2)?).unwrap_or(ChunkKind::Passage),
                        modality: Modality::parse(&r.get::<_, String>(3)?).unwrap_or(Modality::None),
                        locator: r.get(4)?,
                        locator_end: r.get(5)?,
                        text: r.get(6)?,
                        path: PathBuf::from(r.get::<_, String>(7)?),
                        file_kind: FileKind::parse(&r.get::<_, String>(8)?).unwrap_or(FileKind::Doc),
                    })
                })
                .optional()?
            {
                out.push(row);
            }
        }
        Ok(out)
    }

    pub fn stats(&self) -> Result<Stats> {
        let mut stats = Stats::default();
        stats.files = self.conn.query_row("SELECT count(*) FROM files", [], |r| r.get(0))?;
        stats.indexed = self.conn.query_row("SELECT count(*) FROM files WHERE status = 'indexed'", [], |r| r.get(0))?;
        stats.pending = self.conn.query_row("SELECT count(*) FROM files WHERE status = 'pending'", [], |r| r.get(0))?;
        stats.failed = self.conn.query_row("SELECT count(*) FROM files WHERE status = 'failed'", [], |r| r.get(0))?;
        stats.chunks = self.conn.query_row("SELECT count(*) FROM chunks", [], |r| r.get(0))?;
        stats.vectors = self.conn.query_row("SELECT count(*) FROM vec_chunks", [], |r| r.get(0))?;
        let mut stmt = self.conn.prepare("SELECT kind, count(*) FROM files GROUP BY kind ORDER BY kind")?;
        stats.by_kind = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
        Ok(stats)
    }
}

/// A float32 vector as the little-endian bytes sqlite-vec reads.
fn as_bytes(vector: &[f32]) -> Vec<u8> {
    vector.iter().flat_map(|v| v.to_le_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(dir: usize) -> Vec<f32> {
        let mut v = vec![0.0; dig_core::model::DIM];
        v[dir] = 1.0;
        v
    }

    #[test]
    fn files_chunks_vectors_and_keywords_round_trip() -> Result<()> {
        let mut store = Store::open_in_memory()?;
        let (id, changed) = store.see_file(Path::new("/tmp/a.txt"), 1, 10, FileKind::Doc)?;
        assert!(changed);
        store.write_chunks(
            id,
            &[
                NewChunk { kind: ChunkKind::Passage, modality: Modality::Text, locator: Some(0.0), locator_end: None, text: "the clause about pets in the lease".into(), vector: Some(unit(0)) },
                NewChunk { kind: ChunkKind::Passage, modality: Modality::Text, locator: Some(100.0), locator_end: None, text: "rent is due on the first".into(), vector: Some(unit(1)) },
            ],
        )?;
        assert_eq!(store.see_file(Path::new("/tmp/a.txt"), 1, 10, FileKind::Doc)?, (id, false));
        assert_eq!(store.see_file(Path::new("/tmp/a.txt"), 2, 10, FileKind::Doc)?, (id, true));

        let near = store.knn(&unit(1), 5)?;
        assert_eq!(near[0].1, 1.0);
        let rows = store.chunks(&[near[0].0])?;
        assert_eq!(rows[0].text, "rent is due on the first");

        let hits = store.keyword_search(&["pets".into(), "lease".into()], 10)?;
        assert_eq!(store.chunks(&hits)?[0].locator, Some(0.0));
        let stats = store.stats()?;
        assert_eq!((stats.files, stats.chunks, stats.vectors), (1, 2, 2));

        // Removing the file takes its chunks, vectors and keywords with it.
        assert!(store.remove_file(Path::new("/tmp/a.txt"))?);
        let stats = store.stats()?;
        assert_eq!((stats.files, stats.chunks, stats.vectors), (0, 0, 0));
        assert!(store.keyword_search(&["pets".into()], 10)?.is_empty());
        Ok(())
    }

    #[test]
    fn missing_files_under_a_root_are_dropped() -> Result<()> {
        let store = Store::open_in_memory()?;
        store.see_file(Path::new("/r/a.jpg"), 1, 1, FileKind::Image)?;
        store.see_file(Path::new("/r/b.jpg"), 1, 1, FileKind::Image)?;
        store.see_file(Path::new("/other/c.jpg"), 1, 1, FileKind::Image)?;
        let keep = [PathBuf::from("/r/a.jpg")].into_iter().collect();
        assert_eq!(store.remove_missing(Path::new("/r"), &keep)?, 1);
        assert_eq!(store.stats()?.files, 2);
        Ok(())
    }
}

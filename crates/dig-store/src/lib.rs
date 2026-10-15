//! The index: one SQLite file with the files, their chunks, their vectors and an FTS5 table over chunk text and file
//! names. WAL mode, so the daemon writes while a search reads.
//!
//! Vectors and keywords are two views of the same `chunks` rows: `vectors` is keyed by chunk id and scanned in
//! memory (`vectors.rs`), and FTS5 is an external-content table over `chunks.text`, kept in step by triggers.

mod vectors;
pub use vectors::{Candidate, RowMeta, VectorIndex, quantize};

use std::path::{Path, PathBuf};
use std::sync::Once;

use anyhow::{Context, Result, bail};
use dig_core::{ChunkKind, FileKind, Modality};
use rusqlite::{Connection, OptionalExtension, params};

pub const SCHEMA_VERSION: u32 = 2;

static VEC_EXTENSION: Once = Once::new();

/// Registers sqlite-vec with every connection this process opens: only to read a schema-1 index's `vec0` table once,
/// when it is migrated.
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
    /// When it was taken (photos with EXIF), else modified.
    pub mtime: i64,
    pub size: i64,
}

/// What a search is limited to. Empty means everything.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Filter {
    pub kinds: Vec<FileKind>,
    /// Absolute folders; a file passes when it is under any of them.
    pub folders: Vec<PathBuf>,
    /// Unix seconds, against the file's modification time.
    pub after: Option<i64>,
    pub before: Option<i64>,
}

impl Filter {
    /// A WHERE clause over the files table aliased `alias`, and its arguments (empty when the filter is empty).
    fn sql(&self, alias: &str) -> (String, Vec<rusqlite::types::Value>) {
        use rusqlite::types::Value;
        let mut parts = Vec::new();
        let mut args = Vec::new();
        if !self.kinds.is_empty() {
            parts.push(format!(
                "{alias}.kind IN ({})",
                vec!["?"; self.kinds.len()].join(",")
            ));
            args.extend(self.kinds.iter().map(|k| Value::Text(k.as_str().into())));
        }
        if !self.folders.is_empty() {
            let ors: Vec<String> = self
                .folders
                .iter()
                .map(|_| format!("{alias}.path LIKE ? ESCAPE '\\'"))
                .collect();
            parts.push(format!("({})", ors.join(" OR ")));
            for folder in &self.folders {
                let prefix = format!("{}/", folder.to_string_lossy().trim_end_matches('/'));
                args.push(Value::Text(format!(
                    "{}%",
                    prefix
                        .replace('\\', "\\\\")
                        .replace('%', "\\%")
                        .replace('_', "\\_")
                )));
            }
        }
        if let Some(after) = self.after {
            parts.push(format!("coalesce({alias}.taken_at, {alias}.mtime) >= ?"));
            args.push(Value::Integer(after));
        }
        if let Some(before) = self.before {
            parts.push(format!("coalesce({alias}.taken_at, {alias}.mtime) < ?"));
            args.push(Value::Integer(before));
        }
        (parts.join(" AND "), args)
    }

    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
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
    /// (kind, indexed, total)
    pub progress: Vec<(String, i64, i64)>,
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
        // Schema 1 kept vectors in a `vec0` table whose delete trigger must go before anything is deleted.
        let from_vec0 = self.conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name = 'vec_chunks'",
            [],
            |r| r.get::<_, i64>(0),
        )? > 0;
        self.conn
            .execute_batch("DROP TRIGGER IF EXISTS chunks_ad;")?;
        self.conn.execute_batch(
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
             CREATE TABLE IF NOT EXISTS vectors (
               seq INTEGER PRIMARY KEY AUTOINCREMENT,
               chunk_id INTEGER NOT NULL UNIQUE,
               file_id INTEGER NOT NULL,
               file_kind INTEGER NOT NULL,
               modality INTEGER NOT NULL,
               scale REAL NOT NULL,
               q BLOB NOT NULL,
               f BLOB NOT NULL
             );
             CREATE TABLE IF NOT EXISTS vectors_gone (
               seq INTEGER PRIMARY KEY AUTOINCREMENT,
               chunk_id INTEGER NOT NULL
             );
             CREATE TRIGGER IF NOT EXISTS vectors_ad AFTER DELETE ON vectors BEGIN
               INSERT INTO vectors_gone(chunk_id) VALUES (old.chunk_id);
             END;
             CREATE TABLE IF NOT EXISTS searches (
               query TEXT PRIMARY KEY,
               last INTEGER NOT NULL,
               count INTEGER NOT NULL DEFAULT 1
             );
             CREATE TABLE IF NOT EXISTS opens (
               file_id INTEGER PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
               last INTEGER NOT NULL,
               count INTEGER NOT NULL DEFAULT 1
             );
             CREATE VIRTUAL TABLE IF NOT EXISTS fts_chunks USING fts5(
               text, content='chunks', content_rowid='id',
               tokenize='unicode61 remove_diacritics 2'
             );
             CREATE TRIGGER IF NOT EXISTS chunks_ai AFTER INSERT ON chunks BEGIN
               INSERT INTO fts_chunks(rowid, text) VALUES (new.id, new.text);
             END;
             CREATE TRIGGER IF NOT EXISTS chunks_ad_v2 AFTER DELETE ON chunks BEGIN
               INSERT INTO fts_chunks(fts_chunks, rowid, text) VALUES ('delete', old.id, old.text);
               DELETE FROM vectors WHERE chunk_id = old.id;
             END;
             CREATE TRIGGER IF NOT EXISTS chunks_au AFTER UPDATE OF text ON chunks BEGIN
               INSERT INTO fts_chunks(fts_chunks, rowid, text) VALUES ('delete', old.id, old.text);
               INSERT INTO fts_chunks(rowid, text) VALUES (new.id, new.text);
             END;
             CREATE INDEX IF NOT EXISTS files_mtime ON files(mtime);
             CREATE INDEX IF NOT EXISTS vectors_file ON vectors(file_id);",
        )?;
        for column in ["taken_at INTEGER", "place TEXT", "name TEXT"] {
            let name = column.split(' ').next().unwrap();
            let has: i64 = self.conn.query_row(
                "SELECT count(*) FROM pragma_table_info('files') WHERE name = ?1",
                [name],
                |r| r.get(0),
            )?;
            if has == 0 {
                self.conn
                    .execute_batch(&format!("ALTER TABLE files ADD COLUMN {column};"))?;
            }
        }
        self.names_index()?;
        if from_vec0 {
            self.migrate_vec0()?;
        }
        let version: Option<String> = self.get_meta("schema_version")?;
        match version.as_deref().map(|v| v.parse::<u32>().unwrap_or(0)) {
            None => {
                self.set_meta("schema_version", &SCHEMA_VERSION.to_string())?;
                self.set_meta("model_id", dig_core::model::REPOSITORY)?;
                self.set_meta("model_revision", dig_core::model::REVISION)?;
                self.set_meta(
                    "vector_version",
                    &dig_core::model::VECTOR_VERSION.to_string(),
                )?;
            }
            Some(v) if v > SCHEMA_VERSION => {
                bail!("this index was written by a newer Dig (schema {v})")
            }
            Some(_) => self.set_meta("schema_version", &SCHEMA_VERSION.to_string())?,
        }
        Ok(())
    }

    /// File names in an FTS5 trigram index: substring matches (half a word as it is being typed) without scanning
    /// every row. The name column is filled here for indexes written before it existed.
    fn names_index(&self) -> Result<()> {
        let existed = self.conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name = 'names_fts'",
            [],
            |r| r.get::<_, i64>(0),
        )? > 0;
        if !existed {
            // Names first, index rebuilt from them, triggers last: a trigger firing on these updates would ask FTS5
            // to delete entries it never had, which corrupts an external-content table.
            let tx = self.conn.unchecked_transaction()?;
            tx.execute_batch("DROP TRIGGER IF EXISTS files_names_au;")?;
            {
                let mut read = tx.prepare("SELECT id, path FROM files WHERE name IS NULL")?;
                let rows: Vec<(i64, String)> = read
                    .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<Result<_, _>>()?;
                let mut write = tx.prepare("UPDATE files SET name = ?2 WHERE id = ?1")?;
                for (id, path) in rows {
                    write.execute(params![id, file_name(Path::new(&path))])?;
                }
            }
            tx.execute_batch(
                "CREATE VIRTUAL TABLE names_fts USING fts5(name, content='files', content_rowid='id', tokenize='trigram');
                 INSERT INTO names_fts(names_fts) VALUES ('rebuild');",
            )?;
            tx.commit()?;
        }
        self.conn.execute_batch(
            "CREATE TRIGGER IF NOT EXISTS files_names_ai AFTER INSERT ON files BEGIN
               INSERT INTO names_fts(rowid, name) VALUES (new.id, new.name);
             END;
             CREATE TRIGGER IF NOT EXISTS files_names_ad AFTER DELETE ON files BEGIN
               INSERT INTO names_fts(names_fts, rowid, name) VALUES ('delete', old.id, old.name);
             END;
             CREATE TRIGGER IF NOT EXISTS files_names_au AFTER UPDATE OF name ON files BEGIN
               INSERT INTO names_fts(names_fts, rowid, name) VALUES ('delete', old.id, old.name);
               INSERT INTO names_fts(rowid, name) VALUES (new.id, new.name);
             END;",
        )?;
        Ok(())
    }

    /// Copies a schema-1 index's vectors out of `vec0` (same floats, so nothing re-embeds) and drops the table.
    fn migrate_vec0(&self) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut read = tx.prepare(
                "SELECT v.rowid, v.embedding, c.file_id, f.kind, c.modality FROM vec_chunks v
                 JOIN chunks c ON c.id = v.rowid JOIN files f ON f.id = c.file_id",
            )?;
            let mut rows = read.query([])?;
            while let Some(r) = rows.next()? {
                let blob = r.get_ref(1)?.as_blob()?;
                let vector: Vec<f32> = blob
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                    .collect();
                let kind = FileKind::parse(&r.get::<_, String>(3)?).unwrap_or(FileKind::Doc);
                let modality = Modality::parse(&r.get::<_, String>(4)?).unwrap_or(Modality::Text);
                insert_vector(&tx, r.get(0)?, r.get(2)?, kind, modality, &vector)?;
            }
        }
        tx.execute_batch("DROP TABLE vec_chunks;")?;
        tx.commit()?;
        Ok(())
    }

    pub fn get_meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute("INSERT INTO meta(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value", params![key, value])?;
        Ok(())
    }

    /// The vector version this index holds. A mismatch with the running pipeline means every file re-embeds.
    pub fn vector_version(&self) -> Result<u32> {
        Ok(self
            .get_meta("vector_version")?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0))
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
        self.conn.execute(
            "DELETE FROM roots WHERE path = ?1",
            [path.to_string_lossy()],
        )?;
        Ok(())
    }

    /// (path, is code root)
    pub fn roots(&self) -> Result<Vec<(PathBuf, bool)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT path, code FROM roots ORDER BY path")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                PathBuf::from(r.get::<_, String>(0)?),
                r.get::<_, i64>(1)? != 0,
            ))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    // ---- files ------------------------------------------------------------------------------------------------

    /// Records a file the crawler saw. Returns its id and whether it needs (re)indexing: new, or changed in mtime
    /// or size, or failed before and changed since. An unchanged indexed file is left alone.
    pub fn see_file(
        &self,
        path: &Path,
        mtime: i64,
        size: i64,
        kind: FileKind,
    ) -> Result<(i64, bool)> {
        let path_s = path.to_string_lossy();
        let existing: Option<(i64, i64, i64, String, String)> = self
            .conn
            .query_row(
                "SELECT id, mtime, size, kind, status FROM files WHERE path = ?1",
                [&path_s],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
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
                    "INSERT INTO files(path, mtime, size, kind, status, name) VALUES (?1, ?2, ?3, ?4, 'pending', ?5)",
                    params![path_s, mtime, size, kind.as_str(), file_name(path)],
                )?;
                Ok((self.conn.last_insert_rowid(), true))
            }
        }
    }

    pub fn file(&self, id: i64) -> Result<Option<FileRow>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, path, mtime, size, kind, status, error FROM files WHERE id = ?1",
                [id],
                Self::file_row,
            )
            .optional()?)
    }

    pub fn file_by_path(&self, path: &Path) -> Result<Option<FileRow>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, path, mtime, size, kind, status, error FROM files WHERE path = ?1",
                [path.to_string_lossy()],
                Self::file_row,
            )
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

    /// Files waiting to be read: the newest month first (what people look for soonest after setup), and within a
    /// month cheap kinds first, small files first.
    pub fn pending_files(&self, limit: usize) -> Result<Vec<FileRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, mtime, size, kind, status, error FROM files WHERE status = 'pending'
             ORDER BY mtime / 2592000 DESC,
                      CASE kind WHEN 'screenshot' THEN 0 WHEN 'image' THEN 1 WHEN 'doc' THEN 2 WHEN 'pdf' THEN 3
                                WHEN 'audio' THEN 4 WHEN 'video' THEN 5 ELSE 6 END, size, id
             LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], Self::file_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Replaces a file's chunks with `chunks` and marks it indexed, in one transaction.
    pub fn write_chunks(&mut self, file_id: i64, chunks: &[NewChunk]) -> Result<()> {
        self.write_many(&[(file_id, chunks)])
    }

    /// `write_chunks` for several files in one transaction.
    pub fn write_many(&mut self, files: &[(i64, &[NewChunk])]) -> Result<()> {
        let tx = self.conn.transaction()?;
        for (file_id, chunks) in files {
            let file_id = *file_id;
            tx.execute("DELETE FROM chunks WHERE file_id = ?1", [file_id])?;
            let kind: String =
                tx.query_row("SELECT kind FROM files WHERE id = ?1", [file_id], |r| {
                    r.get(0)
                })?;
            let kind = FileKind::parse(&kind).unwrap_or(FileKind::Doc);
            let mut insert = tx.prepare_cached(
                "INSERT INTO chunks(file_id, kind, modality, locator, locator_end, text) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for chunk in chunks.iter() {
                insert.execute(params![
                    file_id,
                    chunk.kind.as_str(),
                    chunk.modality.as_str(),
                    chunk.locator,
                    chunk.locator_end,
                    chunk.text
                ])?;
                let id = tx.last_insert_rowid();
                if let Some(vector) = &chunk.vector {
                    if vector.len() != dig_core::model::DIM {
                        bail!(
                            "vector of {} values, expected {}",
                            vector.len(),
                            dig_core::model::DIM
                        );
                    }
                    insert_vector(&tx, id, file_id, kind, chunk.modality, vector)?;
                }
            }
            tx.execute("UPDATE files SET status = 'indexed', error = NULL, indexed_at = unixepoch() WHERE id = ?1", [file_id])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Files that couldn't be read, with why, newest first.
    pub fn failed_files(&self, k: usize) -> Result<Vec<(PathBuf, String)>> {
        let mut stmt = self.conn.prepare("SELECT path, coalesce(error, '') FROM files WHERE status = 'failed' ORDER BY mtime DESC LIMIT ?1")?;
        let rows = stmt.query_map([k as i64], |r| {
            Ok((PathBuf::from(r.get::<_, String>(0)?), r.get(1)?))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// When a photo was taken (EXIF), which date filters and result rows use instead of the modification time.
    pub fn set_taken_at(&self, file_id: i64, taken_at: Option<i64>) -> Result<()> {
        self.conn.execute(
            "UPDATE files SET taken_at = ?2 WHERE id = ?1",
            params![file_id, taken_at],
        )?;
        Ok(())
    }

    pub fn mark_failed(&self, file_id: i64, error: &str) -> Result<()> {
        let mut error = error.to_owned();
        error.truncate(500);
        self.conn.execute(
            "UPDATE files SET status = 'failed', error = ?2 WHERE id = ?1",
            params![file_id, error],
        )?;
        Ok(())
    }

    /// Drops every file under `root` whose path isn't in `keep` (the crawler's current view). Returns how many.
    pub fn remove_missing(
        &self,
        root: &Path,
        keep: &std::collections::HashSet<PathBuf>,
    ) -> Result<usize> {
        let prefix = format!("{}/", root.to_string_lossy().trim_end_matches('/'));
        let mut stmt = self
            .conn
            .prepare("SELECT id, path FROM files WHERE path LIKE ?1 ESCAPE '\\'")?;
        let like = format!(
            "{}%",
            prefix
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        let gone: Vec<i64> = stmt
            .query_map([like], |r| {
                Ok((r.get::<_, i64>(0)?, PathBuf::from(r.get::<_, String>(1)?)))
            })?
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
        Ok(self.conn.execute(
            "DELETE FROM files WHERE path = ?1",
            [path.to_string_lossy()],
        )? > 0)
    }

    /// Marks every file pending, for a re-embed after a vector version change.
    pub fn reset_all(&self) -> Result<()> {
        self.conn.execute_batch("DELETE FROM chunks; UPDATE files SET status = 'pending', error = NULL; DELETE FROM vectors_gone;")?;
        Ok(())
    }

    // ---- search -----------------------------------------------------------------------------------------------

    /// The `k` nearest chunks to `vector`, as (chunk id, cosine similarity, modality), best first. Loads the whole
    /// matrix, so a resident searcher keeps a `VectorIndex` instead; this is for one-off searches and tests.
    pub fn knn(&self, vector: &[f32], k: usize) -> Result<Vec<(i64, f32, Modality)>> {
        let index = VectorIndex::load(self)?;
        let ids: Vec<i64> = index
            .search(vector, k, &|_| true)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        self.rescore(vector, &ids)
    }

    /// Keyword hits for the query's words (any of them, BM25 order), as chunk ids, best first, limited to files that
    /// pass `filter`.
    pub fn keyword_search(&self, words: &[String], k: usize) -> Result<Vec<i64>> {
        self.keyword_search_in(words, k, &Filter::default())
    }

    /// Keyword hits with their files' kinds, for one query serving both the unfiltered list and a kind filter.
    pub fn keyword_hits(
        &self,
        words: &[String],
        k: usize,
        filter: &Filter,
    ) -> Result<Vec<(i64, FileKind)>> {
        if words.is_empty() {
            return Ok(Vec::new());
        }
        let query = words
            .iter()
            .map(|w| format!("\"{}\"", w.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" OR ");
        let (clause, mut args) = filter.sql("f");
        let clause = if clause.is_empty() {
            String::new()
        } else {
            format!("AND {clause}")
        };
        let sql = format!(
            "SELECT x.rowid, f.kind FROM fts_chunks x JOIN chunks c ON c.id = x.rowid JOIN files f ON f.id = c.file_id
             WHERE fts_chunks MATCH ? {clause} ORDER BY x.rank LIMIT ?"
        );
        args.insert(0, rusqlite::types::Value::Text(query));
        args.push(rusqlite::types::Value::Integer(k as i64));
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(args), |r| {
            Ok((
                r.get::<_, i64>(0)?,
                FileKind::parse(&r.get::<_, String>(1)?).unwrap_or(FileKind::Doc),
            ))
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn keyword_search_in(
        &self,
        words: &[String],
        k: usize,
        filter: &Filter,
    ) -> Result<Vec<i64>> {
        if words.is_empty() {
            return Ok(Vec::new());
        }
        let query = words
            .iter()
            .map(|w| format!("\"{}\"", w.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" OR ");
        let (clause, mut args) = filter.sql("f");
        if clause.is_empty() {
            let mut stmt = self.conn.prepare_cached(
                "SELECT rowid FROM fts_chunks WHERE fts_chunks MATCH ?1 ORDER BY rank LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![query, k as i64], |r| r.get::<_, i64>(0))?;
            return Ok(rows.filter_map(|r| r.ok()).collect());
        }
        let sql = format!(
            "SELECT x.rowid FROM fts_chunks x JOIN chunks c ON c.id = x.rowid JOIN files f ON f.id = c.file_id
             WHERE fts_chunks MATCH ? AND {clause} ORDER BY x.rank LIMIT ?"
        );
        args.insert(0, rusqlite::types::Value::Text(query));
        args.push(rusqlite::types::Value::Integer(k as i64));
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(args), |r| r.get::<_, i64>(0))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Files whose name holds every one of `words` (case-insensitive, anywhere in the name), most recently changed
    /// first: what a search shows before the model has answered, and what someone typing half a file name wants.
    /// Words under three characters are left to the other lists (a trigram index can't look them up).
    pub fn names_like(&self, words: &[String], k: usize, filter: &Filter) -> Result<Vec<i64>> {
        let words: Vec<&String> = words.iter().filter(|w| w.chars().count() >= 3).collect();
        if words.is_empty() {
            return Ok(Vec::new());
        }
        let query = words
            .iter()
            .map(|w| format!("\"{}\"", w.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" AND ");
        let (clause, mut args) = filter.sql("f");
        let clause = if clause.is_empty() {
            String::new()
        } else {
            format!("AND {clause}")
        };
        args.insert(0, rusqlite::types::Value::Text(query));
        args.push(rusqlite::types::Value::Integer(k as i64));
        let sql = format!(
            "SELECT c.id FROM names_fts n JOIN files f ON f.id = n.rowid JOIN chunks c ON c.file_id = f.id AND c.kind = 'name'
             WHERE names_fts MATCH ? AND f.status = 'indexed' {clause} ORDER BY f.mtime DESC LIMIT ?"
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(args), |r| r.get::<_, i64>(0))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// The name chunks of the most recently changed files that pass `filter`: a search of filters alone
    /// (`type:pdf last week`).
    pub fn recent_files(&self, filter: &Filter, k: usize) -> Result<Vec<i64>> {
        let (clause, mut args) = filter.sql("f");
        let clause = if clause.is_empty() {
            "1".to_owned()
        } else {
            clause
        };
        args.push(rusqlite::types::Value::Integer(k as i64));
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT c.id FROM files f JOIN chunks c ON c.file_id = f.id AND c.kind = 'name'
             WHERE f.status = 'indexed' AND {clause} ORDER BY coalesce(f.taken_at, f.mtime) DESC LIMIT ?"
        ))?;
        let rows = stmt.query_map(rusqlite::params_from_iter(args), |r| r.get::<_, i64>(0))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// File ids that pass `filter`, when it says anything a vector row can't check by itself (folders, dates).
    pub fn file_ids(&self, filter: &Filter) -> Result<Option<std::collections::HashSet<i64>>> {
        if filter.folders.is_empty() && filter.after.is_none() && filter.before.is_none() {
            return Ok(None);
        }
        let (clause, args) = filter.sql("f");
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT f.id FROM files f WHERE {clause}"))?;
        let rows = stmt.query_map(rusqlite::params_from_iter(args), |r| r.get::<_, i64>(0))?;
        Ok(Some(rows.filter_map(|r| r.ok()).collect()))
    }

    /// The chunks with these ids, joined with their files, in no particular order.
    pub fn chunks(&self, ids: &[i64]) -> Result<Vec<ChunkRow>> {
        let mut out = Vec::with_capacity(ids.len());
        let mut unique: Vec<i64> = ids.to_vec();
        unique.sort_unstable();
        unique.dedup();
        for batch in unique.chunks(500) {
            let marks = vec!["?"; batch.len()].join(",");
            let mut stmt = self.conn.prepare_cached(&format!(
                "SELECT c.id, c.file_id, c.kind, c.modality, c.locator, c.locator_end, c.text, f.path, f.kind, coalesce(f.taken_at, f.mtime), f.size
                 FROM chunks c JOIN files f ON f.id = c.file_id WHERE c.id IN ({marks})"
            ))?;
            let rows = stmt.query_map(rusqlite::params_from_iter(batch.iter()), |r| {
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
                    mtime: r.get(9)?,
                    size: r.get(10)?,
                })
            })?;
            for row in rows {
                out.push(row?);
            }
        }
        Ok(out)
    }

    /// The text of the chunks next to this one in its file (same kind, by locator): (before, after).
    pub fn neighbours(&self, chunk_id: i64) -> Result<(String, String)> {
        let row: Option<(i64, String, Option<f64>)> = self
            .conn
            .query_row(
                "SELECT file_id, kind, locator FROM chunks WHERE id = ?1",
                [chunk_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((file, kind, Some(at))) = row else {
            return Ok(Default::default());
        };
        let side = |op: &str, order: &str| -> Result<String> {
            Ok(self
                .conn
                .query_row(
                    &format!("SELECT text FROM chunks WHERE file_id = ?1 AND kind = ?2 AND locator {op} ?3 ORDER BY locator {order} LIMIT 1"),
                    params![file, kind, at],
                    |r| r.get(0),
                )
                .optional()?
                .unwrap_or_default())
        };
        Ok((side("<", "DESC")?, side(">", "ASC")?))
    }

    // ---- history ----------------------------------------------------------------------------------------------

    pub fn record_search(&self, query: &str) -> Result<()> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(());
        }
        self.conn.execute(
            "INSERT INTO searches(query, last) VALUES (?1, unixepoch()) ON CONFLICT(query) DO UPDATE SET last = unixepoch(), count = count + 1",
            [query],
        )?;
        // A few hundred is plenty for the empty screen and ↑.
        self.conn.execute("DELETE FROM searches WHERE query NOT IN (SELECT query FROM searches ORDER BY last DESC LIMIT 500)", [])?;
        Ok(())
    }

    pub fn record_open(&self, path: &Path) -> Result<()> {
        self.conn.execute(
            "INSERT INTO opens(file_id, last) SELECT id, unixepoch() FROM files WHERE path = ?1
             ON CONFLICT(file_id) DO UPDATE SET last = unixepoch(), count = count + 1",
            [path.to_string_lossy()],
        )?;
        Ok(())
    }

    /// Newest first: (query, unix seconds).
    pub fn recent_searches(&self, k: usize) -> Result<Vec<(String, i64)>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT query, last FROM searches ORDER BY last DESC LIMIT ?1")?;
        let rows = stmt.query_map([k as i64], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Name chunks of the files opened most, weighted toward recent opens (each week since the last open divides the count further).
    pub fn most_opened(&self, k: usize) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT c.id FROM opens o JOIN chunks c ON c.file_id = o.file_id AND c.kind = 'name'
             ORDER BY o.count / (1.0 + (unixepoch() - o.last) / 604800.0) DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([k as i64], |r| r.get::<_, i64>(0))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Name chunks of the files indexed most recently.
    pub fn recently_added(&self, k: usize) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT c.id FROM files f JOIN chunks c ON c.file_id = f.id AND c.kind = 'name'
             WHERE f.status = 'indexed' ORDER BY f.mtime DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([k as i64], |r| r.get::<_, i64>(0))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// How often and how lately each of these files was opened from the window: (count, last).
    pub fn open_stats(
        &self,
        file_ids: &[i64],
    ) -> Result<std::collections::HashMap<i64, (i64, i64)>> {
        let mut out = std::collections::HashMap::new();
        for batch in file_ids.chunks(500) {
            let marks = vec!["?"; batch.len()].join(",");
            let mut stmt = self.conn.prepare_cached(&format!(
                "SELECT file_id, count, last FROM opens WHERE file_id IN ({marks})"
            ))?;
            let rows = stmt.query_map(rusqlite::params_from_iter(batch.iter()), |r| {
                Ok((r.get::<_, i64>(0)?, (r.get(1)?, r.get(2)?)))
            })?;
            for row in rows {
                let (id, v) = row?;
                out.insert(id, v);
            }
        }
        Ok(out)
    }

    /// A chunk's float32 vector, if it has one.
    pub fn vector_of(&self, chunk_id: i64) -> Result<Option<Vec<f32>>> {
        let blob: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT f FROM vectors WHERE chunk_id = ?1",
                [chunk_id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(blob.map(|b| {
            b.as_chunks::<4>()
                .0
                .iter()
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect()
        }))
    }

    /// The best vector of a file, for "more like this" from a row whose chunk has none (a name hit).
    pub fn any_vector_chunk(&self, file_id: i64) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT chunk_id FROM vectors WHERE file_id = ?1 ORDER BY seq LIMIT 1",
                [file_id],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn stats(&self) -> Result<Stats> {
        let (files, indexed, pending, failed): (i64, i64, i64, i64) = self.conn.query_row(
            "SELECT count(*), coalesce(sum(status = 'indexed'), 0), coalesce(sum(status = 'pending'), 0),
                    coalesce(sum(status = 'failed'), 0) FROM files",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
        let chunks = self
            .conn
            .query_row("SELECT count(*) FROM chunks", [], |r| r.get(0))?;
        let vectors = self
            .conn
            .query_row("SELECT count(*) FROM vectors", [], |r| r.get(0))?;
        let mut stmt = self.conn.prepare(
            "SELECT kind, count(*), sum(status != 'pending') FROM files GROUP BY kind ORDER BY kind",
        )?;
        let rows: Vec<(String, i64, i64)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<Result<_, _>>()?;
        Ok(Stats {
            files,
            indexed,
            pending,
            failed,
            chunks,
            vectors,
            by_kind: rows.iter().map(|(k, n, _)| (k.clone(), *n)).collect(),
            progress: rows.into_iter().map(|(k, n, done)| (k, done, n)).collect(),
        })
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// A float32 vector as little-endian bytes.
fn as_bytes(vector: &[f32]) -> Vec<u8> {
    vector.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn insert_vector(
    conn: &Connection,
    chunk_id: i64,
    file_id: i64,
    kind: FileKind,
    modality: Modality,
    vector: &[f32],
) -> Result<()> {
    let (q, scale) = quantize(vector);
    let q: Vec<u8> = q.into_iter().map(|b| b as u8).collect();
    conn.prepare_cached(
        "INSERT INTO vectors(chunk_id, file_id, file_kind, modality, scale, q, f) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(chunk_id) DO NOTHING",
    )?
    .execute(params![chunk_id, file_id, vectors::kind_code(kind) as i64, vectors::modality_code(modality) as i64, scale as f64, q, as_bytes(vector)])?;
    Ok(())
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
                NewChunk {
                    kind: ChunkKind::Passage,
                    modality: Modality::Text,
                    locator: Some(0.0),
                    locator_end: None,
                    text: "the clause about pets in the lease".into(),
                    vector: Some(unit(0)),
                },
                NewChunk {
                    kind: ChunkKind::Passage,
                    modality: Modality::Text,
                    locator: Some(100.0),
                    locator_end: None,
                    text: "rent is due on the first".into(),
                    vector: Some(unit(1)),
                },
            ],
        )?;
        assert_eq!(
            store.see_file(Path::new("/tmp/a.txt"), 1, 10, FileKind::Doc)?,
            (id, false)
        );
        assert_eq!(
            store.see_file(Path::new("/tmp/a.txt"), 2, 10, FileKind::Doc)?,
            (id, true)
        );

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
    fn a_schema_1_index_migrates_without_re_embedding() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("old.sqlite");
        register_vec();
        {
            let conn = Connection::open(&path)?;
            conn.execute_batch(&format!(
                "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 INSERT INTO meta VALUES ('schema_version', '1'), ('vector_version', '1');
                 CREATE TABLE roots (path TEXT PRIMARY KEY, code INTEGER NOT NULL DEFAULT 0);
                 CREATE TABLE files (id INTEGER PRIMARY KEY, path TEXT NOT NULL UNIQUE, mtime INTEGER NOT NULL, size INTEGER NOT NULL,
                   kind TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'pending', error TEXT, indexed_at INTEGER);
                 CREATE TABLE chunks (id INTEGER PRIMARY KEY, file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
                   kind TEXT NOT NULL, modality TEXT NOT NULL, locator REAL, locator_end REAL, text TEXT NOT NULL DEFAULT '');
                 CREATE VIRTUAL TABLE vec_chunks USING vec0(embedding float[{}] distance_metric=cosine);
                 CREATE VIRTUAL TABLE fts_chunks USING fts5(text, content='chunks', content_rowid='id', tokenize='unicode61 remove_diacritics 2');
                 CREATE TRIGGER chunks_ai AFTER INSERT ON chunks BEGIN INSERT INTO fts_chunks(rowid, text) VALUES (new.id, new.text); END;
                 CREATE TRIGGER chunks_ad AFTER DELETE ON chunks BEGIN
                   INSERT INTO fts_chunks(fts_chunks, rowid, text) VALUES ('delete', old.id, old.text);
                   DELETE FROM vec_chunks WHERE rowid = old.id;
                 END;
                 INSERT INTO files VALUES (1, '/p/beach-day.jpg', 1, 1, 'image', 'indexed', NULL, 1);
                 INSERT INTO chunks VALUES (1, 1, 'name', 'none', NULL, NULL, 'beach day');
                 INSERT INTO chunks VALUES (2, 1, 'image', 'image', NULL, NULL, '');",
                dig_core::model::DIM
            ))?;
            conn.execute(
                "INSERT INTO vec_chunks(rowid, embedding) VALUES (2, ?1)",
                [as_bytes(&unit(3))],
            )?;
        }
        let store = Store::open(&path)?;
        assert_eq!(store.get_meta("schema_version")?.as_deref(), Some("2"));
        assert_eq!(store.knn(&unit(3), 5)?[0].0, 2);
        assert_eq!(
            store.names_like(&["beach".into()], 5, &Filter::default())?,
            vec![1]
        );
        store.conn.execute_batch("INSERT INTO names_fts(names_fts) VALUES ('integrity-check'); INSERT INTO fts_chunks(fts_chunks) VALUES ('integrity-check');")?;
        // Removing the file takes its vector with it, and a matrix following the index sees that.
        let mut index = VectorIndex::load(&store)?;
        assert_eq!(index.len(), 1);
        store.remove_file(Path::new("/p/beach-day.jpg"))?;
        index.refresh(&store)?;
        assert_eq!(index.len(), 0);
        Ok(())
    }

    #[test]
    fn filters_narrow_keywords_and_names() -> Result<()> {
        let mut store = Store::open_in_memory()?;
        for (i, (path, kind, mtime)) in [
            ("/home/u/Documents/lease.pdf", FileKind::Pdf, 100),
            ("/home/u/Pictures/lease-signing.jpg", FileKind::Image, 200),
        ]
        .into_iter()
        .enumerate()
        {
            let (id, _) = store.see_file(Path::new(path), mtime, 1, kind)?;
            store.write_chunks(
                id,
                &[NewChunk {
                    kind: ChunkKind::Name,
                    modality: Modality::None,
                    locator: None,
                    locator_end: None,
                    text: format!("lease {i}"),
                    vector: None,
                }],
            )?;
        }
        let words = vec!["lease".to_owned()];
        assert_eq!(
            store
                .keyword_search_in(&words, 10, &Filter::default())?
                .len(),
            2
        );
        let pdfs = Filter {
            kinds: vec![FileKind::Pdf],
            ..Filter::default()
        };
        assert_eq!(store.keyword_search_in(&words, 10, &pdfs)?.len(), 1);
        let docs = Filter {
            folders: vec![PathBuf::from("/home/u/Documents")],
            ..Filter::default()
        };
        assert_eq!(store.names_like(&["leas".into()], 10, &docs)?.len(), 1);
        let recent = Filter {
            after: Some(150),
            ..Filter::default()
        };
        assert_eq!(store.recent_files(&recent, 10)?.len(), 1);
        // Half a word finds the name; newest first.
        assert_eq!(
            store
                .names_like(&["leas".into()], 10, &Filter::default())?
                .len(),
            2
        );
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

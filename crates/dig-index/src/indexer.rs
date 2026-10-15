//! Reading changed files, embedding their chunks and writing them, cheapest first.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::Result;
use dig_core::FileKind;
use dig_embed::Embedder;
use dig_extract::{Chunk, Embed};
use dig_store::{NewChunk, Store};

use crate::crawler::{Root, Skipped, crawl};

/// What the indexer reports as it goes.
#[derive(Debug, Clone)]
pub enum Event {
    /// A root was crawled: how many files it holds, how many are new or changed, what was skipped.
    Crawled { root: PathBuf, files: usize, pending: usize, removed: usize, skipped_secrets: usize, skipped_code_dirs: usize, huge_dirs: Vec<PathBuf> },
    /// A file was read and embedded.
    Indexed { path: PathBuf, kind: FileKind, chunks: usize },
    /// A file couldn't be read; it is tried again when it changes.
    Failed { path: PathBuf, error: String },
    /// Everything pending is done.
    Done { indexed: usize, failed: usize, seconds: f64 },
}

pub struct Indexer<'a> {
    pub store: &'a mut Store,
    pub embedder: &'a mut Embedder,
    pub on_event: Box<dyn FnMut(Event) + 'a>,
}

/// How many pending files are taken per round (texts of a round embed in shared batches).
const ROUND: usize = 32;
/// Texts per model batch (the context holds 16 sequences).
const TEXT_BATCH: usize = 16;

impl<'a> Indexer<'a> {
    /// Crawls `roots`, records what's there, forgets what's gone, and indexes everything pending.
    pub fn run(&mut self, roots: &[Root]) -> Result<()> {
        for root in roots {
            self.store.add_root(&root.path, root.code)?;
            let (candidates, skipped) = crawl(root);
            let mut pending = 0;
            let mut keep: HashSet<PathBuf> = HashSet::with_capacity(candidates.len());
            for c in &candidates {
                let (_, changed) = self.store.see_file(&c.path, c.mtime, c.size as i64, c.kind)?;
                pending += changed as usize;
                keep.insert(c.path.clone());
            }
            let removed = self.store.remove_missing(&root.path, &keep)?;
            let Skipped { secrets, code_project_dirs, huge_dirs, .. } = skipped;
            (self.on_event)(Event::Crawled { root: root.path.clone(), files: candidates.len(), pending, removed, skipped_secrets: secrets, skipped_code_dirs: code_project_dirs, huge_dirs });
        }
        self.index_pending()
    }

    /// Indexes every pending file, cheap kinds first.
    pub fn index_pending(&mut self) -> Result<()> {
        let started = Instant::now();
        let (mut indexed, mut failed) = (0usize, 0usize);
        loop {
            let files = self.store.pending_files(ROUND)?;
            if files.is_empty() {
                break;
            }
            // Read every file of the round first, so their texts can share model batches.
            let mut work: Vec<(dig_store::FileRow, Vec<Chunk>)> = Vec::new();
            for file in files {
                match dig_extract::extract(&file.path, file.kind) {
                    Ok(chunks) => work.push((file, chunks)),
                    Err(error) => {
                        let error = format!("{error:#}");
                        self.store.mark_failed(file.id, &error)?;
                        failed += 1;
                        (self.on_event)(Event::Failed { path: file.path, error });
                    }
                }
            }
            // Texts across the round, in batches.
            let mut texts: Vec<(usize, usize, String)> = Vec::new();
            for (i, (_, chunks)) in work.iter().enumerate() {
                for (j, chunk) in chunks.iter().enumerate() {
                    if let Embed::Text(prompted) = &chunk.embed {
                        texts.push((i, j, prompted.clone()));
                    }
                }
            }
            let mut vectors: Vec<Vec<Option<Vec<f32>>>> = work.iter().map(|(_, chunks)| vec![None; chunks.len()]).collect();
            let mut errors: Vec<Vec<Option<String>>> = work.iter().map(|(_, chunks)| vec![None; chunks.len()]).collect();
            for batch in texts.chunks(TEXT_BATCH) {
                let inputs: Vec<String> = batch.iter().map(|(_, _, t)| t.clone()).collect();
                match self.embedder.embed_texts(&inputs) {
                    Ok(out) => {
                        for ((i, j, _), v) in batch.iter().zip(out) {
                            vectors[*i][*j] = Some(v);
                        }
                    }
                    Err(error) => {
                        for (i, j, _) in batch {
                            errors[*i][*j] = Some(format!("{error:#}"));
                        }
                    }
                }
            }
            // Media, one at a time (each is its own model run).
            for (i, (_, chunks)) in work.iter().enumerate() {
                for (j, chunk) in chunks.iter().enumerate() {
                    let result = match &chunk.embed {
                        Embed::Image { path, budget } => self.embedder.embed_images(&[path.clone()], *budget).map(|mut v| v.remove(0)),
                        Embed::Audio { path } => self.embedder.embed_audio(&[path.clone()]).map(|mut v| v.remove(0)),
                        _ => continue,
                    };
                    match result {
                        Ok(embedding) => {
                            vectors[i][j] = embedding.vector;
                            errors[i][j] = embedding.error;
                        }
                        Err(error) => errors[i][j] = Some(format!("{error:#}")),
                    }
                }
            }
            // Audio windows and frames of this round are no longer needed once written below.
            let scratch = dig_extract::media::scratch_dir();
            // Write each file: a chunk whose embedding failed is kept for keywords; a file whose only content
            // chunk failed is marked failed so the error is visible.
            for (i, (file, chunks)) in work.into_iter().enumerate() {
                let mut rows = Vec::with_capacity(chunks.len());
                let mut content_failed: Option<String> = None;
                let mut content_ok = false;
                for (j, chunk) in chunks.into_iter().enumerate() {
                    let wanted = !matches!(chunk.embed, Embed::None);
                    let vector = vectors[i][j].take();
                    if wanted && vector.is_none() {
                        content_failed = errors[i][j].take().or(Some("no vector".into()));
                    } else if wanted {
                        content_ok = true;
                    }
                    let modality = if vector.is_some() { chunk.modality() } else { dig_core::Modality::None };
                    rows.push(NewChunk { kind: chunk.kind, modality, locator: chunk.locator, locator_end: chunk.locator_end, text: chunk.text, vector });
                }
                let _ = &scratch;
                match (content_ok, content_failed) {
                    (false, Some(error)) => {
                        self.store.mark_failed(file.id, &error)?;
                        failed += 1;
                        (self.on_event)(Event::Failed { path: file.path, error });
                    }
                    _ => {
                        let n = rows.len();
                        self.store.write_chunks(file.id, &rows)?;
                        indexed += 1;
                        (self.on_event)(Event::Indexed { path: file.path, kind: file.kind, chunks: n });
                    }
                }
            }
        }
        let _ = std::fs::remove_dir_all(dig_extract::media::scratch_dir());
        (self.on_event)(Event::Done { indexed, failed, seconds: started.elapsed().as_secs_f64() });
        Ok(())
    }
}

/// The roots the store remembers, for `dig index` with no arguments.
pub fn stored_roots(store: &Store) -> Result<Vec<Root>> {
    Ok(store.roots()?.into_iter().map(|(path, code)| Root { path, code }).collect())
}

/// Canonical, absolute, existing directory, or an error that says which.
pub fn resolve_root(path: &Path) -> Result<PathBuf> {
    let full = std::fs::canonicalize(path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    anyhow::ensure!(full.is_dir(), "{} is not a folder", full.display());
    Ok(full)
}

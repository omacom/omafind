//! A search: embed the query, take the nearest vectors and the keyword matches, fuse and rank them, and dress each
//! winner up as a `Hit` with its snippet and locator.

use std::collections::HashMap;

use anyhow::Result;
use dig_core::ranking::{self, KeywordHit, VectorHit};
use dig_core::{ChunkKind, FileKind, Hit, Locator, prompts};
use dig_embed::Embedder;
use dig_store::Store;

#[derive(Debug, Clone)]
pub struct SearchOptions {
    pub limit: usize,
    pub kinds: Vec<FileKind>,
    /// Candidates taken from each index before fusion.
    pub candidates: usize,
    /// The code index: the query is embedded with the code-retrieval prompt.
    pub code: bool,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self { limit: 20, kinds: Vec::new(), candidates: 200, code: false }
    }
}

/// `code: retry with backoff` → `Some("retry with backoff")`: a search of the code index and nothing else.
pub fn code_query(query: &str) -> Option<&str> {
    let trimmed = query.trim_start();
    let rest = trimmed.get(..5).filter(|p| p.eq_ignore_ascii_case("code:")).map(|_| &trimmed[5..])?;
    Some(rest.trim())
}

pub fn search(store: &Store, embedder: &mut Embedder, query: &str, options: &SearchOptions) -> Result<Vec<Hit>> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let words = ranking::query_words(query);
    let prompted = if options.code { prompts::code_query(query) } else { prompts::query(query) };
    let vector = embedder.embed_texts(&[prompted])?.remove(0);
    let nearest: Vec<VectorHit> = store
        .knn(&vector, options.candidates)?
        .into_iter()
        .map(|(chunk_id, similarity, modality)| VectorHit { chunk_id, similarity, modality })
        .collect();
    let keywords: Vec<KeywordHit> = store.keyword_search(&words, options.candidates)?.into_iter().map(|chunk_id| KeywordHit { chunk_id }).collect();

    let gated = ranking::gate(&nearest);
    let ids: Vec<i64> = gated.iter().map(|h| h.chunk_id).chain(keywords.iter().map(|h| h.chunk_id)).collect();
    let rows: HashMap<i64, dig_store::ChunkRow> = store.chunks(&ids)?.into_iter().map(|r| (r.id, r)).collect();
    let name_of = |row: &dig_store::ChunkRow| row.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();

    let fused = ranking::fuse(&gated, &keywords, |id| {
        rows.get(&id).map_or(0.0, |r| ranking::word_share(&words, &format!("{} {}", r.text, name_of(r))))
    });
    let fused = ranking::collapse(fused, |id| rows.get(&id).map(|r| r.file_id), |id| rows.get(&id).is_some_and(|r| r.kind == ChunkKind::Name));

    let mut hits = Vec::with_capacity(options.limit);
    for f in fused {
        let Some(row) = rows.get(&f.chunk_id) else { continue };
        if !options.kinds.is_empty() && !options.kinds.contains(&row.file_kind) {
            continue;
        }
        let locator = Locator::from_kind(row.kind, row.locator);
        let haystack = format!("{} {}", row.text, name_of(row)).to_lowercase();
        let matched_words = words.iter().filter(|w| haystack.contains(w.as_str())).cloned().collect();
        let snippet = match row.kind {
            ChunkKind::Name | ChunkKind::Image | ChunkKind::Frame | ChunkKind::PageImage | ChunkKind::Audio => String::new(),
            _ => ranking::snippet(&row.text, &words, 160),
        };
        hits.push(Hit {
            file_id: row.file_id,
            chunk_id: row.id,
            path: row.path.to_string_lossy().into_owned(),
            name: name_of(row),
            kind: row.file_kind,
            chunk_kind: row.kind,
            locator,
            locator_label: locator.map(|l| l.label()).unwrap_or_default(),
            snippet,
            score: f.score,
            similarity: f.similarity,
            matched_words,
        });
        if hits.len() >= options.limit {
            break;
        }
    }
    Ok(hits)
}

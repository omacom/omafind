//! A search: the nearest vectors and the keyword matches, fused and ranked, each winner dressed up as a `Hit` with its
//! snippet, locator and the reason it matched.
//!
//! Two stages, so the window paints before the model answers: `rank` with no query vector uses names and keywords
//! only (a few ms), and again with the vector for the full answer.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use anyhow::Result;
use dig_core::ranking::{self, Fused, KeywordHit, VectorHit};
use dig_core::{ChunkKind, FileKind, Hit, Locator, Reason, prompts};
use dig_embed::Embedder;
use dig_store::{ChunkRow, Filter, Store, VectorIndex};

/// Cosine similarity above which two photos count as the same picture: copies, crops, edits and bursts score 0.91
/// and up, different photos of the same subject under 0.7 (measured on EmbeddingGemma 2 image vectors).
const DUPLICATE: f32 = 0.88;

#[derive(Debug, Clone)]
pub struct SearchOptions {
    pub limit: usize,
    pub filter: Filter,
    /// Candidates taken from each index before fusion.
    pub candidates: usize,
    /// The code index: the query is embedded with the code-retrieval prompt.
    pub code: bool,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            limit: 20,
            filter: Filter::default(),
            candidates: 200,
            code: false,
        }
    }
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Timings {
    pub vector_ms: f64,
    pub keyword_ms: f64,
    pub total_ms: f64,
}

#[derive(Debug, Clone, Default)]
pub struct Answer {
    pub hits: Vec<Hit>,
    /// Files per kind in the unfiltered result list: the numbers on the filter chips.
    pub counts: Vec<(FileKind, usize)>,
    pub timings: Timings,
}

/// `code: retry with backoff` → `Some("retry with backoff")`: a search of the code index and nothing else.
pub fn code_query(query: &str) -> Option<&str> {
    let trimmed = query.trim_start();
    let rest = trimmed
        .get(..5)
        .filter(|p| p.eq_ignore_ascii_case("code:"))
        .map(|_| &trimmed[5..])?;
    Some(rest.trim())
}

/// The prompted text the model embeds for `text`.
pub fn prompted(text: &str, code: bool) -> String {
    if code {
        prompts::code_query(text)
    } else {
        prompts::query(text)
    }
}

/// A one-off search (the CLI without a daemon, tests): embeds, loads the vectors, ranks.
pub fn search(
    store: &Store,
    embedder: &mut Embedder,
    query: &str,
    options: &SearchOptions,
) -> Result<Vec<Hit>> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let vector = embedder
        .embed_texts(&[prompted(query, options.code)])?
        .remove(0);
    let index = VectorIndex::load(store)?;
    Ok(rank(store, Some((&index, &vector)), query, options)?.hits)
}

struct Pass {
    files: Vec<Fused>,
    rows: HashMap<i64, ChunkRow>,
    from_vector: HashSet<i64>,
}

/// Ranks `text` against the index. With `vector` (the matrix and the embedded query) it is the full search; without,
/// names and keywords only.
pub fn rank(
    store: &Store,
    vector: Option<(&VectorIndex, &[f32])>,
    text: &str,
    options: &SearchOptions,
) -> Result<Answer> {
    let started = Instant::now();
    let mut timings = Timings::default();
    let words = ranking::query_words(text);
    let filter = &options.filter;
    if words.is_empty() && filter.is_empty() {
        return Ok(Answer::default());
    }
    // Folders and dates narrow the vector scan through a set of file ids; kinds are checked per row.
    let unkinded = Filter {
        kinds: Vec::new(),
        ..filter.clone()
    };
    let allowed = store.file_ids(&unkinded)?;
    // One keyword query for both passes: BM25 over every match is the cost, not the limit.
    let t = Instant::now();
    let keyword_hits = store.keyword_hits(&words, options.candidates * 5, &unkinded)?;
    timings.keyword_ms += t.elapsed().as_secs_f64() * 1000.0;

    let pass = |kinds: &[FileKind], timings: &mut Timings| -> Result<Pass> {
        let mut nearest: Vec<VectorHit> = Vec::new();
        if let Some((index, query)) = vector.filter(|_| !words.is_empty()) {
            let t = Instant::now();
            let keep = |m: dig_store::RowMeta| {
                (kinds.is_empty() || kinds.contains(&m.kind))
                    && allowed.as_ref().is_none_or(|s| s.contains(&m.file_id))
            };
            let ids: Vec<i64> = index
                .search(query, options.candidates * 2, &keep)
                .into_iter()
                .map(|(id, _)| id)
                .collect();
            nearest = store
                .rescore(query, &ids)?
                .into_iter()
                .take(options.candidates)
                .map(|(chunk_id, similarity, modality)| VectorHit {
                    chunk_id,
                    similarity,
                    modality,
                })
                .collect();
            timings.vector_ms += t.elapsed().as_secs_f64() * 1000.0;
        }
        let t = Instant::now();
        let kinded = Filter {
            kinds: kinds.to_vec(),
            ..filter.clone()
        };
        let (keywords, names): (Vec<KeywordHit>, Vec<KeywordHit>) = if words.is_empty() {
            (
                Vec::new(),
                store
                    .recent_files(&kinded, options.candidates)?
                    .into_iter()
                    .map(|chunk_id| KeywordHit { chunk_id })
                    .collect(),
            )
        } else {
            (
                keyword_hits
                    .iter()
                    .filter(|(_, k)| kinds.is_empty() || kinds.contains(k))
                    .take(options.candidates)
                    .map(|&(chunk_id, _)| KeywordHit { chunk_id })
                    .collect(),
                store
                    .names_like(&words, 50, &kinded)?
                    .into_iter()
                    .map(|chunk_id| KeywordHit { chunk_id })
                    .collect(),
            )
        };
        timings.keyword_ms += t.elapsed().as_secs_f64() * 1000.0;

        let gated = ranking::gate(&nearest);
        let from_vector: HashSet<i64> = gated.iter().map(|h| h.chunk_id).collect();
        let ids: Vec<i64> = gated
            .iter()
            .map(|h| h.chunk_id)
            .chain(keywords.iter().chain(&names).map(|h| h.chunk_id))
            .collect();
        let rows: HashMap<i64, ChunkRow> =
            store.chunks(&ids)?.into_iter().map(|r| (r.id, r)).collect();
        let fused = ranking::fuse(&gated, &keywords, &names, |id| {
            rows.get(&id).map_or(0.0, |r| {
                ranking::word_share(&words, &format!("{} {}", r.text, name_of(r)))
            })
        });
        let mut files = ranking::collapse(
            fused,
            |id| rows.get(&id).map(|r| r.file_id),
            |id| rows.get(&id).is_some_and(|r| r.kind == ChunkKind::Name),
        );
        frecency(store, &mut files, &rows)?;
        Ok(Pass {
            files,
            rows,
            from_vector,
        })
    };

    let all = pass(&[], &mut timings)?;
    let mut counts: HashMap<FileKind, usize> = HashMap::new();
    for f in &all.files {
        if let Some(row) = all.rows.get(&f.chunk_id) {
            *counts.entry(row.file_kind).or_default() += 1;
        }
    }
    let shown = if filter.kinds.is_empty() {
        all
    } else {
        pass(&filter.kinds, &mut timings)?
    };
    if !filter.kinds.is_empty() {
        // The filtered pass sees further into its kinds than the mixed list did: its numbers are the ones shown.
        for kind in &filter.kinds {
            counts.remove(kind);
        }
        for f in &shown.files {
            if let Some(row) = shown
                .rows
                .get(&f.chunk_id)
                .filter(|r| filter.kinds.contains(&r.file_kind))
            {
                *counts.entry(row.file_kind).or_default() += 1;
            }
        }
    }

    let mut hits: Vec<Hit> = Vec::with_capacity(options.limit);
    // Photos already shown, with their vectors, for folding near-duplicates into the first of them.
    let mut pictures: Vec<(usize, Vec<f32>)> = Vec::new();
    for f in &shown.files {
        let Some(row) = shown.rows.get(&f.chunk_id) else {
            continue;
        };
        if !filter.kinds.is_empty() && !filter.kinds.contains(&row.file_kind) {
            continue;
        }
        if matches!(row.file_kind, FileKind::Image | FileKind::Screenshot) {
            let chunk = if row.modality == dig_core::Modality::Image {
                Some(row.id)
            } else {
                store.any_vector_chunk(row.file_id)?
            };
            if let Some(vector) = chunk.map(|c| store.vector_of(c)).transpose()?.flatten() {
                let twin = pictures.iter().find(|(_, v)| {
                    v.iter().zip(&vector).map(|(a, b)| a * b).sum::<f32>() >= DUPLICATE
                });
                if let Some(&(at, _)) = twin {
                    hits[at]
                        .duplicates
                        .push(row.path.to_string_lossy().into_owned());
                    continue;
                }
                pictures.push((hits.len(), vector));
            }
        }
        hits.push(hit(row, f, &words, shown.from_vector.contains(&f.chunk_id)));
        if hits.len() >= options.limit {
            break;
        }
    }
    // Folded near-duplicates are one result, on the chips too.
    for h in &hits {
        if let Some(n) = counts.get_mut(&h.kind) {
            *n = n.saturating_sub(h.duplicates.len());
        }
    }
    let mut counts: Vec<(FileKind, usize)> = counts.into_iter().collect();
    counts.sort_by_key(|(k, _)| k.priority());
    timings.total_ms = started.elapsed().as_secs_f64() * 1000.0;
    Ok(Answer {
        hits,
        counts,
        timings,
    })
}

/// Files opened from the window before rank a little higher, more for often and lately: at most +30 %, so
/// relevance still decides.
fn frecency(store: &Store, files: &mut [Fused], rows: &HashMap<i64, ChunkRow>) -> Result<()> {
    let ids: Vec<i64> = files
        .iter()
        .filter_map(|f| rows.get(&f.chunk_id).map(|r| r.file_id))
        .collect();
    let stats = store.open_stats(&ids)?;
    if stats.is_empty() {
        return Ok(());
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    for f in files.iter_mut() {
        let Some((count, last)) = rows.get(&f.chunk_id).and_then(|r| stats.get(&r.file_id)) else {
            continue;
        };
        let decay = 0.5f64.powf((now - last).max(0) as f64 / 604_800.0);
        f.score *= 1.0 + (0.1 * (1.0 + *count as f64).ln() * decay).min(0.3);
    }
    files.sort_by(|a, b| b.score.total_cmp(&a.score));
    Ok(())
}

/// Hits for these chunks in this order, for lists that aren't a search (recents).
pub fn hits_for(store: &Store, ids: &[i64]) -> Result<Vec<Hit>> {
    let rows: HashMap<i64, ChunkRow> = store.chunks(ids)?.into_iter().map(|r| (r.id, r)).collect();
    Ok(ids
        .iter()
        .filter_map(|id| rows.get(id))
        .map(|row| {
            hit(
                row,
                &Fused {
                    chunk_id: row.id,
                    score: 0.0,
                    similarity: None,
                    from_keywords: false,
                },
                &[],
                false,
            )
        })
        .collect())
}

/// Files like this chunk: its own vector as the query, its own file left out. Media chunks find media and text
/// chunks find text best, but the space is shared, so a photo also finds the PDF page that shows the same thing.
pub fn similar(
    store: &Store,
    index: &VectorIndex,
    chunk_id: i64,
    limit: usize,
) -> Result<Vec<Hit>> {
    let chunk_id = match store.vector_of(chunk_id)? {
        Some(_) => chunk_id,
        None => {
            let file = store.chunks(&[chunk_id])?.first().map(|r| r.file_id);
            match file
                .map(|f| store.any_vector_chunk(f))
                .transpose()?
                .flatten()
            {
                Some(id) => id,
                None => return Ok(Vec::new()),
            }
        }
    };
    let Some(vector) = store.vector_of(chunk_id)? else {
        return Ok(Vec::new());
    };
    let own = store.chunks(&[chunk_id])?.first().map(|r| r.file_id);
    let ids: Vec<i64> = index
        .search(&vector, 400, &|m| Some(m.file_id) != own)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    let near: Vec<VectorHit> = store
        .rescore(&vector, &ids)?
        .into_iter()
        .take(200)
        .map(|(chunk_id, similarity, modality)| VectorHit {
            chunk_id,
            similarity,
            modality,
        })
        .collect();
    let rows: HashMap<i64, ChunkRow> = store
        .chunks(&near.iter().map(|h| h.chunk_id).collect::<Vec<_>>())?
        .into_iter()
        .map(|r| (r.id, r))
        .collect();
    let fused = ranking::fuse(&near, &[], &[], |_| 0.0);
    let files = ranking::collapse(fused, |id| rows.get(&id).map(|r| r.file_id), |_| false);
    Ok(files
        .iter()
        .filter_map(|f| rows.get(&f.chunk_id).map(|r| hit(r, f, &[], true)))
        .take(limit)
        .collect())
}

fn name_of(row: &ChunkRow) -> String {
    row.path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn hit(row: &ChunkRow, f: &Fused, words: &[String], by_vector: bool) -> Hit {
    let locator = Locator::from_kind(row.kind, row.locator);
    let haystack = format!("{} {}", row.text, name_of(row)).to_lowercase();
    let matched_words = words
        .iter()
        .filter(|w| haystack.contains(w.as_str()))
        .cloned()
        .collect();
    let snippet = match row.kind {
        ChunkKind::Name
        | ChunkKind::Image
        | ChunkKind::Frame
        | ChunkKind::PageImage
        | ChunkKind::Audio => String::new(),
        _ => ranking::snippet(&row.text, words, 160),
    };
    Hit {
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
        reason: Reason::of(row.kind, by_vector),
        mtime: row.mtime,
        size: row.size,
        duplicates: Vec::new(),
    }
}

/// A raw query as typed plus the window's filter chips → what to search for, how, and the chips to show.
pub fn plan(
    raw: &str,
    kinds: &[FileKind],
    limit: usize,
) -> (String, SearchOptions, Vec<dig_core::query::Chip>) {
    let parsed = dig_core::query::parse(raw);
    let kinds = if parsed.kinds.is_empty() {
        kinds.to_vec()
    } else {
        parsed.kinds.clone()
    };
    let code = parsed.code || kinds == [FileKind::Code];
    let filter = Filter {
        kinds: if code { Vec::new() } else { kinds },
        folders: parsed.folders.clone(),
        after: parsed.after,
        before: parsed.before,
    };
    (
        parsed.text.clone(),
        SearchOptions {
            limit,
            filter,
            candidates: 200,
            code,
        },
        parsed.chips,
    )
}

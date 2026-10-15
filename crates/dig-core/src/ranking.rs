//! Fusing vector and keyword hits into one list, best file first.
//!
//! 1. Vector hits are judged by how much they stand out: a z-score of similarity against the other hits of the
//!    same modality (text vectors run higher than image or audio ones for the same topic). Hits below the gate are
//!    dropped, except the top few, which always stay so that a sparse index still answers.
//! 2. Keyword hits (FTS5, BM25) catch exact strings: invoice numbers, error codes, names.
//! 3. Reciprocal rank fusion joins the two lists, with a bonus when every query word appears in the chunk's text or
//!    the file name. Then the best chunk per file wins and files are ordered by it.

use std::collections::HashMap;

use crate::Modality;

/// A chunk the vector index returned, with its cosine similarity to the query.
#[derive(Debug, Clone, Copy)]
pub struct VectorHit {
    pub chunk_id: i64,
    pub similarity: f32,
    pub modality: Modality,
}

/// A chunk the keyword index returned, already in BM25 order (best first).
#[derive(Debug, Clone, Copy)]
pub struct KeywordHit {
    pub chunk_id: i64,
}

#[derive(Debug, Clone)]
pub struct Fused {
    pub chunk_id: i64,
    pub score: f64,
    pub similarity: Option<f32>,
    pub from_keywords: bool,
}

/// How far above the modality's mean a vector hit must sit, in standard deviations.
pub const Z_GATE: f64 = 1.5;
/// Vector hits that stay regardless of the gate.
pub const ALWAYS_KEEP: usize = 3;
/// RRF's smoothing constant.
const RRF_K: f64 = 60.0;
const ALL_WORDS_BONUS: f64 = 0.01;

/// Vector hits that stand out, best first.
pub fn gate(hits: &[VectorHit]) -> Vec<VectorHit> {
    let mut by_modality: HashMap<Modality, Vec<f64>> = HashMap::new();
    for hit in hits {
        by_modality.entry(hit.modality).or_default().push(hit.similarity as f64);
    }
    let stats: HashMap<Modality, (f64, f64)> = by_modality
        .into_iter()
        .map(|(modality, sims)| {
            let n = sims.len() as f64;
            let mean = sims.iter().sum::<f64>() / n;
            let var = sims.iter().map(|s| (s - mean).powi(2)).sum::<f64>() / n.max(1.0);
            (modality, (mean, var.sqrt()))
        })
        .collect();
    let mut sorted: Vec<VectorHit> = hits.to_vec();
    sorted.sort_by(|a, b| b.similarity.partial_cmp(&a.similarity).unwrap_or(std::cmp::Ordering::Equal));
    sorted
        .into_iter()
        .enumerate()
        .filter(|(rank, hit)| {
            if *rank < ALWAYS_KEEP {
                return true;
            }
            let (mean, sd) = stats[&hit.modality];
            // Too few of a modality to say what stands out: keep them.
            sd > 1e-6 && (hit.similarity as f64 - mean) / sd >= Z_GATE
        })
        .map(|(_, hit)| hit)
        .collect()
}

/// Reciprocal rank fusion of the gated vector list and the keyword list. `word_share(chunk_id)` is the share of
/// the query's words that appear in that chunk (0..=1): a keyword hit counts in proportion to it, so one common
/// word doesn't weigh as much as the whole phrase, and a chunk with every word earns a small bonus that breaks ties
/// toward exact matches.
pub fn fuse(vector: &[VectorHit], keywords: &[KeywordHit], word_share: impl Fn(i64) -> f64) -> Vec<Fused> {
    let mut scores: HashMap<i64, Fused> = HashMap::new();
    for (rank, hit) in vector.iter().enumerate() {
        let entry = scores.entry(hit.chunk_id).or_insert(Fused { chunk_id: hit.chunk_id, score: 0.0, similarity: None, from_keywords: false });
        entry.score += 1.0 / (RRF_K + rank as f64 + 1.0);
        entry.similarity = Some(hit.similarity);
    }
    for (rank, hit) in keywords.iter().enumerate() {
        let share = word_share(hit.chunk_id).clamp(0.0, 1.0);
        if share <= 0.0 {
            continue;
        }
        let entry = scores.entry(hit.chunk_id).or_insert(Fused { chunk_id: hit.chunk_id, score: 0.0, similarity: None, from_keywords: false });
        entry.score += share * share / (RRF_K + rank as f64 + 1.0);
        entry.from_keywords = true;
    }
    let mut fused: Vec<Fused> = scores.into_values().collect();
    for hit in &mut fused {
        if word_share(hit.chunk_id) >= 1.0 {
            hit.score += ALL_WORDS_BONUS;
        }
    }
    fused.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    fused
}

/// One entry per file, in score order. A file's score is its best chunk plus a share of its other hits, so a
/// photo whose name also matches outranks one that matched only by picture. The entry keeps the best *content*
/// chunk (not the file-name chunk) when one scored at least half as well, so the locator and snippet point at
/// something inside the file. `file_of` maps a chunk to its file; `is_name` says whether a chunk is the name.
pub fn collapse(fused: Vec<Fused>, file_of: impl Fn(i64) -> Option<i64>, is_name: impl Fn(i64) -> bool) -> Vec<Fused> {
    let mut per_file: Vec<(i64, Vec<Fused>)> = Vec::new();
    let mut index: HashMap<i64, usize> = HashMap::new();
    for hit in fused {
        let Some(file) = file_of(hit.chunk_id) else { continue };
        match index.get(&file) {
            Some(&i) => per_file[i].1.push(hit),
            None => {
                index.insert(file, per_file.len());
                per_file.push((file, vec![hit]));
            }
        }
    }
    let mut out: Vec<Fused> = per_file
        .into_iter()
        .map(|(_, hits)| {
            let best = hits[0].score;
            let rest: f64 = hits.iter().skip(1).map(|h| h.score).sum();
            let score = best + 0.5 * rest;
            let representative = hits
                .iter()
                .find(|h| !is_name(h.chunk_id) && h.score >= best * 0.5)
                .unwrap_or(&hits[0]);
            let similarity = representative.similarity.or_else(|| hits.iter().find_map(|h| h.similarity));
            Fused { chunk_id: representative.chunk_id, score, similarity, from_keywords: hits.iter().any(|h| h.from_keywords) }
        })
        .collect();
    out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// The share of `words` that appear in `text` (case-insensitive, substring), 0..=1.
pub fn word_share(words: &[String], text: &str) -> f64 {
    if words.is_empty() {
        return 0.0;
    }
    let lower = text.to_lowercase();
    words.iter().filter(|w| lower.contains(w.as_str())).count() as f64 / words.len() as f64
}

/// Words that say nothing about what is being looked for. Matching them would make every document a keyword hit.
const STOPWORDS: &[&str] = &[
    "a", "an", "the", "and", "or", "of", "in", "on", "at", "to", "for", "with", "by", "from", "about", "as", "is", "are",
    "was", "were", "be", "it", "its", "this", "that", "these", "those", "my", "me", "i", "we", "our", "you", "your", "they",
    "them", "their", "he", "she", "his", "her", "where", "when", "what", "which", "who", "how", "do", "does", "did", "have",
    "has", "had", "not", "no", "so", "if", "then", "than", "there", "here", "into", "over", "under", "up", "down", "out",
    "some", "any", "all", "one", "two", "talk", "talks", "talking", "say", "says", "said", "like", "thing", "things",
    "something", "someone", "find", "show", "get", "picture", "photo", "image", "file", "files", "document", "video",
    "recording", "clip", "page",
];

/// The words of a query, lowercased, for keyword search, the all-words bonus and snippet highlighting. Stopwords
/// and the `code:` prefix are left out; if nothing is left (a query of stopwords only), every word stays.
pub fn query_words(query: &str) -> Vec<String> {
    let all: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric() && c != '\'' && c != '-' && c != '_' && c != '.')
        .map(|w| w.trim_matches(|c: char| c == '\'' || c == '-' || c == '.').to_lowercase())
        .filter(|w| w.chars().count() >= 2 && w != "code:")
        .collect();
    let content: Vec<String> = all.iter().filter(|w| !STOPWORDS.contains(&w.as_str())).cloned().collect();
    if content.is_empty() { all } else { content }
}

/// Whether every word appears in `text` (case-insensitive, substring).
pub fn has_all_words(words: &[String], text: &str) -> bool {
    let lower = text.to_lowercase();
    !words.is_empty() && words.iter().all(|w| lower.contains(w.as_str()))
}

/// A window of `text` about `width` characters wide, placed where the most query words sit together (the earliest
/// such place on a tie), with ellipses for what's cut.
pub fn snippet(text: &str, words: &[String], width: usize) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.is_empty() {
        return String::new();
    }
    let lower = flat.to_lowercase();
    let chars: Vec<char> = flat.chars().collect();
    // Every occurrence of every word, as (char index, word index).
    let mut hits: Vec<(usize, usize)> = Vec::new();
    for (wi, word) in words.iter().enumerate() {
        let mut from = 0;
        while let Some(at) = lower[from..].find(word.as_str()) {
            let byte = from + at;
            hits.push((lower[..byte].chars().count(), wi));
            from = byte + word.len().max(1);
            if hits.len() > 2000 {
                break;
            }
        }
    }
    hits.sort();
    // The hit whose following window holds the most distinct words, each worth more the rarer it is in this text
    // (a word in the title repeats; the one that only appears in the clause is the one that was asked about).
    let mut occurrences = vec![0usize; words.len()];
    for &(_, wi) in &hits {
        occurrences[wi] += 1;
    }
    let mut best: Option<(f64, usize)> = None; // (weight, char index)
    for (i, &(at, _)) in hits.iter().enumerate() {
        let mut seen = std::collections::HashSet::new();
        for &(other, wi) in &hits[i..] {
            if other > at + width * 2 / 3 {
                break;
            }
            seen.insert(wi);
        }
        let weight: f64 = seen.iter().map(|&wi| 1.0 / occurrences[wi] as f64).sum();
        if best.is_none_or(|(w, _)| weight > w + 1e-9) {
            best = Some((weight, at));
        }
    }
    let center = best.map(|(_, at)| at).unwrap_or(0);
    let start = center.saturating_sub(width / 4);
    let end = (start + width).min(chars.len());
    let start = if end == chars.len() { end.saturating_sub(width) } else { start };
    let mut out: String = chars[start..end].iter().collect();
    if start > 0 {
        out = format!("…{}", out.trim_start());
    }
    if end < chars.len() {
        out = format!("{}…", out.trim_end());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(id: i64, sim: f32) -> VectorHit {
        VectorHit { chunk_id: id, similarity: sim, modality: Modality::Image }
    }

    #[test]
    fn gate_keeps_what_stands_out() {
        let hits = [hit(1, 0.73), hit(2, 0.59), hit(3, 0.55), hit(4, 0.55), hit(5, 0.55), hit(6, 0.54), hit(7, 0.49)];
        let kept = gate(&hits);
        // The top three always stay; the rest are within the noise.
        assert_eq!(kept.iter().map(|h| h.chunk_id).collect::<Vec<_>>(), vec![1, 2, 3]);
    }

    #[test]
    fn fusion_prefers_a_hit_in_both_lists() {
        let vector = [hit(1, 0.7), hit(2, 0.65)];
        let keywords = [KeywordHit { chunk_id: 2 }, KeywordHit { chunk_id: 3 }];
        let fused = fuse(&vector, &keywords, |_| 1.0);
        assert_eq!(fused[0].chunk_id, 2);
        assert!(fused[0].from_keywords && fused[0].similarity.is_some());
    }

    #[test]
    fn collapse_keeps_one_chunk_per_file() {
        let fused = vec![
            Fused { chunk_id: 1, score: 3.0, similarity: None, from_keywords: false },
            Fused { chunk_id: 2, score: 2.0, similarity: None, from_keywords: false },
            Fused { chunk_id: 3, score: 1.0, similarity: None, from_keywords: false },
        ];
        let out = collapse(fused, |c| Some(if c == 2 { 1 } else { c }), |_| false);
        assert_eq!(out.iter().map(|h| h.chunk_id).collect::<Vec<_>>(), vec![1, 3]);
        // Two hits on one file add up, and the content chunk represents the file over its name chunk.
        assert_eq!(out[0].score, 4.0);
        let fused = vec![
            Fused { chunk_id: 10, score: 1.0, similarity: None, from_keywords: true },
            Fused { chunk_id: 11, score: 0.9, similarity: Some(0.7), from_keywords: false },
        ];
        let out = collapse(fused, |_| Some(1), |c| c == 10);
        assert_eq!((out[0].chunk_id, out[0].similarity), (11, Some(0.7)));
    }

    #[test]
    fn stopwords_are_dropped_unless_nothing_remains() {
        assert_eq!(query_words("the clause about pets in the lease"), vec!["clause", "pets", "lease"]);
        assert_eq!(query_words("a dog on the beach"), vec!["dog", "beach"]);
        assert_eq!(query_words("where they talk about it"), vec!["where", "they", "talk", "about", "it"]);
        assert_eq!(query_words("INV-20394 DO_NOT_HONOR v1.2"), vec!["inv-20394", "do_not_honor", "v1.2"]);
    }

    #[test]
    fn snippets_center_where_the_words_cluster() {
        let text = "a ".repeat(100) + "the clause about pets in the lease says " + &"b ".repeat(100);
        let s = snippet(&text, &query_words("pets lease"), 60);
        assert!(s.contains("pets"), "{s}");
        assert!(s.starts_with('…') && s.ends_with('…'));
        // "lease" alone appears early; "pets" and "lease" together appear later: the pair wins.
        let text = "Residential Lease Agreement. ".to_owned() + &"x ".repeat(200) + "Tenants may keep pets under the lease. " + &"y ".repeat(50);
        let s = snippet(&text, &query_words("pets in the lease"), 80);
        assert!(s.contains("pets"), "{s}");
        assert_eq!(query_words("Payment DECLINED error!"), vec!["payment", "declined", "error"]);
    }
}

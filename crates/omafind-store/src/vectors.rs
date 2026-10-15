//! Nearest neighbours without an index structure: every vector as int8 in one flat matrix, scanned in parallel, the
//! best few hundred rescored against their float32 originals. On 8 threads that is ~7 ms per 100k chunks with the
//! same top 200 as an exact float search, where sqlite-vec's scalar `vec0` scan took ~110 ms (`examples/knn_bench`).
//!
//! The matrix lives in the searcher while the query encoder does (loaded on show, dropped when idle), and follows
//! the index through two append-only logs: `vectors.seq` for rows added, `vectors_gone.seq` for rows removed. The
//! worker is another process, so nothing is pushed; each search asks for rows past the last sequence it saw.

use std::collections::HashMap;

use anyhow::Result;
use omafind_core::{FileKind, Modality};
use rusqlite::params;

use crate::Store;

const DIM: usize = omafind_core::model::DIM;

/// A vector as the matrix stores it: symmetric int8 with one scale per row (`x ≈ q * scale`).
pub fn quantize(v: &[f32]) -> (Vec<i8>, f32) {
    let max = v.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    if max == 0.0 {
        return (vec![0; v.len()], 0.0);
    }
    let scale = max / 127.0;
    (
        v.iter()
            .map(|x| (x / scale).round().clamp(-127.0, 127.0) as i8)
            .collect(),
        scale,
    )
}

pub(crate) fn kind_code(kind: FileKind) -> u8 {
    FileKind::ALL.iter().position(|k| *k == kind).unwrap_or(0) as u8
}

pub(crate) fn kind_from_code(code: u8) -> FileKind {
    FileKind::ALL
        .get(code as usize)
        .copied()
        .unwrap_or(FileKind::Doc)
}

pub(crate) fn modality_code(m: Modality) -> u8 {
    match m {
        Modality::None => 0,
        Modality::Text => 1,
        Modality::Image => 2,
        Modality::Audio => 3,
    }
}

pub(crate) fn modality_from_code(code: u8) -> Modality {
    match code {
        1 => Modality::Text,
        2 => Modality::Image,
        3 => Modality::Audio,
        _ => Modality::None,
    }
}

/// What a filter sees of a row.
#[derive(Debug, Clone, Copy)]
pub struct RowMeta {
    pub chunk_id: i64,
    pub file_id: i64,
    pub kind: FileKind,
    pub modality: Modality,
}

#[derive(Debug, Clone, Copy)]
struct Row {
    chunk_id: i64,
    file_id: i64,
    kind: u8,
    modality: u8,
    scale: f32,
    alive: bool,
}

#[derive(Default)]
pub struct VectorIndex {
    rows: Vec<Row>,
    data: Vec<i8>,
    by_chunk: HashMap<i64, usize>,
    last_seq: i64,
    last_gone: i64,
    dead: usize,
}

/// A candidate: chunk id, approximate cosine similarity.
pub type Candidate = (i64, f32);

impl VectorIndex {
    pub fn load(store: &Store) -> Result<Self> {
        let mut index = Self {
            last_gone: store.max_gone_seq()?,
            ..Self::default()
        };
        index.append(store)?;
        Ok(index)
    }

    pub fn len(&self) -> usize {
        self.rows.len() - self.dead
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Bytes held, for the status line.
    pub fn bytes(&self) -> usize {
        self.data.len() + self.rows.len() * std::mem::size_of::<Row>()
    }

    /// Catches up with the index: rows removed since the last look, then rows added. Cheap when nothing changed
    /// (two indexed `max()` lookups).
    pub fn refresh(&mut self, store: &Store) -> Result<()> {
        let gone_max = store.max_gone_seq()?;
        if gone_max > self.last_gone {
            if store.min_gone_seq()? > self.last_gone + 1 {
                // The log was pruned past what we saw: start over.
                *self = Self::load(store)?;
                return Ok(());
            }
            let mut stmt = store.conn.prepare_cached(
                "SELECT seq, chunk_id FROM vectors_gone WHERE seq > ?1 ORDER BY seq",
            )?;
            let gone: Vec<(i64, i64)> = stmt
                .query_map([self.last_gone], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<_, _>>()?;
            for (seq, chunk) in gone {
                self.last_gone = seq;
                if let Some(i) = self.by_chunk.remove(&chunk) {
                    self.rows[i].alive = false;
                    self.dead += 1;
                }
            }
        }
        if store.max_vector_seq()? > self.last_seq {
            self.append(store)?;
        }
        if self.dead > 4096 && self.dead * 4 > self.rows.len() {
            self.compact();
        }
        Ok(())
    }

    fn append(&mut self, store: &Store) -> Result<()> {
        let mut stmt = store.conn.prepare_cached("SELECT seq, chunk_id, file_id, file_kind, modality, scale, q FROM vectors WHERE seq > ?1 ORDER BY seq")?;
        let mut rows = stmt.query([self.last_seq])?;
        while let Some(r) = rows.next()? {
            let seq: i64 = r.get(0)?;
            let chunk_id: i64 = r.get(1)?;
            let q = r.get_ref(6)?.as_blob()?;
            if q.len() != DIM {
                continue;
            }
            // A chunk id is written once (chunks are replaced, never updated), but be safe on a re-add.
            if let Some(i) = self.by_chunk.remove(&chunk_id) {
                self.rows[i].alive = false;
                self.dead += 1;
            }
            self.by_chunk.insert(chunk_id, self.rows.len());
            self.rows.push(Row {
                chunk_id,
                file_id: r.get(2)?,
                kind: r.get::<_, i64>(3)? as u8,
                modality: r.get::<_, i64>(4)? as u8,
                scale: r.get::<_, f64>(5)? as f32,
                alive: true,
            });
            self.data.extend(q.iter().map(|&b| b as i8));
            self.last_seq = seq;
        }
        Ok(())
    }

    fn compact(&mut self) {
        let mut rows = Vec::with_capacity(self.len());
        let mut data = Vec::with_capacity(self.len() * DIM);
        self.by_chunk.clear();
        for (i, row) in self.rows.iter().enumerate() {
            if row.alive {
                self.by_chunk.insert(row.chunk_id, rows.len());
                rows.push(*row);
                data.extend_from_slice(&self.data[i * DIM..(i + 1) * DIM]);
            }
        }
        self.rows = rows;
        self.data = data;
        self.dead = 0;
    }

    /// The `k` rows most similar to `query` (a unit vector) that pass `keep`, best first, by approximate cosine.
    pub fn search(
        &self,
        query: &[f32],
        k: usize,
        keep: &(dyn Fn(RowMeta) -> bool + Sync),
    ) -> Vec<Candidate> {
        if k == 0 || self.rows.is_empty() || query.len() != DIM {
            return Vec::new();
        }
        let (q, q_scale) = quantize(query);
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .min(8);
        let threads = if self.rows.len() < 20_000 { 1 } else { threads };
        let per = self.rows.len().div_ceil(threads);
        let scan = |from: usize, to: usize| -> Vec<Candidate> {
            let mut out: Vec<Candidate> = Vec::new();
            for i in from..to {
                let row = &self.rows[i];
                if !row.alive {
                    continue;
                }
                let meta = RowMeta {
                    chunk_id: row.chunk_id,
                    file_id: row.file_id,
                    kind: kind_from_code(row.kind),
                    modality: modality_from_code(row.modality),
                };
                if !keep(meta) {
                    continue;
                }
                let dot = dot_i8(&self.data[i * DIM..(i + 1) * DIM], &q);
                out.push((row.chunk_id, dot as f32 * row.scale * q_scale));
            }
            best(out, k)
        };
        let mut all: Vec<Candidate> = if threads == 1 {
            scan(0, self.rows.len())
        } else {
            std::thread::scope(|s| {
                let handles: Vec<_> = (0..threads)
                    .map(|t| {
                        let (from, to) = (t * per, ((t + 1) * per).min(self.rows.len()));
                        s.spawn(move || scan(from, to))
                    })
                    .collect();
                handles
                    .into_iter()
                    .flat_map(|h| h.join().unwrap_or_default())
                    .collect()
            })
        };
        all = best(all, k);
        all
    }

    pub fn meta(&self, chunk_id: i64) -> Option<RowMeta> {
        let row = self.rows[*self.by_chunk.get(&chunk_id)?];
        Some(RowMeta {
            chunk_id: row.chunk_id,
            file_id: row.file_id,
            kind: kind_from_code(row.kind),
            modality: modality_from_code(row.modality),
        })
    }
}

/// The best `k`, sorted.
fn best(mut v: Vec<Candidate>, k: usize) -> Vec<Candidate> {
    if v.len() > k {
        v.select_nth_unstable_by(k - 1, |a, b| b.1.total_cmp(&a.1));
        v.truncate(k);
    }
    v.sort_by(|a, b| b.1.total_cmp(&a.1));
    v
}

/// Written so LLVM vectorizes it on baseline x86-64 (SSE2 `pmaddwd`) and aarch64 (NEON): fixed-width lanes, i32
/// accumulators.
#[inline]
fn dot_i8(a: &[i8], b: &[i8]) -> i32 {
    let mut acc = [0i32; 16];
    for (ca, cb) in a.as_chunks::<16>().0.iter().zip(b.as_chunks::<16>().0) {
        for l in 0..16 {
            acc[l] += ca[l] as i32 * cb[l] as i32;
        }
    }
    acc.iter().sum()
}

impl Store {
    pub(crate) fn max_vector_seq(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT coalesce(max(seq), 0) FROM vectors", [], |r| {
                r.get(0)
            })?)
    }

    pub(crate) fn max_gone_seq(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT coalesce(max(seq), 0) FROM vectors_gone", [], |r| {
                r.get(0)
            })?)
    }

    pub(crate) fn min_gone_seq(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT coalesce(min(seq), 0) FROM vectors_gone", [], |r| {
                r.get(0)
            })?)
    }

    /// Exact cosine similarity of `query` against these chunks' float32 vectors, with each chunk's modality.
    pub fn rescore(&self, query: &[f32], ids: &[i64]) -> Result<Vec<(i64, f32, Modality)>> {
        let mut out = Vec::with_capacity(ids.len());
        for batch in ids.chunks(500) {
            let marks = vec!["?"; batch.len()].join(",");
            let mut stmt = self.conn.prepare(&format!(
                "SELECT chunk_id, modality, f FROM vectors WHERE chunk_id IN ({marks})"
            ))?;
            let mut rows = stmt.query(rusqlite::params_from_iter(batch.iter()))?;
            while let Some(r) = rows.next()? {
                let f = r.get_ref(2)?.as_blob()?;
                let sim: f32 = f
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .zip(query)
                    .map(|(c, q)| f32::from_le_bytes([c[0], c[1], c[2], c[3]]) * q)
                    .sum();
                out.push((
                    r.get(0)?,
                    sim,
                    modality_from_code(r.get::<_, i64>(1)? as u8),
                ));
            }
        }
        out.sort_by(|a, b| b.1.total_cmp(&a.1));
        Ok(out)
    }

    /// Forgets removal records nobody can need any more: a searcher further behind reloads from scratch.
    pub fn prune_gone(&self, keep: i64) -> Result<()> {
        self.conn.execute("DELETE FROM vectors_gone WHERE seq <= (SELECT coalesce(max(seq), 0) FROM vectors_gone) - ?1", params![keep])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantized_dot_tracks_cosine() {
        let a: Vec<f32> = (0..DIM)
            .map(|i| ((i * 7 % 13) as f32 - 6.0) / 40.0)
            .collect();
        let b: Vec<f32> = (0..DIM)
            .map(|i| ((i * 5 % 11) as f32 - 5.0) / 35.0)
            .collect();
        let norm = |v: &[f32]| v.iter().map(|x| x * x).sum::<f32>().sqrt();
        let (a, b): (Vec<f32>, Vec<f32>) = (
            a.iter().map(|x| x / norm(&a)).collect(),
            b.iter().map(|x| x / norm(&b)).collect(),
        );
        let exact: f32 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
        let ((qa, sa), (qb, sb)) = (quantize(&a), quantize(&b));
        let approx = dot_i8(&qa, &qb) as f32 * sa * sb;
        assert!((exact - approx).abs() < 0.01, "{exact} vs {approx}");
    }
}

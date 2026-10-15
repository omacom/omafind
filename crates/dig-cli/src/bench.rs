//! `dig bench`: how fast a search is at a given index size, without needing that many real files. A synthetic index
//! (clustered unit vectors, passages from a made-up vocabulary, file names, six kinds) is built once per size under
//! `~/.cache/dig/bench/` and searched through the same code the daemon runs; with the model installed, the query
//! encoder is timed on the CPU and the GPU too.

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::Result;
use dig_core::{ChunkKind, FileKind, Modality};
use dig_index::SearchOptions;
use dig_store::{Filter, NewChunk, Store, VectorIndex};

const DIM: usize = dig_core::model::DIM;
const CHUNKS_PER_FILE: usize = 5;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32 - 0.5
    }
    fn gauss(&mut self) -> f32 {
        (0..4).map(|_| self.unit()).sum()
    }
}

fn normalize(mut v: Vec<f32>) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
    v.iter_mut().for_each(|x| *x /= n);
    v
}

/// Pronounceable made-up words, so FTS sees a realistic spread of rare and common terms.
fn vocabulary(rng: &mut Rng) -> Vec<String> {
    const C: &[&str] = &[
        "b", "d", "f", "g", "k", "l", "m", "n", "p", "r", "s", "t", "v", "z", "ch", "st",
    ];
    const V: &[&str] = &["a", "e", "i", "o", "u", "ai", "ou"];
    (0..6000)
        .map(|_| {
            let syllables = 1 + rng.next() % 3;
            (0..syllables)
                .map(|_| {
                    format!(
                        "{}{}",
                        C[(rng.next() % C.len() as u64) as usize],
                        V[(rng.next() % V.len() as u64) as usize]
                    )
                })
                .collect()
        })
        .collect()
}

fn build(path: &Path, chunks: usize) -> Result<()> {
    let tmp = path.with_extension("building");
    let _ = std::fs::remove_file(&tmp);
    let mut store = Store::open(&tmp)?;
    let mut rng = Rng(0x2545f4914f6cdd1d);
    let words = vocabulary(&mut rng);
    let centroids: Vec<Vec<f32>> = (0..(chunks / 40).max(16))
        .map(|_| normalize((0..DIM).map(|_| rng.gauss()).collect()))
        .collect();
    let kinds = [
        FileKind::Image,
        FileKind::Screenshot,
        FileKind::Pdf,
        FileKind::Doc,
        FileKind::Audio,
        FileKind::Video,
    ];
    let files = chunks / CHUNKS_PER_FILE;
    let t = Instant::now();
    let now = 1_790_000_000i64;
    let mut batch: Vec<(i64, Vec<NewChunk>)> = Vec::new();
    for f in 0..files {
        let kind = kinds[f % kinds.len()];
        // Zipf-ish word choice: a few words everywhere, most rare.
        let word = |rng: &mut Rng| -> &str {
            &words[((rng.next() % 1000) * (rng.next() % 6000) / 1000) as usize % words.len()]
        };
        let name = format!(
            "{}-{}-{f}.{}",
            word(&mut rng),
            word(&mut rng),
            match kind {
                FileKind::Image => "jpg",
                FileKind::Screenshot => "png",
                FileKind::Pdf => "pdf",
                FileKind::Doc => "md",
                FileKind::Audio => "m4a",
                _ => "mp4",
            }
        );
        let path = PathBuf::from(format!("/bench/{}/{}/{name}", kind.as_str(), f % 50));
        let (id, _) = store.see_file(&path, now - (f as i64 * 600), 100_000, kind)?;
        let mut rows = vec![NewChunk {
            kind: ChunkKind::Name,
            modality: Modality::None,
            locator: None,
            locator_end: None,
            text: name.replace(['-', '.'], " "),
            vector: None,
        }];
        for c in 0..CHUNKS_PER_FILE {
            let centroid = &centroids[(f * 7 + c) % centroids.len()];
            let vector = normalize(centroid.iter().map(|x| x + rng.gauss() * 0.05).collect());
            let (chunk_kind, modality, text) = match kind {
                FileKind::Image | FileKind::Screenshot => {
                    (ChunkKind::Image, Modality::Image, String::new())
                }
                FileKind::Audio => (ChunkKind::Audio, Modality::Audio, String::new()),
                FileKind::Video => (ChunkKind::Frame, Modality::Image, String::new()),
                _ => (
                    ChunkKind::Passage,
                    Modality::Text,
                    (0..60)
                        .map(|_| word(&mut rng))
                        .collect::<Vec<_>>()
                        .join(" "),
                ),
            };
            rows.push(NewChunk {
                kind: chunk_kind,
                modality,
                locator: Some(c as f64),
                locator_end: None,
                text,
                vector: Some(vector),
            });
        }
        batch.push((id, rows));
        if batch.len() == 2000 || f + 1 == files {
            store.write_many(
                &batch
                    .iter()
                    .map(|(id, rows)| (*id, rows.as_slice()))
                    .collect::<Vec<_>>(),
            )?;
            batch.clear();
            eprint!(
                "\r  building {}/{} files ({:.0} s)",
                f + 1,
                files,
                t.elapsed().as_secs_f64()
            );
        }
    }
    store.set_meta("bench_words", &words[..50].join(" "))?;
    drop(store);
    std::fs::rename(&tmp, path)?;
    let _ = std::fs::remove_file(tmp.with_extension("building-wal"));
    let _ = std::fs::remove_file(tmp.with_extension("building-shm"));
    eprintln!(
        "\r  built {chunks} chunks in {:.0} s                ",
        t.elapsed().as_secs_f64()
    );
    Ok(())
}

fn percentiles(mut ms: Vec<f64>) -> (f64, f64) {
    ms.sort_by(|a, b| a.total_cmp(b));
    (
        ms[ms.len() / 2],
        ms[(ms.len() * 95 / 100).min(ms.len() - 1)],
    )
}

pub fn run(sizes: &[usize], model: bool) -> Result<()> {
    let dir = dig_core::paths::cache_dir().join("bench");
    std::fs::create_dir_all(&dir)?;
    println!(
        "{:<10} {:>10} {:>14} {:>14} {:>14} {:>12}",
        "chunks", "load", "quick p50/95", "full p50/95", "photos p50/95", "memory"
    );
    for &n in sizes {
        let path = dir.join(format!("bench-{n}.sqlite"));
        if !path.exists() {
            build(&path, n)?;
        }
        let store = Store::open(&path)?;
        let t = Instant::now();
        let index = VectorIndex::load(&store)?;
        let load = t.elapsed().as_secs_f64();
        let words: Vec<String> = store
            .get_meta("bench_words")?
            .unwrap_or_default()
            .split(' ')
            .map(str::to_owned)
            .collect();
        let mut rng = Rng(7);
        let queries: Vec<(String, Vec<f32>)> = (0..30)
            .map(|i| {
                let text = format!(
                    "{} {}",
                    words[i % words.len()],
                    words[(i * 7 + 3) % words.len()]
                );
                (text, normalize((0..DIM).map(|_| rng.gauss()).collect()))
            })
            .collect();
        let options = SearchOptions {
            limit: 40,
            ..SearchOptions::default()
        };
        let photos = SearchOptions {
            limit: 40,
            filter: Filter {
                kinds: vec![FileKind::Image, FileKind::Screenshot],
                ..Filter::default()
            },
            ..SearchOptions::default()
        };
        let time = |vector: bool, options: &SearchOptions| -> Result<(f64, f64)> {
            dig_index::rank(
                &store,
                vector.then(|| (&index, queries[0].1.as_slice())),
                &queries[0].0,
                options,
            )?;
            let mut ms = Vec::new();
            for (text, v) in &queries {
                let t = Instant::now();
                dig_index::rank(
                    &store,
                    vector.then_some((&index, v.as_slice())),
                    text,
                    options,
                )?;
                ms.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            Ok(percentiles(ms))
        };
        let quick = time(false, &options)?;
        let full = time(true, &options)?;
        let filtered = time(true, &photos)?;
        println!(
            "{:<10} {:>8.2} s {:>6.1}/{:<5.1}ms {:>6.1}/{:<5.1}ms {:>6.1}/{:<5.1}ms {:>9} MB",
            n,
            load,
            quick.0,
            quick.1,
            full.0,
            full.1,
            filtered.0,
            filtered.1,
            index.bytes() >> 20
        );
    }
    if model {
        println!();
        let models = dig_embed::Models::locate(&dig_core::model::dir())?;
        let queries = [
            "a dog on the beach",
            "the clause about pets in the lease",
            "where they talk about sleep",
            "invoice from the dentist",
        ];
        for cpu in [true, false] {
            let t = Instant::now();
            let mut embedder = dig_embed::Embedder::new(
                &models,
                &dig_embed::Options {
                    text_only: true,
                    cpu_only: cpu,
                    ..Default::default()
                },
            )?;
            let load = t.elapsed().as_secs_f64();
            embedder.embed_texts(&[dig_index::prompted("warm up", false)])?;
            let mut ms = Vec::new();
            for _ in 0..5 {
                for q in queries {
                    let t = Instant::now();
                    embedder.embed_texts(&[dig_index::prompted(q, false)])?;
                    ms.push(t.elapsed().as_secs_f64() * 1000.0);
                }
            }
            let (p50, p95) = percentiles(ms);
            println!(
                "query encoder on {:<4} load {load:.2} s, embed p50 {p50:.1} ms, p95 {p95:.1} ms",
                if cpu { "CPU" } else { "GPU" }
            );
        }
    }
    Ok(())
}

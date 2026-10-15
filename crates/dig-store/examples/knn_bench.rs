//! How nearest-neighbour search scales with the index, per storage layout. Synthetic clustered unit vectors, so it
//! needs no model:
//!
//!   cargo run --release -p dig-store --example knn_bench -- 100000 [dir]

use std::time::Instant;

use rusqlite::{Connection, params};

const DIM: usize = 768;
const K: usize = 200;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        // Sum of uniforms ≈ normal enough for clusters.
        ((self.0 >> 40) as f32 / (1u64 << 24) as f32) - 0.5
    }
    fn gauss(&mut self) -> f32 {
        (0..4).map(|_| self.next()).sum::<f32>()
    }
}

fn unit(mut v: Vec<f32>) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    v.iter_mut().for_each(|x| *x /= n);
    v
}

fn bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn quantize(v: &[f32]) -> Vec<u8> {
    v.iter()
        .map(|x| ((x * 127.0 / 0.25).round().clamp(-127.0, 127.0) as i8) as u8)
        .collect()
}

fn main() -> anyhow::Result<()> {
    let n: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(100_000);
    let dir = std::env::args()
        .nth(2)
        .unwrap_or_else(|| std::env::temp_dir().to_string_lossy().into_owned());
    unsafe {
        type Init = unsafe extern "C" fn(
            *mut rusqlite::ffi::sqlite3,
            *mut *mut std::os::raw::c_char,
            *const rusqlite::ffi::sqlite3_api_routines,
        ) -> std::os::raw::c_int;
        let init: Init = std::mem::transmute(sqlite_vec::sqlite3_vec_init as *const ());
        rusqlite::ffi::sqlite3_auto_extension(Some(init));
    }
    let path = format!("{dir}/knn-bench-{n}.sqlite");
    let fresh = !std::path::Path::new(&path).exists();
    let mut conn = Connection::open(&path)?;
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = OFF; PRAGMA mmap_size = 4294967296; PRAGMA cache_size = -1000000;")?;

    let mut rng = Rng(0x9e3779b97f4a7c15);
    let centroids: Vec<Vec<f32>> = (0..(n / 50).max(10))
        .map(|_| unit((0..DIM).map(|_| rng.gauss()).collect()))
        .collect();
    let kinds = ["image", "screenshot", "pdf", "doc", "audio", "video"];
    if fresh {
        conn.execute_batch(&format!(
            "CREATE VIRTUAL TABLE vf USING vec0(kind text, embedding float[{DIM}] distance_metric=cosine);
             CREATE VIRTUAL TABLE vi USING vec0(kind text, embedding int8[{DIM}] distance_metric=cosine);
             CREATE VIRTUAL TABLE vb USING vec0(embedding bit[{DIM}]);
             CREATE TABLE raw (id INTEGER PRIMARY KEY, kind TEXT, f BLOB, q BLOB);"
        ))?;
        let t = Instant::now();
        let tx = conn.transaction()?;
        {
            let mut a = tx.prepare("INSERT INTO vf(rowid, kind, embedding) VALUES (?1, ?2, ?3)")?;
            let mut b =
                tx.prepare("INSERT INTO vi(rowid, kind, embedding) VALUES (?1, ?2, vec_int8(?3))")?;
            let mut c = tx
                .prepare("INSERT INTO vb(rowid, embedding) VALUES (?1, vec_quantize_binary(?2))")?;
            let mut d = tx.prepare("INSERT INTO raw(id, kind, f, q) VALUES (?1, ?2, ?3, ?4)")?;
            for i in 0..n {
                let c0 = &centroids[i % centroids.len()];
                let v = unit(c0.iter().map(|x| x + rng.gauss() * 0.04).collect());
                let kind = kinds[i % kinds.len()];
                let (f, q) = (bytes(&v), quantize(&v));
                a.execute(params![i as i64 + 1, kind, f])?;
                b.execute(params![i as i64 + 1, kind, q])?;
                c.execute(params![i as i64 + 1, f])?;
                d.execute(params![i as i64 + 1, kind, f, q])?;
            }
        }
        tx.commit()?;
        eprintln!("built {n} in {:.1} s", t.elapsed().as_secs_f64());
    }

    let queries: Vec<Vec<f32>> = (0..8)
        .map(|i| {
            unit(
                centroids[i * 7 % centroids.len()]
                    .iter()
                    .map(|x| x + rng.gauss() * 0.06)
                    .collect(),
            )
        })
        .collect();
    let time = |label: &str,
                f: &mut dyn FnMut(&[f32]) -> Vec<i64>,
                truth: Option<&Vec<Vec<i64>>>|
     -> Vec<Vec<i64>> {
        f(&queries[0]); // warm
        let mut ms = Vec::new();
        let mut out = Vec::new();
        for q in &queries {
            let t = Instant::now();
            out.push(f(q));
            ms.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let recall = truth.map(|t| {
            let hits: usize = t
                .iter()
                .zip(&out)
                .map(|(a, b)| b.iter().filter(|x| a.contains(x)).count())
                .sum();
            hits as f64 / (t.len() * K) as f64
        });
        println!(
            "{label:<34} p50 {:>8.1} ms  max {:>8.1} ms  recall@{K} {}",
            ms[ms.len() / 2],
            ms[ms.len() - 1],
            recall.map_or("—".into(), |r| format!("{r:.3}"))
        );
        out
    };

    fn sql_knn<'c>(
        conn: &'c Connection,
        table: &str,
        qbytes: fn(&[f32]) -> Vec<u8>,
        wrap: &str,
        filter: &str,
    ) -> impl FnMut(&[f32]) -> Vec<i64> + 'c {
        let sql = format!(
            "SELECT rowid FROM {table} WHERE embedding MATCH {wrap} AND k = {K} {filter} ORDER BY distance"
        );
        move |q: &[f32]| -> Vec<i64> {
            let mut st = conn.prepare_cached(&sql).unwrap();
            st.query_map([qbytes(q)], |r| r.get(0))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        }
    }
    let truth = time(
        "vec0 float[768] cosine",
        &mut sql_knn(&conn, "vf", bytes, "?1", ""),
        None,
    );
    time(
        "vec0 int8[768] cosine",
        &mut sql_knn(&conn, "vi", quantize, "vec_int8(?1)", ""),
        Some(&truth),
    );
    time(
        "vec0 bit[768] hamming (no rescore)",
        &mut sql_knn(&conn, "vb", bytes, "vec_quantize_binary(?1)", ""),
        Some(&truth),
    );
    time(
        "vec0 float + kind = 'image'",
        &mut sql_knn(&conn, "vf", bytes, "?1", "AND kind = 'image'"),
        None,
    );

    // The whole matrix in memory, scanned in Rust: what a resident searcher could do.
    let t = Instant::now();
    let mut ids = Vec::with_capacity(n);
    let mut f32s: Vec<f32> = Vec::with_capacity(n * DIM);
    let mut i8s: Vec<i8> = Vec::with_capacity(n * DIM);
    {
        let mut st = conn.prepare("SELECT id, f, q FROM raw ORDER BY id")?;
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            ids.push(r.get::<_, i64>(0)?);
            let f: Vec<u8> = r.get(1)?;
            f32s.extend(f.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)));
            let q: Vec<u8> = r.get(2)?;
            i8s.extend(q.iter().map(|&b| b as i8));
        }
    }
    eprintln!(
        "loaded matrix in {:.2} s ({} MB f32, {} MB i8)",
        t.elapsed().as_secs_f64(),
        (f32s.len() * 4) >> 20,
        i8s.len() >> 20
    );
    fn top(scores: Vec<(f32, usize)>, ids: &[i64]) -> Vec<i64> {
        let mut s = scores;
        let k = K.min(s.len());
        s.select_nth_unstable_by(k - 1, |a, b| b.0.partial_cmp(&a.0).unwrap());
        s.truncate(k);
        s.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
        s.into_iter().map(|(_, i)| ids[i]).collect()
    }
    time(
        "rust f32 dot scan",
        &mut |q| {
            let scores = f32s
                .as_chunks::<DIM>()
                .0
                .iter()
                .enumerate()
                .map(|(i, row)| (row.iter().zip(q).map(|(a, b)| a * b).sum::<f32>(), i))
                .collect();
            top(scores, &ids)
        },
        Some(&truth),
    );
    time(
        "rust i8 dot scan",
        &mut |q| {
            let qq: Vec<i8> = quantize(q).into_iter().map(|b| b as i8).collect();
            let scores = i8s
                .as_chunks::<DIM>()
                .0
                .iter()
                .enumerate()
                .map(|(i, row)| {
                    (
                        row.iter()
                            .zip(&qq)
                            .map(|(&a, &b)| a as i32 * b as i32)
                            .sum::<i32>() as f32,
                        i,
                    )
                })
                .collect();
            top(scores, &ids)
        },
        Some(&truth),
    );
    time(
        "rust i8 scan + f32 rescore of 800",
        &mut |q| {
            let qq: Vec<i8> = quantize(q).into_iter().map(|b| b as i8).collect();
            let mut s: Vec<(i32, usize)> = i8s
                .as_chunks::<DIM>()
                .0
                .iter()
                .enumerate()
                .map(|(i, row)| {
                    (
                        row.iter()
                            .zip(&qq)
                            .map(|(&a, &b)| a as i32 * b as i32)
                            .sum::<i32>(),
                        i,
                    )
                })
                .collect();
            let m = 800.min(s.len());
            s.select_nth_unstable_by(m - 1, |a, b| b.0.cmp(&a.0));
            let rescored = s[..m]
                .iter()
                .map(|&(_, i)| {
                    (
                        f32s[i * DIM..(i + 1) * DIM]
                            .iter()
                            .zip(q)
                            .map(|(a, b)| a * b)
                            .sum::<f32>(),
                        i,
                    )
                })
                .collect();
            top(rescored, &ids)
        },
        Some(&truth),
    );
    Ok(())
}

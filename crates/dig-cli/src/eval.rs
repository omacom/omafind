//! `dig eval`: recall@1 and @5 over a query set with known answers, so a ranking change is measured, not felt.

use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Deserialize)]
struct Case {
    query: String,
    /// A substring of the expected file's name.
    expect: String,
    #[serde(default)]
    note: String,
}

pub fn run(
    queries: &Path,
    search: &mut dyn FnMut(&str) -> Result<Vec<dig_core::Hit>>,
) -> Result<bool> {
    let cases: Vec<Case> = serde_json::from_slice(
        &std::fs::read(queries).with_context(|| format!("reading {}", queries.display()))?,
    )?;
    let (mut at1, mut at5) = (0usize, 0usize);
    println!("{:<48} {:<8} top result", "query", "rank");
    for case in &cases {
        let hits = search(&case.query)?;
        let rank = hits
            .iter()
            .position(|h| h.name.contains(&case.expect))
            .map(|r| r + 1);
        at1 += (rank == Some(1)) as usize;
        at5 += rank.is_some_and(|r| r <= 5) as usize;
        println!(
            "{:<48} {:<8} {}{}",
            case.query,
            rank.map(|r| r.to_string()).unwrap_or_else(|| "miss".into()),
            hits.first().map(|h| h.name.clone()).unwrap_or_default(),
            if case.note.is_empty() {
                String::new()
            } else {
                format!("   ({})", case.note)
            }
        );
    }
    let n = cases.len().max(1);
    println!(
        "recall@1 {:.2}  recall@5 {:.2}  ({} queries)",
        at1 as f64 / n as f64,
        at5 as f64 / n as f64,
        cases.len()
    );
    Ok(at5 == cases.len())
}

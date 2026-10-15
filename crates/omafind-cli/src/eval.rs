//! `omafind eval`: recall@1 and @5 over a query set with known answers, so a ranking change is measured, not felt.

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
    search: &mut dyn FnMut(&str) -> Result<Vec<omafind_core::Hit>>,
) -> Result<bool> {
    let cases: Vec<Case> = serde_json::from_slice(
        &std::fs::read(queries).with_context(|| format!("reading {}", queries.display()))?,
    )?;
    let framed = crate::ui::framed();
    if framed {
        crate::ui::intro("eval", queries.display())?;
    } else {
        println!("{:<48} {:<8} top result", "query", "rank");
    }
    let (mut at1, mut at5) = (0usize, 0usize);
    let width = cases
        .iter()
        .map(|c| c.query.chars().count())
        .max()
        .unwrap_or(0);
    for case in &cases {
        let hits = search(&case.query)?;
        let rank = hits
            .iter()
            .position(|h| h.name.contains(&case.expect))
            .map(|r| r + 1);
        at1 += (rank == Some(1)) as usize;
        at5 += rank.is_some_and(|r| r <= 5) as usize;
        let rank_text = rank.map(|r| r.to_string()).unwrap_or_else(|| "miss".into());
        let top = hits.first().map(|h| h.name.clone()).unwrap_or_default();
        let note = if case.note.is_empty() {
            String::new()
        } else {
            format!("   ({})", case.note)
        };
        if framed {
            let mark = match rank {
                Some(1) => console::style("✓").green().to_string(),
                Some(r) if r <= 5 => console::style(format!("{r}")).yellow().to_string(),
                _ => console::style("✗").red().to_string(),
            };
            crate::ui::line(format!(
                "{mark} {:<width$}  {}",
                case.query,
                crate::ui::dim(format!("{top}{note}"))
            ));
            crate::ui::beat(20);
        } else {
            println!("{:<48} {:<8} {top}{note}", case.query, rank_text);
        }
    }
    let n = cases.len().max(1);
    let summary = format!(
        "recall@1 {:.2}  recall@5 {:.2}  ({} queries)",
        at1 as f64 / n as f64,
        at5 as f64 / n as f64,
        cases.len()
    );
    if framed {
        crate::ui::line("");
        if at5 == cases.len() {
            cliclack::outro(summary)?;
        } else {
            cliclack::outro_cancel(summary)?;
        }
    } else {
        println!("{summary}");
    }
    Ok(at5 == cases.len())
}

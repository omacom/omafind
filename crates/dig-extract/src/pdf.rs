//! PDFs through poppler's command-line tools, which Omarchy ships (`pdftotext`, `pdftoppm`, `pdfinfo`): the text of
//! each page as a chunk, and a rendered image of pages with little text (scans, slides) so they can be found by
//! what they show.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use dig_core::{ChunkKind, prompts};

use crate::{Chunk, Embed, PAGE_BUDGET};

/// A page with fewer characters than this is treated as a picture and rendered.
pub const MIN_PAGE_TEXT: usize = 200;
/// Pages read per file in this phase (the rest of a huge PDF waits for the long-file pass).
pub const MAX_PAGES: usize = 300;
/// Rendered (scanned) pages per file.
pub const MAX_RENDERED_PAGES: usize = 60;
/// Text on one page is also split when it is long (a dense page can be 4,000+ characters).
const PAGE_PASSAGE_CHARS: usize = 2000;

pub fn extract(path: &Path) -> Result<Vec<Chunk>> {
    let title = path.file_stem().and_then(|s| s.to_str()).map(|s| s.replace(['_', '-'], " "));
    let pages = page_texts(path)?;
    let mut chunks = Vec::new();
    let mut rendered = 0usize;
    for (index, text) in pages.iter().enumerate().take(MAX_PAGES) {
        let page = (index + 1) as f64;
        let clean = text.trim();
        if clean.chars().count() >= MIN_PAGE_TEXT {
            for body in split_page(clean) {
                let embed = if crate::text::mostly_numeric(&body) { Embed::None } else { Embed::Text(prompts::document(title.as_deref(), &body)) };
                chunks.push(Chunk { kind: ChunkKind::Page, locator: Some(page), locator_end: None, text: body, embed });
            }
        } else {
            // Little or no text: a scan, a slide, a figure. Keep what text there is for keywords and show the model
            // the page itself.
            if !clean.is_empty() {
                chunks.push(Chunk { kind: ChunkKind::Page, locator: Some(page), locator_end: None, text: clean.to_owned(), embed: Embed::None });
            }
            if rendered < MAX_RENDERED_PAGES {
                if let Ok(image) = render_page(path, index + 1) {
                    rendered += 1;
                    chunks.push(Chunk { kind: ChunkKind::PageImage, locator: Some(page), locator_end: None, text: String::new(), embed: Embed::Image { path: image, budget: PAGE_BUDGET } });
                }
            }
        }
    }
    if chunks.is_empty() {
        bail!("no readable pages");
    }
    Ok(chunks)
}

/// Every page's text, in order. `pdftotext` separates pages with form feeds.
pub fn page_texts(path: &Path) -> Result<Vec<String>> {
    let output = Command::new("pdftotext")
        .args(["-layout", "-enc", "UTF-8"])
        .arg("-l")
        .arg(MAX_PAGES.to_string())
        .arg(path)
        .arg("-")
        .output()
        .context("running pdftotext (poppler)")?;
    if !output.status.success() {
        bail!("pdftotext: {}", String::from_utf8_lossy(&output.stderr).trim());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut pages: Vec<String> = text.split('\u{c}').map(|p| p.to_owned()).collect();
    // pdftotext ends the last page with a form feed too, leaving an empty tail.
    if pages.last().is_some_and(|p| p.trim().is_empty()) && pages.len() > 1 {
        pages.pop();
    }
    Ok(pages)
}

/// Renders page `page` (from 1) to a PNG in the cache and returns its path.
pub fn render_page(path: &Path, page: usize) -> Result<PathBuf> {
    let dir = dig_core::paths::cache_dir().join("pages");
    std::fs::create_dir_all(&dir)?;
    let stem = format!("{:016x}-{page}", hash(path));
    let out = dir.join(format!("{stem}.png"));
    if out.exists() {
        return Ok(out);
    }
    let status = Command::new("pdftoppm")
        .args(["-png", "-singlefile", "-r", "110", "-f"])
        .arg(page.to_string())
        .arg("-l")
        .arg(page.to_string())
        .arg(path)
        .arg(dir.join(&stem))
        .status()
        .context("running pdftoppm (poppler)")?;
    if !status.success() || !out.exists() {
        bail!("pdftoppm failed on page {page}");
    }
    Ok(out)
}

fn split_page(text: &str) -> Vec<String> {
    if text.chars().count() <= PAGE_PASSAGE_CHARS {
        return vec![text.to_owned()];
    }
    crate::text::passages(text).into_iter().map(|(_, body)| body).collect()
}

fn hash(path: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    std::fs::metadata(path).ok().and_then(|m| m.modified().ok()).hash(&mut h);
    h.finish()
}

//! Plain text documents: read as UTF-8 (lossy), cut into overlapping passages at paragraph and sentence breaks.

use std::path::Path;

use anyhow::{Context, Result};
use dig_core::{ChunkKind, prompts};

use crate::{Chunk, Embed};

/// About this many characters per passage, and this much shared with the previous one.
pub const PASSAGE_CHARS: usize = 1500;
pub const OVERLAP_CHARS: usize = 200;
/// Files bigger than this are read in part (the first chunks), the rest after everything else in a later phase.
pub const MAX_BYTES: u64 = 8 << 20;
pub const MAX_PASSAGES: usize = 400;

pub fn extract(path: &Path) -> Result<Vec<Chunk>> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_BYTES as usize)]);
    let title = path.file_stem().and_then(|s| s.to_str()).map(|s| s.replace(['_', '-'], " "));
    Ok(passages(&text)
        .into_iter()
        .take(MAX_PASSAGES)
        .map(|(offset, body)| {
            let embed = if mostly_numeric(&body) { Embed::None } else { Embed::Text(prompts::document(title.as_deref(), &body)) };
            Chunk { kind: ChunkKind::Passage, locator: Some(offset as f64), locator_end: Some((offset + body.chars().count()) as f64), text: body, embed }
        })
        .collect())
}

/// Passages as (character offset, text). Breaks fall at blank lines when one is near the target length, else at a
/// sentence end, else at a space; each passage starts `OVERLAP_CHARS` before the previous one ended.
pub fn passages(text: &str) -> Vec<(usize, String)> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut start = 0usize;
    while start < chars.len() {
        // Skip leading whitespace.
        while start < chars.len() && chars[start].is_whitespace() {
            start += 1;
        }
        if start >= chars.len() {
            break;
        }
        let target = (start + PASSAGE_CHARS).min(chars.len());
        let end = if target == chars.len() { target } else { break_before(&chars, start, target) };
        let body: String = chars[start..end].iter().collect();
        let body = body.trim().to_owned();
        if !body.is_empty() {
            out.push((start, body));
        }
        if end >= chars.len() {
            break;
        }
        let next = end.saturating_sub(OVERLAP_CHARS).max(start + 1);
        start = next;
    }
    out
}

/// The best place to cut in `[start, target]`: a blank line in the last third, else a sentence end there, else a
/// space, else `target`.
fn break_before(chars: &[char], start: usize, target: usize) -> usize {
    let floor = start + (target - start) * 2 / 3;
    let mut i = target;
    while i > floor {
        if chars[i - 1] == '\n' && i >= 2 && chars[i - 2] == '\n' {
            return i;
        }
        i -= 1;
    }
    let mut i = target;
    while i > floor {
        if matches!(chars[i - 1], '.' | '!' | '?' | '。') && (i == chars.len() || chars[i].is_whitespace()) {
            return i;
        }
        i -= 1;
    }
    let mut i = target;
    while i > floor {
        if chars[i - 1].is_whitespace() {
            return i;
        }
        i -= 1;
    }
    target
}

/// Tables of numbers, logs of ids, data dumps: the model has nothing to say about them, keyword search does.
pub fn mostly_numeric(text: &str) -> bool {
    let (mut digits, mut letters) = (0usize, 0usize);
    for c in text.chars() {
        if c.is_ascii_digit() {
            digits += 1;
        } else if c.is_alphabetic() {
            letters += 1;
        }
    }
    digits + letters > 40 && digits * 2 > letters
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passages_overlap_and_break_at_sentences() {
        let sentence = "The quick brown fox jumps over the lazy dog. ";
        let text = sentence.repeat(120); // ~5,400 chars
        let p = passages(&text);
        assert!(p.len() >= 4, "{}", p.len());
        for (offset, body) in &p {
            assert!(body.len() <= PASSAGE_CHARS + 2);
            assert!(body.ends_with('.'), "cut mid-sentence at {offset}: …{}", &body[body.len() - 20..]);
        }
        // Consecutive passages share text.
        let a: Vec<char> = p[0].1.chars().collect();
        let tail: String = a[a.len() - 50..].iter().collect();
        assert!(p[1].1.contains(tail.trim()));
        assert_eq!(p[0].0, 0);
    }

    #[test]
    fn short_text_is_one_passage() {
        assert_eq!(passages("hello world").len(), 1);
        assert!(passages("   \n\n ").is_empty());
    }

    #[test]
    fn numbers_are_keyword_only() {
        assert!(mostly_numeric(&"12345 67890 ".repeat(10)));
        assert!(!mostly_numeric("Rent is due on the first of each month by bank transfer; late payments incur a fee."));
    }
}

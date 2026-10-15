//! Turning a file into chunks: pieces with a kind, a place in the file, text for keyword search and snippets, and
//! what to embed (prompted text, an image at a token budget, or audio).
//!
//! Phase 1 covers images and plain text (txt, md, and the like). Later phases add PDFs, OCR, office documents,
//! audio, video and code; each is one more `match` arm here.

pub mod classify;
pub mod text;

use std::path::{Path, PathBuf};

use anyhow::Result;
use dig_core::{ChunkKind, FileKind, Modality};

/// What to run through the model for a chunk.
#[derive(Debug, Clone)]
pub enum Embed {
    /// Keyword search only.
    None,
    /// A prompted document string.
    Text(String),
    /// A picture at a vision token budget (one of dig_embed::sizing::BUDGETS).
    Image { path: PathBuf, budget: usize },
    /// Up to 30 s of 16 kHz audio.
    Audio { path: PathBuf },
}

/// One piece of a file, before embedding.
#[derive(Debug, Clone)]
pub struct Chunk {
    pub kind: ChunkKind,
    pub locator: Option<f64>,
    pub locator_end: Option<f64>,
    /// What keyword search sees and a snippet is cut from.
    pub text: String,
    pub embed: Embed,
}

impl Chunk {
    pub fn modality(&self) -> Modality {
        match self.embed {
            Embed::None => Modality::None,
            Embed::Text(_) => Modality::Text,
            Embed::Image { .. } => Modality::Image,
            Embed::Audio { .. } => Modality::Audio,
        }
    }
}

/// Vision token budgets per use (70 / 140 / 280 / 560 / 1120).
pub const PHOTO_BUDGET: usize = 140;
pub const SCREENSHOT_BUDGET: usize = 280;
pub const PAGE_BUDGET: usize = 280;
pub const FRAME_BUDGET: usize = 70;

/// Whether this build reads files of `kind` (later phases add the rest).
pub fn supported(kind: FileKind) -> bool {
    matches!(kind, FileKind::Screenshot | FileKind::Image | FileKind::Doc)
}

/// The chunks of `path`, which `classify` said is a `kind`. The file name chunk comes first so a file is always
/// findable by its name.
pub fn extract(path: &Path, kind: FileKind) -> Result<Vec<Chunk>> {
    let mut chunks = vec![name_chunk(path)];
    match kind {
        FileKind::Screenshot => chunks.push(Chunk {
            kind: ChunkKind::Image,
            locator: None,
            locator_end: None,
            text: String::new(),
            embed: Embed::Image { path: path.to_path_buf(), budget: SCREENSHOT_BUDGET },
        }),
        FileKind::Image => chunks.push(Chunk {
            kind: ChunkKind::Image,
            locator: None,
            locator_end: None,
            text: String::new(),
            embed: Embed::Image { path: path.to_path_buf(), budget: PHOTO_BUDGET },
        }),
        FileKind::Doc => {
            let ext = classify::extension(path);
            anyhow::ensure!(classify::PLAIN_TEXT.contains(&ext.as_str()), ".{ext} documents aren't read yet");
            chunks.extend(text::extract(path)?)
        }
        FileKind::Pdf | FileKind::Audio | FileKind::Video | FileKind::Code => {
            anyhow::bail!("{} files aren't read yet", kind.as_str())
        }
    }
    Ok(chunks)
}

/// The file's name and folder names, for keyword search only.
fn name_chunk(path: &Path) -> Chunk {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let folders: Vec<String> = path
        .parent()
        .map(|p| p.components().rev().take(3).filter_map(|c| c.as_os_str().to_str().map(str::to_owned)).collect())
        .unwrap_or_default();
    Chunk {
        kind: ChunkKind::Name,
        locator: None,
        locator_end: None,
        text: format!("{} {}", words_of(&name), folders.iter().map(|f| words_of(f)).collect::<Vec<_>>().join(" ")),
        embed: Embed::None,
    }
}

/// `IMG_2024-07-04_beach-day.jpg` → `IMG 2024 07 04 beach day jpg IMG_2024-07-04_beach-day.jpg`: the pieces and
/// the whole, so both "beach day" and the exact name match.
fn words_of(name: &str) -> String {
    let split: String = name.chars().map(|c| if c.is_alphanumeric() { c } else { ' ' }).collect();
    let mut words: Vec<&str> = split.split_whitespace().collect();
    words.push(name);
    words.join(" ")
}

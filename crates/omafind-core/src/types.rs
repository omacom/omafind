//! What a file is, what a searchable piece of it is, and what a search returns.

use serde::{Deserialize, Serialize};

/// What a file is, for indexing and for grouping results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileKind {
    Screenshot,
    Image,
    Pdf,
    Doc,
    Audio,
    Video,
    /// Source code, only in the code index.
    Code,
}

impl FileKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Screenshot => "screenshot",
            Self::Image => "image",
            Self::Pdf => "pdf",
            Self::Doc => "doc",
            Self::Audio => "audio",
            Self::Video => "video",
            Self::Code => "code",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "screenshot" => Self::Screenshot,
            "image" => Self::Image,
            "pdf" => Self::Pdf,
            "doc" => Self::Doc,
            "audio" => Self::Audio,
            "video" => Self::Video,
            "code" => Self::Code,
            _ => return None,
        })
    }

    /// The first indexing pass goes cheap and common first: what people look for most is searchable soonest.
    pub fn priority(self) -> u8 {
        match self {
            Self::Screenshot => 0,
            Self::Image => 1,
            Self::Doc => 2,
            Self::Pdf => 3,
            Self::Audio => 4,
            Self::Video => 5,
            Self::Code => 6,
        }
    }

    pub const ALL: [FileKind; 7] = [
        Self::Screenshot,
        Self::Image,
        Self::Pdf,
        Self::Doc,
        Self::Audio,
        Self::Video,
        Self::Code,
    ];
}

/// One searchable piece of a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChunkKind {
    /// The file name and folder names (keyword search only).
    Name,
    /// The image itself.
    Image,
    /// Text read off a screenshot (keyword search only).
    Ocr,
    /// A PDF page's text.
    Page,
    /// A PDF page rendered as an image (scans, slides).
    PageImage,
    /// A passage of a document's text.
    Passage,
    /// A window of audio (a file, or a video's soundtrack).
    Audio,
    /// A video keyframe.
    Frame,
    /// A stretch of lines of a code file.
    Lines,
}

impl ChunkKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Image => "image",
            Self::Ocr => "ocr",
            Self::Page => "page",
            Self::PageImage => "page_image",
            Self::Passage => "passage",
            Self::Audio => "audio",
            Self::Frame => "frame",
            Self::Lines => "lines",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "name" => Self::Name,
            "image" => Self::Image,
            "ocr" => Self::Ocr,
            "page" => Self::Page,
            "page_image" => Self::PageImage,
            "passage" => Self::Passage,
            "audio" => Self::Audio,
            "frame" => Self::Frame,
            "lines" => Self::Lines,
            _ => return None,
        })
    }
}

/// What produced a chunk's vector. Text vectors score higher than image or audio vectors for the same topic, so
/// similarities are judged per modality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Modality {
    /// Keyword search only, no vector.
    None,
    Text,
    Image,
    Audio,
}

impl Modality {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Text => "text",
            Self::Image => "image",
            Self::Audio => "audio",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "none" => Self::None,
            "text" => Self::Text,
            "image" => Self::Image,
            "audio" => Self::Audio,
            _ => return None,
        })
    }
}

/// Where in a file a chunk is.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "lowercase")]
pub enum Locator {
    /// A PDF page, from 1.
    Page(u32),
    /// A character offset into a document.
    Offset(u64),
    /// A moment in seconds.
    Seconds(f64),
    /// A line, from 1.
    Line(u32),
}

impl Locator {
    /// As people read it: `p. 12`, `03:41`, `L88`; nothing for an offset (the snippet says where).
    pub fn label(self) -> String {
        match self {
            Self::Page(page) => format!("p. {page}"),
            Self::Offset(_) => String::new(),
            Self::Seconds(seconds) => {
                let total = seconds.max(0.0).round() as u64;
                let (h, m, s) = (total / 3600, total % 3600 / 60, total % 60);
                if h > 0 {
                    format!("{h}:{m:02}:{s:02}")
                } else {
                    format!("{m:02}:{s:02}")
                }
            }
            Self::Line(line) => format!("L{line}"),
        }
    }

    /// What goes in the index: the kind says how to read the number back.
    pub fn value(self) -> f64 {
        match self {
            Self::Page(p) => p as f64,
            Self::Offset(o) => o as f64,
            Self::Seconds(s) => s,
            Self::Line(l) => l as f64,
        }
    }

    pub fn from_kind(kind: ChunkKind, value: Option<f64>) -> Option<Self> {
        let value = value?;
        Some(match kind {
            ChunkKind::Page | ChunkKind::PageImage => Self::Page(value as u32),
            ChunkKind::Passage | ChunkKind::Ocr => Self::Offset(value as u64),
            ChunkKind::Audio | ChunkKind::Frame => Self::Seconds(value),
            ChunkKind::Lines => Self::Line(value as u32),
            ChunkKind::Name | ChunkKind::Image => return None,
        })
    }
}

/// One search result: a file and the best place in it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hit {
    pub file_id: i64,
    pub chunk_id: i64,
    pub path: String,
    pub name: String,
    pub kind: FileKind,
    pub chunk_kind: ChunkKind,
    pub locator: Option<Locator>,
    pub locator_label: String,
    /// The matched passage (or OCR text) around the query words, or empty for an image.
    pub snippet: String,
    /// Fused rank score, higher is better.
    pub score: f64,
    /// Cosine similarity of the vector match, if the hit came from one.
    pub similarity: Option<f32>,
    /// Which of the query's words appear in the chunk text or file name.
    pub matched_words: Vec<String>,
    /// Why this file is here, for the tag on its row.
    #[serde(default)]
    pub reason: Reason,
    /// Modification time, unix seconds.
    #[serde(default)]
    pub mtime: i64,
    #[serde(default)]
    pub size: i64,
    /// Near-duplicates folded into this result (the same photo copied, cropped, edited, or a burst), by path.
    #[serde(default)]
    pub duplicates: Vec<String>,
}

/// What matched: the picture, the sound, the meaning of its text, its name, or only some of the words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Reason {
    Picture,
    Sound,
    #[default]
    Text,
    Name,
    Words,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Picture => "picture",
            Self::Sound => "sound",
            Self::Text => "text",
            Self::Name => "name",
            Self::Words => "words",
        }
    }

    pub fn of(chunk: ChunkKind, by_vector: bool) -> Self {
        match (chunk, by_vector) {
            (ChunkKind::Name, _) => Self::Name,
            (ChunkKind::Image | ChunkKind::Frame | ChunkKind::PageImage, true) => Self::Picture,
            (ChunkKind::Audio, true) => Self::Sound,
            (_, true) => Self::Text,
            (_, false) => Self::Words,
        }
    }
}

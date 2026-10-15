//! What a folder holds before it is indexed: counts by kind and a rough first-pass time, for the folder picker.
//! Bounded: a walk stops after `MAX_FILES` candidates or `BUDGET` seconds and says it was cut short, and several
//! folders are surveyed at once.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use dig_core::FileKind;

pub const MAX_FILES: usize = 20_000;
pub const BUDGET: Duration = Duration::from_secs(4);

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Survey {
    pub path: PathBuf,
    pub files: usize,
    pub by_kind: BTreeMap<FileKind, usize>,
    /// Rough seconds for the first pass on a GPU (×6 on the CPU).
    pub seconds: f64,
    /// The walk hit its limit; counts are a lower bound.
    pub truncated: bool,
}

/// Seconds per file of each kind on a mid-range GPU, measured in Phase 0–3.
fn cost(kind: FileKind, size: u64) -> f64 {
    match kind {
        FileKind::Image => 0.8,
        FileKind::Screenshot => 1.8,
        FileKind::Doc => 0.3,
        FileKind::Pdf => 3.0,
        FileKind::Audio => 0.4 * (size as f64 / 1_000_000.0).max(0.5),
        FileKind::Video => 2.0 * (size as f64 / 20_000_000.0).max(0.5),
        FileKind::Code => 1.0,
    }
}

pub fn survey(path: &Path, code: bool) -> Survey {
    let started = Instant::now();
    let mut out = Survey { path: path.to_path_buf(), ..Default::default() };
    let root = crate::Root { path: path.to_path_buf(), code };
    crate::crawler::walk(&root, |candidate| {
        out.files += 1;
        *out.by_kind.entry(candidate.kind).or_insert(0) += 1;
        out.seconds += cost(candidate.kind, candidate.size);
        if out.files >= MAX_FILES || started.elapsed() > BUDGET {
            out.truncated = true;
            return false;
        }
        true
    });
    out
}

/// Surveys folders side by side (one thread each).
pub fn survey_all(paths: &[PathBuf], code: bool) -> Vec<Survey> {
    std::thread::scope(|scope| {
        let handles: Vec<_> = paths.iter().map(|p| scope.spawn(move || survey(p, code))).collect();
        handles.into_iter().map(|h| h.join().unwrap_or_default()).collect()
    })
}

impl Survey {
    /// `312 photos, 40 documents (about 6 min)`.
    pub fn summary(&self) -> String {
        let parts: Vec<String> = self.by_kind.iter().map(|(k, n)| format!("{n}{} {}", if self.truncated { "+" } else { "" }, plural(*k, *n))).collect();
        if parts.is_empty() {
            return "nothing to index".into();
        }
        let s = self.seconds;
        let time = if s < 60.0 { format!("{s:.0} s") } else if s < 3600.0 { format!("{:.0} min", s / 60.0) } else { format!("{:.1} h", s / 3600.0) };
        format!("{} · about {time}{}", parts.join(", "), if self.truncated { " or more" } else { "" })
    }
}

pub fn plural(kind: FileKind, n: usize) -> &'static str {
    match (kind, n) {
        (FileKind::Image, 1) => "photo",
        (FileKind::Image, _) => "photos",
        (FileKind::Screenshot, 1) => "screenshot",
        (FileKind::Screenshot, _) => "screenshots",
        (FileKind::Pdf, 1) => "PDF",
        (FileKind::Pdf, _) => "PDFs",
        (FileKind::Doc, 1) => "document",
        (FileKind::Doc, _) => "documents",
        (FileKind::Audio, 1) => "recording",
        (FileKind::Audio, _) => "recordings",
        (FileKind::Video, 1) => "video",
        (FileKind::Video, _) => "videos",
        (FileKind::Code, _) => "code files",
    }
}

//! What a path is, by extension and a few hints. `None` means Dig doesn't index it.

use std::path::Path;

use dig_core::FileKind;

const IMAGES: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "bmp", "heic", "heif", "avif", "tif", "tiff"];
const DOCS: &[&str] = &["txt", "md", "markdown", "rst", "org", "adoc", "text", "log", "csv", "tsv", "json", "yaml", "yml", "toml", "html", "htm", "xml", "docx", "odt", "rtf", "doc", "epub"];
const AUDIO: &[&str] = &["mp3", "m4a", "wav", "flac", "ogg", "opus", "aac", "wma", "aiff"];
const VIDEO: &[&str] = &["mp4", "mkv", "mov", "webm", "avi", "m4v", "mpg", "mpeg", "wmv"];

/// Plain text documents read in Phase 1; the rest of DOCS wait for pandoc.
pub const PLAIN_TEXT: &[&str] = &["txt", "md", "markdown", "rst", "org", "adoc", "text", "log", "csv", "tsv", "json", "yaml", "yml", "toml", "html", "htm", "xml"];

pub fn extension(path: &Path) -> String {
    path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).unwrap_or_default()
}

/// The kind of file at `path`, from its extension; screenshots by name or folder.
pub fn classify(path: &Path) -> Option<FileKind> {
    let ext = extension(path);
    let ext = ext.as_str();
    if IMAGES.contains(&ext) {
        return Some(if looks_like_screenshot(path) { FileKind::Screenshot } else { FileKind::Image });
    }
    if ext == "pdf" {
        return Some(FileKind::Pdf);
    }
    if DOCS.contains(&ext) {
        return Some(FileKind::Doc);
    }
    if AUDIO.contains(&ext) {
        return Some(FileKind::Audio);
    }
    if VIDEO.contains(&ext) {
        return Some(FileKind::Video);
    }
    None
}

/// A screenshot by the names screenshot tools give (`Screenshot 2024-…`, `screenshot-…`, `grim-…`, `swappy-…`) or
/// by living in a folder called Screenshots.
pub fn looks_like_screenshot(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_ascii_lowercase();
    if name.starts_with("screenshot") || name.starts_with("screen shot") || name.starts_with("grim-") || name.starts_with("swappy-") || name.starts_with("scrot") {
        return true;
    }
    path.components().any(|c| c.as_os_str().to_str().is_some_and(|s| s.eq_ignore_ascii_case("screenshots")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_by_extension_and_name() {
        assert_eq!(classify(Path::new("/p/IMG_1.JPG")), Some(FileKind::Image));
        assert_eq!(classify(Path::new("/p/Screenshots/x.png")), Some(FileKind::Screenshot));
        assert_eq!(classify(Path::new("/p/Screenshot 2026-10-14.png")), Some(FileKind::Screenshot));
        assert_eq!(classify(Path::new("/p/lease.pdf")), Some(FileKind::Pdf));
        assert_eq!(classify(Path::new("/p/notes.md")), Some(FileKind::Doc));
        assert_eq!(classify(Path::new("/p/memo.m4a")), Some(FileKind::Audio));
        assert_eq!(classify(Path::new("/p/clip.mkv")), Some(FileKind::Video));
        assert_eq!(classify(Path::new("/p/main.rs")), None);
        assert_eq!(classify(Path::new("/p/noext")), None);
    }
}

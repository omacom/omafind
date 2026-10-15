//! Pictures of results, kept in `~/.cache/omafind/thumbs`: `small` for rows and the photo grid, made while indexing so a
//! search shows them at once; `large` for the preview pane, made the first time a result is previewed.
//!
//! Photos and video keyframes are decoded and scaled in-process (the indexer has already pulled the frames out with
//! ffmpeg); what the `image` crate can't read (HEIC, RAW) goes through `ffmpegthumbnailer`, PDF pages through
//! `pdftoppm`, both on every Omarchy install.

use std::path::{Path, PathBuf};
use std::process::Command;

use omafind_core::FileKind;

use crate::{Chunk, Embed};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    /// 384 px on the long side, uncropped: rows and the masonry grid size each picture's box to its own shape.
    Small,
    /// 1280 px on the long side.
    Large,
}

impl Size {
    pub fn pixels(self) -> u32 {
        match self {
            Self::Small => 384,
            Self::Large => 1280,
        }
    }

    fn dir(self) -> &'static str {
        match self {
            Self::Small => "fit",
            Self::Large => "l",
        }
    }
}

pub fn dir() -> PathBuf {
    omafind_core::paths::cache_dir().join("thumbs")
}

/// Where the picture of `path` at `locator` (a page, a moment) goes. Keyed by path, modification time and locator,
/// so a changed file gets new pictures.
pub fn path_for(path: &Path, locator: Option<f64>, size: Size) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .hash(&mut h);
    locator.map(|l| (l * 10.0).round() as i64).hash(&mut h);
    dir()
        .join(size.dir())
        .join(format!("{:016x}.jpg", h.finish()))
}

/// The picture for a result, made now if it isn't there yet. None when the kind has none or it can't be made.
pub fn get(path: &Path, kind: FileKind, locator: Option<f64>, size: Size) -> Option<PathBuf> {
    let out = path_for(path, locator, size);
    if out.exists() {
        return Some(out);
    }
    std::fs::create_dir_all(out.parent()?).ok()?;
    let ok = match kind {
        FileKind::Image | FileKind::Screenshot => {
            scale(path, &out, size.pixels()) || thumbnailer(path, &out, None, size.pixels())
        }
        FileKind::Video => thumbnailer(path, &out, locator.or(Some(0.0)), size.pixels()),
        FileKind::Pdf => pdf_page(
            path,
            locator.map(|p| p.max(1.0) as u32).unwrap_or(1),
            &out,
            size.pixels(),
        ),
        FileKind::Doc | FileKind::Audio | FileKind::Code => false,
    };
    (ok && out.exists()).then_some(out)
}

/// A picture's width and height from its header, without decoding it: what the window lays a tile out by.
pub fn dimensions(path: &Path) -> Option<(u32, u32)> {
    image::image_dimensions(path).ok()
}

/// The small pictures a file's results will ask for, while its frames are still on disk: the photo, each video
/// keyframe at its moment, a PDF's first page and each rendered page. Failures are fine: the window makes what is
/// missing on demand.
pub fn pregenerate(path: &Path, kind: FileKind, chunks: &[Chunk]) {
    if std::fs::create_dir_all(dir().join(Size::Small.dir())).is_err() {
        return;
    }
    match kind {
        FileKind::Image | FileKind::Screenshot => {
            get(path, kind, None, Size::Small);
        }
        FileKind::Video => {
            for chunk in chunks {
                if let Embed::Image { path: frame, .. } = &chunk.embed {
                    let out = path_for(path, chunk.locator, Size::Small);
                    if !out.exists() {
                        scale(frame, &out, Size::Small.pixels());
                    }
                }
            }
        }
        FileKind::Pdf => {
            get(path, kind, None, Size::Small);
            for chunk in chunks {
                if let Embed::Image { path: page, .. } = &chunk.embed {
                    let out = path_for(path, chunk.locator, Size::Small);
                    if !out.exists() {
                        scale(page, &out, Size::Small.pixels());
                    }
                }
            }
        }
        _ => {}
    }
}

/// Decodes `src`, scales it to fit `max` px (never up) and writes a JPEG. EXIF orientation is applied so phone
/// photos stand the right way up.
fn scale(src: &Path, out: &Path, max: u32) -> bool {
    let Ok(reader) = image::ImageReader::open(src).and_then(|r| r.with_guessed_format()) else {
        return false;
    };
    let Ok(mut decoder) = reader.into_decoder() else {
        return false;
    };
    let orientation = image::ImageDecoder::orientation(&mut decoder).ok();
    let Ok(mut picture) = image::DynamicImage::from_decoder(decoder) else {
        return false;
    };
    if let Some(o) = orientation {
        picture.apply_orientation(o);
    }
    if picture.width() > max || picture.height() > max {
        picture = picture.resize(max, max, image::imageops::FilterType::Triangle);
    }
    let tmp = out.with_extension("tmp");
    let rgb = picture.to_rgb8();
    let ok = std::fs::File::create(&tmp)
        .map(std::io::BufWriter::new)
        .ok()
        .and_then(|mut w| {
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut w, 82)
                .encode_image(&rgb)
                .ok()
        })
        .is_some();
    ok && std::fs::rename(&tmp, out).is_ok()
}

fn thumbnailer(path: &Path, out: &Path, seconds: Option<f64>, max: u32) -> bool {
    let mut cmd = Command::new("ffmpegthumbnailer");
    cmd.arg("-i").arg(path).arg("-o").arg(out).args([
        "-s",
        &max.to_string(),
        "-c",
        "jpeg",
        "-q",
        "8",
    ]);
    if let Some(seconds) = seconds {
        let total = seconds.max(0.0) as u64;
        cmd.args([
            "-t",
            &format!(
                "{:02}:{:02}:{:02}",
                total / 3600,
                total % 3600 / 60,
                total % 60
            ),
        ]);
    }
    cmd.stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    cmd.status().map(|s| s.success()).unwrap_or(false)
}

fn pdf_page(path: &Path, page: u32, out: &Path, max: u32) -> bool {
    let stem = out.with_extension("");
    let ok = Command::new("pdftoppm")
        .args([
            "-jpeg",
            "-singlefile",
            "-scale-to",
            &max.to_string(),
            "-f",
            &page.to_string(),
            "-l",
            &page.to_string(),
        ])
        .arg(path)
        .arg(&stem)
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    ok && out.exists()
}

//! Small pictures for result rows, made once and kept in `~/.cache/dig/thumbs`: photos and video moments through
//! `ffmpegthumbnailer` (on every Omarchy install), PDF pages through `pdftoppm`. Documents and audio have none.

use std::path::{Path, PathBuf};
use std::process::Command;

use dig_core::FileKind;

pub const SIZE: u32 = 96;

fn dir() -> PathBuf {
    dig_core::paths::cache_dir().join("thumbs")
}

fn key(path: &Path, locator: Option<f64>) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    std::fs::metadata(path).ok().and_then(|m| m.modified().ok()).hash(&mut h);
    locator.map(|l| (l * 10.0) as i64).hash(&mut h);
    format!("{:016x}", h.finish())
}

/// The thumbnail for a result, or None when the kind has none or it can't be made.
pub fn get(path: &Path, kind: FileKind, locator: Option<f64>) -> Option<PathBuf> {
    let dir = dir();
    std::fs::create_dir_all(&dir).ok()?;
    let out = dir.join(format!("{}.png", key(path, locator)));
    if out.exists() {
        return Some(out);
    }
    let ok = match kind {
        FileKind::Image | FileKind::Screenshot => thumbnailer(path, &out, None),
        FileKind::Video => thumbnailer(path, &out, locator),
        FileKind::Pdf => {
            let page = locator.map(|p| p.max(1.0) as u32).unwrap_or(1);
            let stem = out.with_extension("");
            Command::new("pdftoppm")
                .args(["-png", "-singlefile", "-scale-to", &(SIZE * 2).to_string(), "-f"])
                .arg(page.to_string())
                .arg("-l")
                .arg(page.to_string())
                .arg(path)
                .arg(&stem)
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        }
        FileKind::Doc | FileKind::Audio | FileKind::Code => false,
    };
    (ok && out.exists()).then_some(out)
}

fn thumbnailer(path: &Path, out: &Path, seconds: Option<f64>) -> bool {
    let mut cmd = Command::new("ffmpegthumbnailer");
    cmd.arg("-i").arg(path).arg("-o").arg(out).args(["-s", &(SIZE * 2).to_string(), "-c", "png", "-q", "6"]);
    if let Some(seconds) = seconds {
        let total = seconds.max(0.0) as u64;
        cmd.args(["-t", &format!("{:02}:{:02}:{:02}", total / 3600, total % 3600 / 60, total % 60)]);
    }
    cmd.status().map(|s| s.success()).unwrap_or(false)
}

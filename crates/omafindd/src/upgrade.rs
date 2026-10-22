//! Picking up a package upgrade without a manual restart: pacman writes the new `omafindd` over the old one while
//! this process runs the old code. Watching the executable's folder (inotify) tells when the file was replaced.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use notify::Watcher;

/// Calls `on_replaced` once, a moment after the file at `exe` is replaced by another (an upgrade writes a new file and
/// renames it into place), so the rest of the package (the engine's libraries) has landed too. Keep the watcher.
pub fn watch(
    exe: PathBuf,
    on_replaced: impl Fn() + Send + Sync + 'static,
) -> Result<notify::RecommendedWatcher> {
    let inode = std::fs::metadata(&exe)
        .with_context(|| format!("reading {}", exe.display()))?
        .ino();
    let dir = exe.parent().context("no folder")?.to_path_buf();
    let fired = AtomicBool::new(false);
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let Ok(event) = event else {
            return;
        };
        if !event.paths.iter().any(|p| p == &exe)
            || !replaced(&exe, inode)
            || fired.swap(true, Ordering::SeqCst)
        {
            return;
        }
        std::thread::sleep(Duration::from_secs(2));
        on_replaced();
    })?;
    watcher.watch(&dir, notify::RecursiveMode::NonRecursive)?;
    Ok(watcher)
}

/// A different, complete, runnable file now sits at `exe`.
fn replaced(exe: &Path, inode: u64) -> bool {
    std::fs::metadata(exe).is_ok_and(|m| m.ino() != inode && m.len() > 0 && m.mode() & 0o111 != 0)
}

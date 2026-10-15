//! What Enter, Ctrl+Enter and Ctrl+C do with a result, with the programs Omarchy ships.

use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::Result;
use dig_core::{FileKind, Locator};

fn detached(program: &str, args: &[String]) -> Result<()> {
    Command::new("setsid")
        .arg("-f")
        .arg(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}

/// Opens the file at the place the result points to.
pub fn open(path: &Path, kind: FileKind, locator: Option<Locator>, query_words: &[String]) -> Result<()> {
    let file = path.to_string_lossy().into_owned();
    match (kind, locator) {
        (FileKind::Video | FileKind::Audio, Some(Locator::Seconds(s))) => detached("mpv", &[format!("--start={:.1}", s.max(0.0)), file]),
        (FileKind::Video | FileKind::Audio, _) => detached("mpv", &[file]),
        (FileKind::Pdf, locator) => {
            let mut args = Vec::new();
            if let Some(Locator::Page(page)) = locator {
                args.push(format!("--page-index={page}"));
            }
            if let Some(word) = query_words.iter().max_by_key(|w| w.len()) {
                args.push(format!("--find={word}"));
            }
            args.push(file);
            detached("evince", &args)
        }
        (FileKind::Image | FileKind::Screenshot, _) => detached("imv", &[file]),
        (FileKind::Code, _) => detached("omarchy-launch-editor", &[file]),
        _ => detached("xdg-open", &[file]),
    }
}

/// Shows the file in the file manager.
pub fn reveal(path: &Path) -> Result<()> {
    detached("nautilus", &["--select".into(), path.to_string_lossy().into_owned()])
}

/// Puts the file on the clipboard as a file (pastes into file managers and chats) and its path as text.
pub fn copy(path: &Path) -> Result<()> {
    let uri = format!("file://{}\n", path.to_string_lossy());
    let mut child = Command::new("wl-copy").args(["-t", "text/uri-list"]).stdin(Stdio::piped()).spawn()?;
    if let Some(stdin) = child.stdin.as_mut() {
        use std::io::Write;
        stdin.write_all(uri.as_bytes())?;
    }
    child.wait()?;
    Ok(())
}

//! What Enter, Ctrl+Enter and Ctrl+C do with a result, with the programs Omarchy ships.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::Result;
use omafind_core::{FileKind, Locator};

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
pub fn open(
    path: &Path,
    kind: FileKind,
    locator: Option<Locator>,
    query_words: &[String],
) -> Result<()> {
    let file = path.to_string_lossy().into_owned();
    match (kind, locator) {
        (FileKind::Video | FileKind::Audio, Some(Locator::Seconds(s))) => {
            detached("mpv", &[format!("--start={:.1}", s.max(0.0)), file])
        }
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
        (FileKind::Code, locator) => open_in_editor(
            path,
            match locator {
                Some(Locator::Line(l)) => l,
                _ => 1,
            },
        ),
        _ => detached("xdg-open", &[file]),
    }
}

/// Omarchy's default editor (`~/.local/state/omarchy/defaults/editor`), at a line where the editor can take one:
/// terminal editors through `omarchy-launch-tui` with `+line`, Zed/VS Code/Cursor with `file:line`.
fn open_in_editor(path: &Path, line: u32) -> Result<()> {
    let default = PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
        .join(".local/state/omarchy/defaults/editor");
    let editor = std::fs::read_to_string(default)
        .map(|s| s.trim().to_owned())
        .unwrap_or_else(|_| "nvim".into());
    let file = path.to_string_lossy().into_owned();
    let name = editor.rsplit('/').next().unwrap_or(&editor).to_owned();
    match name.as_str() {
        "nvim" | "vim" | "nano" | "micro" | "hx" | "helix" | "fresh" => {
            detached("omarchy-launch-tui", &[editor, format!("+{line}"), file])
        }
        "zeditor" | "zed" | "code" | "codium" | "cursor" | "windsurf" => {
            let target = format!("{file}:{line}");
            if name.starts_with("code")
                || name == "codium"
                || name == "cursor"
                || name == "windsurf"
            {
                detached(
                    "setsid",
                    &[
                        "uwsm-app".into(),
                        "--".into(),
                        editor,
                        "--goto".into(),
                        target,
                    ],
                )
            } else {
                detached("setsid", &["uwsm-app".into(), "--".into(), editor, target])
            }
        }
        _ => detached("omarchy-launch-editor", &[file]),
    }
}

/// Shows the file in the file manager.
pub fn reveal(path: &Path) -> Result<()> {
    detached(
        "nautilus",
        &["--select".into(), path.to_string_lossy().into_owned()],
    )
}

/// Puts the file on the clipboard as a file (pastes into file managers and chats) and its path as text.
pub fn copy(path: &Path) -> Result<()> {
    let uri = format!("file://{}\n", path.to_string_lossy());
    let mut child = Command::new("wl-copy")
        .args(["-t", "text/uri-list"])
        .stdin(Stdio::piped())
        .spawn()?;
    if let Some(stdin) = child.stdin.as_mut() {
        use std::io::Write;
        stdin.write_all(uri.as_bytes())?;
    }
    child.wait()?;
    Ok(())
}

/// Moves the file to the trash (`gio trash`, from glib, on every GNOME-stack install).
pub fn trash(path: &Path) -> Result<()> {
    let status = Command::new("gio")
        .arg("trash")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    anyhow::ensure!(status.success(), "gio trash failed for {}", path.display());
    Ok(())
}

pub fn copy_text(text: &str) -> Result<()> {
    let mut child = Command::new("wl-copy").stdin(Stdio::piped()).spawn()?;
    if let Some(stdin) = child.stdin.as_mut() {
        use std::io::Write;
        stdin.write_all(text.as_bytes())?;
    }
    child.wait()?;
    Ok(())
}

/// Lines `from..=to` of a text file, numbered from 1; nothing for a file that isn't UTF-8 text.
pub fn lines(path: &Path, from: usize, to: usize) -> Vec<(usize, String)> {
    use std::io::BufRead;
    let Ok(file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    std::io::BufReader::new(file)
        .lines()
        .enumerate()
        .skip(from.saturating_sub(1))
        .take(to.saturating_sub(from) + 1)
        .map_while(|(i, l)| l.ok().map(|l| (i + 1, l.replace('\t', "    "))))
        .collect()
}

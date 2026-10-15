//! What Enter, Ctrl+Enter and Ctrl+C do with a result, in the apps the desktop is set to use.

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

/// Opens the file at the place the result points to, in the app the desktop opens its type with (`xdg-mime`), so
/// someone's own choices win. The page, moment or line is passed only to apps known to take one; any other app gets
/// the file through `xdg-open`, from the top. Code always goes to Omarchy's default editor, which is what Omarchy
/// opens code with.
pub fn open(
    path: &Path,
    kind: FileKind,
    locator: Option<Locator>,
    query_words: &[String],
) -> Result<()> {
    let file = path.to_string_lossy().into_owned();
    if kind == FileKind::Code {
        let line = match locator {
            Some(Locator::Line(l)) => l,
            _ => 1,
        };
        return open_in_editor(path, line, &omarchy_editor());
    }
    let Some(app) = default_app(path) else {
        return detached("xdg-open", &[file]);
    };
    let program = app.program.clone();
    match (app.name(), locator) {
        ("mpv", Some(Locator::Seconds(s))) => {
            detached(&program, &[format!("--start={:.1}", s.max(0.0)), file])
        }
        ("vlc", Some(Locator::Seconds(s))) => {
            detached(&program, &[format!("--start-time={:.1}", s.max(0.0)), file])
        }
        ("evince" | "papers", locator) => {
            let mut args = Vec::new();
            if let Some(Locator::Page(page)) = locator {
                args.push(format!("--page-index={page}"));
            }
            if let Some(word) = longest(query_words) {
                args.push(format!("--find={word}"));
            }
            args.push(file);
            detached(&program, &args)
        }
        ("okular", locator) => {
            let mut args = Vec::new();
            if let Some(Locator::Page(page)) = locator {
                args.extend(["-p".into(), page.to_string()]);
            }
            if let Some(word) = longest(query_words) {
                args.extend(["--find".into(), word.clone()]);
            }
            args.push(file);
            detached(&program, &args)
        }
        ("zathura", Some(Locator::Page(page))) => {
            detached(&program, &["-P".into(), page.to_string(), file])
        }
        (name, Some(Locator::Line(line))) if app.terminal || EDITORS.contains(&name) => {
            open_in_editor(path, line, &program)
        }
        _ => detached("xdg-open", &[file]),
    }
}

fn longest(words: &[String]) -> Option<&String> {
    words.iter().max_by_key(|w| w.len())
}

/// Editors that take a line: terminal ones with `+line`, GUI ones with `file:line`.
const TERMINAL_EDITORS: &[&str] = &[
    "nvim", "vim", "vi", "nano", "micro", "hx", "helix", "fresh", "kak", "emacs",
];
const GUI_EDITORS: &[&str] = &["zeditor", "zed", "code", "codium", "cursor", "windsurf"];
const EDITORS: &[&str] = &[
    "nvim", "vim", "vi", "nano", "micro", "hx", "helix", "fresh", "kak", "emacs", "zeditor", "zed",
    "code", "codium", "cursor", "windsurf",
];

/// Omarchy's default editor (`~/.local/state/omarchy/defaults/editor`), nvim when none is set.
fn omarchy_editor() -> String {
    let default = PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
        .join(".local/state/omarchy/defaults/editor");
    std::fs::read_to_string(default)
        .map(|s| s.trim().to_owned())
        .unwrap_or_else(|_| "nvim".into())
}

/// `editor` at a line where it can take one: terminal editors through `omarchy-launch-tui` with `+line`,
/// Zed/VS Code/Cursor with `file:line`; anything else through `omarchy-launch-editor`, from the top.
fn open_in_editor(path: &Path, line: u32, editor: &str) -> Result<()> {
    let file = path.to_string_lossy().into_owned();
    let editor = editor.to_owned();
    let name = editor.rsplit('/').next().unwrap_or(&editor).to_owned();
    if TERMINAL_EDITORS.contains(&name.as_str()) {
        return detached("omarchy-launch-tui", &[editor, format!("+{line}"), file]);
    }
    if GUI_EDITORS.contains(&name.as_str()) {
        let target = format!("{file}:{line}");
        let mut args: Vec<String> = vec!["uwsm-app".into(), "--".into(), editor];
        if name != "zed" && name != "zeditor" {
            args.push("--goto".into());
        }
        args.push(target);
        return detached("setsid", &args);
    }
    detached("omarchy-launch-editor", &[file])
}

/// The app the desktop opens a file type with: its desktop entry's program and whether it runs in a terminal.
struct App {
    program: String,
    terminal: bool,
}

impl App {
    /// The program's file name, which is what decides the arguments it takes.
    fn name(&self) -> &str {
        self.program.rsplit('/').next().unwrap_or(&self.program)
    }
}

/// `xdg-mime`'s default app for `path`'s type, from its desktop entry. None when there is no default, or the
/// entry can't be read (then `xdg-open` decides).
fn default_app(path: &Path) -> Option<App> {
    let query = |args: &[&std::ffi::OsStr]| -> Option<String> {
        let out = Command::new("xdg-mime")
            .args(args)
            .stderr(Stdio::null())
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        (out.status.success() && !text.is_empty()).then_some(text)
    };
    let mime = query(&["query".as_ref(), "filetype".as_ref(), path.as_os_str()])?;
    app_for(&mime)
}

/// The desktop's default app for a MIME type.
fn app_for(mime: &str) -> Option<App> {
    let out = Command::new("xdg-mime")
        .args(["query", "default", mime])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let desktop = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if !out.status.success() || desktop.is_empty() {
        return None;
    }
    parse_entry(&desktop_entry(&desktop)?)
}

/// A desktop entry's program (the first word of `Exec` that isn't `env` or a VAR=value) and `Terminal`.
fn parse_entry(entry: &str) -> Option<App> {
    let mut program = None;
    let mut terminal = false;
    let mut in_main = false;
    for line in entry.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_main = line == "[Desktop Entry]";
        } else if in_main && let Some(exec) = line.strip_prefix("Exec=") {
            // The first word that isn't `env` or a VAR=value assignment.
            program = exec
                .split_whitespace()
                .map(|w| w.trim_matches('"'))
                .find(|w| *w != "env" && !w.contains('='))
                .map(str::to_owned);
        } else if in_main && let Some(value) = line.strip_prefix("Terminal=") {
            terminal = value.trim() == "true";
        }
    }
    Some(App {
        program: program?,
        terminal,
    })
}

/// A desktop entry by id, from the user's applications, then the system's (`$XDG_DATA_DIRS`).
fn desktop_entry(id: &str) -> Option<String> {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"));
    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    std::iter::once(data_home)
        .chain(data_dirs.split(':').map(PathBuf::from))
        .find_map(|dir| std::fs::read_to_string(dir.join("applications").join(id)).ok())
}

/// Shows the file in the desktop's file manager, selected where the file manager can do that, else its folder.
pub fn reveal(path: &Path) -> Result<()> {
    let file = path.to_string_lossy().into_owned();
    let folder = path
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| file.clone());
    match app_for("inode/directory") {
        Some(app) if matches!(app.name(), "nautilus" | "dolphin" | "nemo") => {
            let program = app.program.clone();
            if app.name() == "nemo" {
                // Nemo selects a file passed on its own.
                detached(&program, &[file])
            } else {
                detached(&program, &["--select".into(), file])
            }
        }
        _ => detached("xdg-open", &[folder]),
    }
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

/// Numbered lines, then the first and last line of the stretch they surround.
pub type Section = (Vec<(usize, String)>, usize, usize);

/// The lines of a plain text file holding characters `start..end` (a passage's locators), with `context` lines
/// either side and at most `most` lines of the passage itself: (lines, first line of the passage, last line).
/// None for files whose passages aren't offsets into the file as it is on disk (office documents, HTML).
pub fn text_section(
    path: &Path,
    start: usize,
    end: usize,
    context: usize,
    most: usize,
) -> Option<Section> {
    let ext = omafind_extract::classify::extension(path);
    if !omafind_extract::classify::PLAIN_TEXT.contains(&ext.as_str())
        || ext == "html"
        || ext == "htm"
    {
        return None;
    }
    // Read as the indexer did, so character offsets land on the same characters.
    let bytes = std::fs::read(path).ok()?;
    let max = omafind_extract::text::MAX_BYTES as usize;
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(max)]);
    let (mut line, mut first, mut last) = (1, None, None);
    for (i, ch) in text.chars().enumerate() {
        if i == start {
            first = Some(line);
        }
        if i + 1 >= end {
            last = Some(line);
            break;
        }
        if ch == '\n' {
            line += 1;
        }
    }
    let first = first?;
    let last = last.unwrap_or(line).max(first).min(first + most - 1);
    let from = first.saturating_sub(context).max(1);
    let lines: Vec<(usize, String)> = text
        .lines()
        .enumerate()
        .skip(from - 1)
        .take(last + context + 1 - from)
        .map(|(i, l)| (i + 1, l.replace('\t', "    ")))
        .collect();
    Some((lines, first, last))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_entries_name_their_program() {
        let app = parse_entry(
            "[Desktop Entry]\nName=Evince\nExec=env GDK_BACKEND=wayland /usr/bin/evince %U\n\n[Desktop Action new]\nExec=evince --new",
        )
        .unwrap();
        assert_eq!((app.name(), app.terminal), ("evince", false));
        let nvim = parse_entry("[Desktop Entry]\nExec=nvim %F\nTerminal=true").unwrap();
        assert_eq!((nvim.name(), nvim.terminal), ("nvim", true));
        assert!(parse_entry("[Desktop Entry]\nName=Nothing").is_none());
    }
}

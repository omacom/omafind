//! Walking the chosen folders and listing what to index, under `rules`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use ignore::WalkBuilder;
use omafind_core::FileKind;

use crate::rules;

/// A chosen folder. A code root is read as code (Phase 6); otherwise code projects inside only give up screenshots.
#[derive(Debug, Clone)]
pub struct Root {
    pub path: PathBuf,
    pub code: bool,
}

/// A file the crawler wants indexed.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub path: PathBuf,
    pub kind: FileKind,
    /// Seconds since the epoch.
    pub mtime: i64,
    pub size: u64,
}

/// What the crawl skipped and why, for `omafind status` and the log.
#[derive(Debug, Default)]
pub struct Skipped {
    pub hidden_or_ignored_dirs: usize,
    pub code_project_dirs: usize,
    pub huge_dirs: Vec<PathBuf>,
    pub secrets: usize,
    pub by_size: usize,
    pub by_type: usize,
}

/// Every indexable file under `root`, plus what was skipped. Hidden entries and `.gitignore`d files never appear.
pub fn crawl(root: &Root) -> (Vec<Candidate>, Skipped) {
    let mut out = Vec::new();
    let skipped = walk(root, |c| {
        out.push(c);
        true
    });
    (out, skipped)
}

/// Walks `root`, handing each candidate to `each` until it returns false. Returns what was skipped.
pub fn walk(root: &Root, mut each: impl FnMut(Candidate) -> bool) -> Skipped {
    let mut skipped = Skipped::default();
    let huge: Arc<Mutex<Vec<PathBuf>>> = Arc::new(Mutex::new(Vec::new()));
    let huge_in_filter = huge.clone();
    let ignored_dirs = Arc::new(AtomicUsize::new(0));
    let ignored_in_filter = ignored_dirs.clone();
    let mut code_dirs = 0usize;
    let root_is_code = root.code;
    // (depth, inside a code project) for the directories on the current path.
    let mut stack: Vec<(usize, bool)> = vec![(0, false)];

    // `.gitignore` rules apply only inside git repositories: a dotfiles repo in $HOME with a `*` ignore file
    // must not hide the whole home directory.
    let walker = WalkBuilder::new(&root.path)
        .hidden(true)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(true)
        .require_git(true)
        .follow_links(false)
        .same_file_system(true)
        .filter_entry(move |entry| {
            if !entry.file_type().is_some_and(|t| t.is_dir()) || entry.depth() == 0 {
                return true;
            }
            let name = entry.file_name().to_string_lossy();
            if rules::skip_dir(&name) || rules::is_system_tree(entry.path()) {
                ignored_in_filter.fetch_add(1, Ordering::Relaxed);
                return false;
            }
            if let Ok(dir) = std::fs::read_dir(entry.path())
                && dir.take(rules::MAX_DIR_ENTRIES + 1).count() > rules::MAX_DIR_ENTRIES
            {
                huge_in_filter
                    .lock()
                    .unwrap()
                    .push(entry.path().to_path_buf());
                return false;
            }
            true
        })
        .build();

    for entry in walker.flatten() {
        let depth = entry.depth();
        let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
        while stack.last().is_some_and(|(d, _)| *d >= depth && depth > 0) {
            stack.pop();
        }
        if is_dir {
            let parent_code = stack.last().map(|(_, c)| *c).unwrap_or(false);
            let code =
                parent_code || (!root_is_code && depth > 0 && rules::is_code_project(entry.path()));
            if code && !parent_code {
                code_dirs += 1;
            }
            stack.push((depth, code));
            continue;
        }
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let in_code = stack.last().map(|(_, c)| *c).unwrap_or(false);
        let path = entry.path();
        if rules::is_secret(path) {
            skipped.secrets += 1;
            continue;
        }
        let kind = if root_is_code {
            // A code root: every source file (and the repo's docs and config) as code, nothing else.
            if omafind_extract::code::is_code(path) {
                FileKind::Code
            } else {
                skipped.by_type += 1;
                continue;
            }
        } else {
            let Some(kind) = omafind_extract::classify::classify(path)
                .filter(|k| omafind_extract::supported(*k))
            else {
                skipped.by_type += 1;
                continue;
            };
            if in_code && kind != FileKind::Screenshot {
                // Inside somebody's project: README passages and asset images would swamp real documents.
                continue;
            }
            kind
        };
        let Ok(meta) = entry.metadata() else { continue };
        if !rules::size_ok(kind, meta.len()) {
            skipped.by_size += 1;
            continue;
        }
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        if !each(Candidate {
            path: path.to_path_buf(),
            kind,
            mtime,
            size: meta.len(),
        }) {
            break;
        }
    }
    skipped.huge_dirs = std::mem::take(&mut *huge.lock().unwrap());
    skipped.code_project_dirs = code_dirs;
    skipped.hidden_or_ignored_dirs = ignored_dirs.load(Ordering::Relaxed);
    skipped
}

/// `~`-relative display of a root.
pub fn display(path: &Path) -> String {
    omafind_core::paths::tilde(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crawl_applies_the_rules() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        std::fs::write(p.join("notes.md"), "hello").unwrap();
        std::fs::write(p.join("photo.jpg"), vec![0u8; 20_000]).unwrap();
        std::fs::write(p.join("tiny.png"), vec![0u8; 100]).unwrap();
        std::fs::write(p.join("server.pem"), "secret").unwrap();
        std::fs::create_dir_all(p.join(".hidden")).unwrap();
        std::fs::write(p.join(".hidden/x.md"), "no").unwrap();
        std::fs::create_dir_all(p.join("node_modules/pkg")).unwrap();
        std::fs::write(p.join("node_modules/pkg/README.md"), "no").unwrap();
        std::fs::create_dir_all(p.join("proj/docs")).unwrap();
        std::fs::write(p.join("proj/Cargo.toml"), "[package]").unwrap();
        std::fs::write(p.join("proj/docs/design.md"), "no: inside a project").unwrap();
        std::fs::write(p.join("proj/docs/Screenshot-1.png"), vec![0u8; 20_000]).unwrap();
        std::fs::create_dir_all(p.join(".git")).unwrap(); // a repo: its .gitignore counts
        std::fs::write(p.join(".gitignore"), "ignored.md\n").unwrap();
        std::fs::write(p.join("ignored.md"), "no").unwrap();

        let (files, skipped) = crawl(&Root {
            path: p.to_path_buf(),
            code: false,
        });
        let mut names: Vec<String> = files
            .iter()
            .map(|c| {
                c.path
                    .strip_prefix(p)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec!["notes.md", "photo.jpg", "proj/docs/Screenshot-1.png"]
        );
        assert_eq!(skipped.secrets, 1);
        assert_eq!(skipped.by_size, 1);
        assert_eq!(skipped.code_project_dirs, 1);
    }
}

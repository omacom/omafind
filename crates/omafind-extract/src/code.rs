//! Source code, for the opt-in code index: a file becomes stretches of whole lines of about `STRETCH_CHARS`
//! characters, each located by its first and last line. Function boundaries (tree-sitter) can refine the cuts
//! later; whole-line stretches already find "where is the retry with backoff" by what the code says.

use std::path::Path;

use anyhow::{Context, Result};
use omafind_core::{ChunkKind, prompts};

use crate::{Chunk, Embed};

pub const STRETCH_CHARS: usize = 1800;
/// Bigger files were made by a program (an amalgamation, data as code): left out.
pub const MAX_BYTES: u64 = 1 << 20;
pub const MAX_STRETCHES: usize = 300;

const EXTENSIONS: &[&str] = &[
    "rs", "py", "js", "mjs", "cjs", "ts", "tsx", "jsx", "go", "c", "h", "cc", "cpp", "cxx", "hpp",
    "hh", "java", "kt", "kts", "swift", "rb", "php", "lua", "sh", "bash", "zsh", "fish", "pl",
    "pm", "sql", "css", "scss", "less", "vue", "svelte", "zig", "nim", "ex", "exs", "erl", "hs",
    "ml", "mli", "scala", "cs", "fs", "dart", "r", "jl", "nix", "cmake", "mk", "toml", "yaml",
    "yml", "json", "jsonc", "md", "rst", "txt", "html", "xml", "proto", "graphql", "tf", "hcl",
    "qml", "el", "clj", "cljs", "groovy", "gradle", "m", "mm", "v", "sv", "vhd", "asm", "s", "ps1",
    "bat", "cfg", "ini", "conf", "env",
];
const NAMES: &[&str] = &[
    "Makefile",
    "Dockerfile",
    "Justfile",
    "Rakefile",
    "Gemfile",
    "CMakeLists.txt",
    "PKGBUILD",
    "Cargo.lock",
];

/// Whether this file is source (or a repo's docs and config), by extension or name.
pub fn is_code(path: &Path) -> bool {
    let ext = crate::classify::extension(path);
    if EXTENSIONS.contains(&ext.as_str()) {
        return true;
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    NAMES.contains(&name) || (ext.is_empty() && !name.starts_with('.') && looks_like_script(path))
}

fn looks_like_script(path: &Path) -> bool {
    let mut head = [0u8; 2];
    std::fs::File::open(path)
        .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut head))
        .map(|_| &head == b"#!")
        .unwrap_or(false)
}

/// The stretches of `path`, titled with its last path components so a search for "the config loader" lands on
/// `config.rs` before `lib.rs`.
pub fn extract(path: &Path) -> Result<Vec<Chunk>> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    if bytes.len() as u64 > MAX_BYTES {
        anyhow::bail!("bigger than {} KB: generated, not read", MAX_BYTES / 1024);
    }
    if bytes.iter().take(8192).any(|&b| b == 0) {
        anyhow::bail!("binary");
    }
    let text = String::from_utf8_lossy(&bytes);
    let title: String = path
        .components()
        .rev()
        .take(3)
        .filter_map(|c| c.as_os_str().to_str())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("/");
    Ok(stretches(&text)
        .into_iter()
        .take(MAX_STRETCHES)
        .map(|(first, last, body)| Chunk {
            kind: ChunkKind::Lines,
            locator: Some(first as f64),
            locator_end: Some(last as f64),
            embed: if crate::text::mostly_numeric(&body) {
                Embed::None
            } else {
                Embed::Text(prompts::document(Some(&title), &body))
            },
            text: body,
        })
        .collect())
}

/// (first line, last line, text) stretches of whole lines, from 1. A line longer than a stretch is one by itself.
pub fn stretches(text: &str) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    let mut start_line = 1usize;
    let mut buffer = String::new();
    let mut line_no = 0usize;
    for line in text.lines() {
        line_no += 1;
        if !buffer.is_empty() && buffer.len() + line.len() + 1 > STRETCH_CHARS {
            if !buffer.trim().is_empty() {
                out.push((start_line, line_no - 1, std::mem::take(&mut buffer)));
            } else {
                buffer.clear();
            }
            start_line = line_no;
        }
        if !buffer.is_empty() {
            buffer.push('\n');
        }
        buffer.push_str(line);
    }
    if !buffer.trim().is_empty() {
        out.push((start_line, line_no.max(start_line), buffer));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stretches_keep_whole_lines() {
        let text: String = (1..=200)
            .map(|i| format!("fn f{i}() {{ return {i}; }}"))
            .collect::<Vec<_>>()
            .join("\n");
        let s = stretches(&text);
        assert!(s.len() > 1);
        assert_eq!(s[0].0, 1);
        for (i, (first, last, body)) in s.iter().enumerate() {
            assert!(
                body.len() <= STRETCH_CHARS,
                "stretch {i} is {} chars",
                body.len()
            );
            assert_eq!(body.lines().count(), last - first + 1);
            if i > 0 {
                assert_eq!(*first, s[i - 1].1 + 1);
            }
        }
        assert!(
            is_code(Path::new("/p/main.rs"))
                && is_code(Path::new("/p/Makefile"))
                && !is_code(Path::new("/p/photo.jpg"))
        );
    }
}

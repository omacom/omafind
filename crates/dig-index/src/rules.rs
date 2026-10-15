//! What gets read and what gets skipped. Only chosen folders; inside them, nothing hidden, nothing a `.gitignore`
//! hides, no caches or build output, no keys or secrets, and inside a code project only screenshots (code is its own
//! opt-in index). Files are never modified, moved or uploaded.

use std::path::Path;

use dig_core::FileKind;

/// Directory names that are never read, wherever they are below a root: caches, build output, package trees,
/// trash. Names people use for real folders (`dev`, `tmp`, `bin`, `out`) are deliberately not here.
const SKIP_DIRS: &[&str] = &[
    "node_modules", "bower_components", "target", "build", "dist", "__pycache__", "venv", "Pods", "DerivedData",
    "cache", "Cache", "caches", "Caches", "Trash", "lost+found", "steamapps", "thumbnails", "Thumbnails", "icons",
    "squashfs-root", "AppDir", "rootfs", "sysroot", "initramfs",
];

/// Marks of a code project: inside one, only screenshots are read unless the folder is a code root.
const PROJECT_MARKERS: &[&str] = &[
    ".git", "Cargo.toml", "package.json", "pyproject.toml", "setup.py", "go.mod", "CMakeLists.txt", "Makefile", "build.gradle",
    "pom.xml", "Gemfile", "mix.exs", "composer.json", "Package.swift", "pubspec.yaml", "deno.json", "flake.nix",
];

/// File names and patterns that hold secrets or keys.
const SECRET_NAMES: &[&str] = &[
    "id_rsa", "id_dsa", "id_ecdsa", "id_ed25519", "known_hosts", "authorized_keys", "wallet.dat", "credentials", "credentials.json",
    "secrets", "secrets.json", "secrets.yaml", "secrets.yml", "token", "tokens", "passwords", "passwords.txt",
];
const SECRET_EXTENSIONS: &[&str] = &["pem", "key", "p12", "pfx", "kdbx", "kdb", "gpg", "pgp", "asc", "crt", "cer", "der", "jks", "keystore", "ovpn", "keychain", "agilekeychain", "opvault", "1pif", "psafe3"];
const SECRET_FRAGMENTS: &[&str] = &["password", "passwd", "secret", "private_key", "privatekey", ".env"];

/// Bytes: below this an image is an icon or thumbnail; above these a file is left for later phases.
pub const MIN_IMAGE_BYTES: u64 = 8 * 1024;
pub const MAX_IMAGE_BYTES: u64 = 200 << 20;
pub const MAX_DOC_BYTES: u64 = 64 << 20;
pub const MAX_PDF_BYTES: u64 = 512 << 20;
pub const MAX_MEDIA_BYTES: u64 = 16 << 30;
/// A directory with more direct entries than this looks like a dataset or a dump: skipped, and said so.
pub const MAX_DIR_ENTRIES: usize = 3000;

pub fn skip_dir(name: &str) -> bool {
    name.starts_with('.') || SKIP_DIRS.contains(&name)
}

pub fn is_code_project(dir: &Path) -> bool {
    PROJECT_MARKERS.iter().any(|m| dir.join(m).exists())
}

/// An unpacked system image or install tree (an AppImage's contents, a container root, a firmware dump): thousands
/// of files nobody looks for. `usr` with `bin` or `lib` beside it says so.
pub fn is_system_tree(dir: &Path) -> bool {
    dir.join("usr").is_dir() && (dir.join("bin").exists() || dir.join("lib").exists() || dir.join("etc").is_dir())
}

pub fn is_secret(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    let lower = name.to_ascii_lowercase();
    if SECRET_NAMES.contains(&lower.as_str()) || lower.starts_with("id_rsa") || lower.starts_with("id_ed25519") || lower.starts_with("id_ecdsa") {
        return true;
    }
    let ext = dig_extract::classify::extension(path);
    if SECRET_EXTENSIONS.contains(&ext.as_str()) {
        return true;
    }
    SECRET_FRAGMENTS.iter().any(|f| lower.contains(f))
}

/// Whether a file of `kind` and `size` is worth reading.
pub fn size_ok(kind: FileKind, size: u64) -> bool {
    match kind {
        FileKind::Screenshot | FileKind::Image => (MIN_IMAGE_BYTES..=MAX_IMAGE_BYTES).contains(&size),
        FileKind::Doc | FileKind::Code => size > 0 && size <= MAX_DOC_BYTES,
        FileKind::Pdf => size > 0 && size <= MAX_PDF_BYTES,
        FileKind::Audio | FileKind::Video => size > 0 && size <= MAX_MEDIA_BYTES,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_and_skips() {
        assert!(is_secret(Path::new("/h/.ssh/id_rsa")));
        assert!(is_secret(Path::new("/h/certs/server.pem")));
        assert!(is_secret(Path::new("/h/work/.env.production")));
        assert!(is_secret(Path::new("/h/passwords-2024.txt")));
        assert!(!is_secret(Path::new("/h/notes/meeting.md")));
        assert!(skip_dir("node_modules") && skip_dir(".cache") && !skip_dir("Documents"));
        assert!(size_ok(FileKind::Image, 50_000) && !size_ok(FileKind::Image, 1_000));
    }
}

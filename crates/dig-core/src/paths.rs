//! Where Dig keeps things, per the XDG base directory spec.

use std::path::PathBuf;

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(fallback))
}

/// `~/.local/share/dig`: models, engine, the index.
pub fn data_dir() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share").join("dig")
}

/// `~/.cache/dig`: thumbnails, scratch audio.
pub fn cache_dir() -> PathBuf {
    xdg("XDG_CACHE_HOME", ".cache").join("dig")
}

/// `~/.config/dig`: config.toml.
pub fn config_dir() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join("dig")
}

/// `$XDG_RUNTIME_DIR`, else `/run/user/<uid>` when it exists (a shell without the variable, a service), else /tmp:
/// where the daemon's socket is.
pub fn runtime_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        return PathBuf::from(dir);
    }
    let uid = std::fs::metadata(home())
        .map(|m| std::os::unix::fs::MetadataExt::uid(&m))
        .unwrap_or(0);
    let run = PathBuf::from(format!("/run/user/{uid}"));
    if run.is_dir() {
        run
    } else {
        PathBuf::from("/tmp")
    }
}

/// The main index.
pub fn index_path() -> PathBuf {
    data_dir().join("index.sqlite")
}

/// The code index (separate: code never appears in normal results).
pub fn code_index_path() -> PathBuf {
    data_dir().join("code.sqlite")
}

/// `~` shortened, for display.
pub fn tilde(path: &std::path::Path) -> String {
    let home = home();
    match path.strip_prefix(&home) {
        Ok(rest) if !home.as_os_str().is_empty() => format!("~/{}", rest.display()),
        _ => path.display().to_string(),
    }
}

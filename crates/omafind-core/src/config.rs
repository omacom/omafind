//! `~/.config/omafind/config.toml`: which folders to index and how to behave on battery.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub index: IndexConfig,
    pub power: PowerConfig,
    pub search: SearchConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct IndexConfig {
    /// Folders to index (`~` allowed).
    pub folders: Vec<String>,
    /// Folders to index as code, searched with a `code:` prefix.
    pub code: Vec<String>,
    /// Folders inside the above to leave out.
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PowerConfig {
    /// Keep indexing on battery (the default), but not while the power profile is power-saver.
    pub pause_in_power_saver: bool,
    /// Only index while a charger is connected.
    pub wait_for_ac: bool,
}

impl Default for PowerConfig {
    fn default() -> Self {
        Self {
            pause_in_power_saver: true,
            wait_for_ac: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SearchConfig {
    /// Minutes without a search after which the daemon unloads the query encoder (0 = keep it loaded).
    pub unload_after_minutes: u64,
    /// `gpu` (default when one is found), or `cpu`.
    pub device: String,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            unload_after_minutes: 10,
            device: "gpu".into(),
        }
    }
}

pub fn path() -> PathBuf {
    crate::paths::config_dir().join("config.toml")
}

impl Config {
    /// The config, or the defaults when there is none yet.
    pub fn load() -> Result<Self> {
        let path = path();
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn save(&self) -> Result<()> {
        let path = path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = format!(
            "# Omafind: local semantic file search. `omafind index <folders>` edits [index]; the rest is by hand.\n\n{}",
            toml::to_string_pretty(self)?
        );
        std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))
    }

    /// The folders as absolute paths (missing ones dropped).
    pub fn folders(&self) -> Vec<PathBuf> {
        self.index
            .folders
            .iter()
            .map(|f| expand(f))
            .filter(|p| p.is_dir())
            .collect()
    }

    pub fn code_roots(&self) -> Vec<PathBuf> {
        self.index
            .code
            .iter()
            .map(|f| expand(f))
            .filter(|p| p.is_dir())
            .collect()
    }

    pub fn excludes(&self) -> Vec<PathBuf> {
        self.index.exclude.iter().map(|f| expand(f)).collect()
    }

    /// Replaces the folder lists with these (stored `~`-relative).
    pub fn set_folders(&mut self, folders: &[PathBuf], code: &[PathBuf]) {
        self.index.folders = folders.iter().map(|p| crate::paths::tilde(p)).collect();
        self.index.code = code.iter().map(|p| crate::paths::tilde(p)).collect();
    }
}

/// `~/x` → `/home/me/x`.
pub fn expand(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(rest)
    } else if path == "~" {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
    } else {
        PathBuf::from(path)
    }
}

/// Whether `path` is under one of `excludes`.
pub fn is_excluded(path: &Path, excludes: &[PathBuf]) -> bool {
    excludes.iter().any(|e| path.starts_with(e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_round_trip() {
        let config: Config = toml::from_str("[index]\nfolders = [\"~/Pictures\"]\n").unwrap();
        assert_eq!(config.index.folders, vec!["~/Pictures"]);
        assert!(config.power.pause_in_power_saver && !config.power.wait_for_ac);
        assert_eq!(config.search.unload_after_minutes, 10);
        let text = toml::to_string_pretty(&config).unwrap();
        let again: Config = toml::from_str(&text).unwrap();
        assert_eq!(again, config);
    }
}

//! Setting Omafind up, step by step, without asking: the callers (`omafind setup` with gum, the window's first-run
//! screen) do the asking. Every step is safe to repeat.

pub mod download;
pub mod hypr;

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use omafind_core::config::Config;

/// The folders worth offering, with what they hold (surveyed side by side, bounded).
pub fn candidate_folders(config: &Config) -> Vec<(omafind_index::survey::Survey, bool)> {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let names = [
        "Pictures",
        "Documents",
        "Downloads",
        "Videos",
        "Music",
        "Desktop",
        "Notes",
        "Books",
        "Screenshots",
    ];
    let chosen = config.folders();
    let mut paths: Vec<PathBuf> = names
        .iter()
        .map(|n| home.join(n))
        .filter(|p| p.is_dir())
        .collect();
    for extra in &chosen {
        if !paths.contains(extra) {
            paths.push(extra.clone());
        }
    }
    let surveys = omafind_index::survey::survey_all(&paths, false);
    surveys
        .into_iter()
        .map(|s| {
            let default_on = chosen.is_empty()
                && matches!(
                    s.path.file_name().and_then(|n| n.to_str()),
                    Some("Pictures" | "Documents" | "Screenshots")
                );
            let on = default_on || chosen.contains(&s.path);
            (s, on)
        })
        .collect()
}

/// Folders that look like where code lives, for the optional code index.
pub fn candidate_code_folders(config: &Config) -> Vec<(omafind_index::survey::Survey, bool)> {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let names = [
        "dev", "src", "code", "projects", "Projects", "repos", "work", "Code",
    ];
    let chosen = config.code_roots();
    let mut paths: Vec<PathBuf> = names
        .iter()
        .map(|n| home.join(n))
        .filter(|p| p.is_dir())
        .collect();
    for extra in &chosen {
        if !paths.contains(extra) {
            paths.push(extra.clone());
        }
    }
    let surveys = omafind_index::survey::survey_all(&paths, true);
    surveys
        .into_iter()
        .map(|s| {
            let on = chosen.contains(&s.path);
            (s, on)
        })
        .collect()
}

/// Saves the folders and tells the daemon, when it runs.
pub fn set_folders(folders: &[PathBuf], code: &[PathBuf]) -> Result<()> {
    let mut config = Config::load().unwrap_or_default();
    config.set_folders(folders, code);
    config.save()?;
    if omafind_client::Client::is_running() {
        let mut client = omafind_client::Client::connect()?;
        client.simple(&omafind_client::Request::SetRoots {
            folders: folders.to_vec(),
            code: code.to_vec(),
        })?;
    }
    Ok(())
}

/// The user unit, pointing at this build's `omafindd`, unless a packaged unit exists in /usr/lib/systemd/user.
pub fn install_unit(omafindd: &Path) -> Result<()> {
    if Path::new("/usr/lib/systemd/user/omafindd.service").exists() {
        return Ok(());
    }
    let unit = include_str!("../../../systemd/omafindd.service")
        .replace("/usr/bin/omafindd", &omafindd.to_string_lossy());
    let dir = omafind_core::paths::config_dir()
        .parent()
        .context("config dir")?
        .join("systemd/user");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("omafindd.service"), unit)?;
    Ok(())
}

/// Enables and starts the daemon with systemd. `omafindd_beside` is the daemon binary next to the caller.
pub fn start_daemon(omafindd_beside: &Path) -> Result<()> {
    install_unit(omafindd_beside)?;
    let ok = Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
        && Command::new("systemctl")
            .args(["--user", "enable", "--now", "omafindd"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
    anyhow::ensure!(ok, "systemctl --user enable --now omafindd failed");
    Ok(())
}

pub fn daemon_enabled() -> bool {
    Command::new("systemctl")
        .args(["--user", "is-enabled", "omafindd"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "enabled")
        .unwrap_or(false)
}

/// Adds the keybinding, the window rules and the autostart line (`~/.config/hypr/omafind.lua`), and reloads Hyprland
/// when anything changed.
pub fn install_hyprland(binding: bool, rules: bool, autostart: bool) -> Result<bool> {
    if !(binding || rules || autostart) {
        return Ok(false);
    }
    let changed = hypr::install(hypr::Parts {
        binding,
        rules,
        autostart,
    })?;
    if changed {
        hypr::reload();
    }
    Ok(changed)
}

/// Starts `omafind-app` (beside the caller) if no window exists.
pub fn start_app(app_beside: &Path) -> Result<bool> {
    let has_window = Command::new("hyprctl")
        .args(["-j", "clients"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("\"class\": \"omafind\""))
        .unwrap_or(false);
    if has_window {
        return Ok(false);
    }
    let program = if app_beside.exists() {
        app_beside.to_path_buf()
    } else {
        PathBuf::from("omafind-app")
    };
    Command::new("setsid")
        .arg("-f")
        .arg(program)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("starting omafind-app")?;
    Ok(true)
}

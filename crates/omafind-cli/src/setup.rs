//! `omafind setup`: models, folders, the daemon, the window, the key. Each step asks unless `--yes` (or there is no
//! terminal to ask on), and `--dry-run` asks but changes nothing.

use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result};
use omafind_core::config::Config;
use omafind_core::paths::tilde;

use omafind_setup::{download, hypr};

use crate::ui;

const WHAT: &str = "Setup";

pub struct Options {
    pub yes: bool,
    pub dry_run: bool,
    pub folders: Vec<PathBuf>,
    pub code: Vec<PathBuf>,
    pub text_only: bool,
}

/// Fetches one model file with a progress bar, then a spinner while its checksum is checked.
fn fetch(models: &std::path::Path, file: &download::ModelFile) -> Result<()> {
    let bar = cliclack::progress_bar(file.bytes).with_download_template();
    bar.start(file.name);
    let check = cliclack::spinner();
    let result = download::fetch_with(models, file, &mut |step| match step {
        download::Progress::Copying => {
            bar.set_message(format!("{} (from the Hugging Face cache)", file.name))
        }
        download::Progress::Downloading { done, .. } => bar.set_position(done),
        download::Progress::Checking => {
            bar.stop(format!("{} downloaded", file.name));
            check.start(format!("Checking {}", file.name));
        }
    });
    match &result {
        Ok(()) => check.stop(format!("{} ({} MB)", file.name, file.bytes / 1_000_000)),
        Err(error) => {
            bar.error(format!("{}: {error:#}", file.name));
            check.error(format!("{}: {error:#}", file.name));
        }
    }
    result
}

/// (path, label, hint) rows for a folder picker, and the ones to preselect.
fn folder_items(
    candidates: &[(omafind_index::survey::Survey, bool)],
) -> (Vec<(PathBuf, String, String)>, Vec<PathBuf>) {
    let items = candidates
        .iter()
        .map(|(s, _)| (s.path.clone(), tilde(&s.path), s.summary()))
        .collect();
    let on = candidates
        .iter()
        .filter(|(_, on)| *on)
        .map(|(s, _)| s.path.clone())
        .collect();
    (items, on)
}

pub fn run(options: Options) -> Result<()> {
    let dry = options.dry_run;
    cliclack::intro(if dry {
        "Omafind setup (dry run)"
    } else {
        "Omafind setup"
    })?;

    // 1. Models.
    let models = omafind_core::model::dir();
    let wanted: Vec<&download::ModelFile> = if options.text_only {
        vec![&download::TEXT]
    } else {
        vec![&download::TEXT, &download::MEDIA]
    };
    let missing: Vec<&download::ModelFile> = wanted
        .into_iter()
        .filter(|f| !download::present(&models, f))
        .collect();
    if missing.is_empty() {
        cliclack::log::success(format!("Model files are in {}", tilde(&models)))?;
    } else {
        let mb: u64 = missing.iter().map(|f| f.bytes).sum::<u64>() / 1_000_000;
        let question = format!(
            "Download the EmbeddingGemma 2 model files ({mb} MB) into {}?",
            tilde(&models)
        );
        if ui::confirm(question, options.yes, WHAT)? && !dry {
            for file in &missing {
                fetch(&models, file)?;
            }
        }
    }

    // 2. Folders.
    let mut config = Config::load().unwrap_or_default();
    let folders: Vec<PathBuf> = if !options.folders.is_empty() {
        options
            .folders
            .iter()
            .map(|p| omafind_index::indexer::resolve_root(p))
            .collect::<Result<_>>()?
    } else {
        let looking = cliclack::spinner();
        looking.start("Looking at your folders");
        let candidates = omafind_setup::candidate_folders(&config);
        looking.stop(format!("{} folders worth indexing", candidates.len()));
        let (items, on) = folder_items(&candidates);
        ui::choose(
            "Which folders should Omafind index?",
            &items,
            on,
            options.yes,
            WHAT,
        )?
    };
    let code: Vec<PathBuf> = if !options.code.is_empty() {
        options
            .code
            .iter()
            .map(|p| omafind_index::indexer::resolve_root(p))
            .collect::<Result<_>>()?
    } else if ui::interactive(options.yes) {
        let candidates = omafind_setup::candidate_code_folders(&config);
        if candidates.is_empty() {
            Vec::new()
        } else {
            let (items, on) = folder_items(&candidates);
            ui::choose(
                "Index code too? It is searched on its own, with `code:` (enter to skip)",
                &items,
                on,
                options.yes,
                WHAT,
            )?
        }
    } else {
        config.code_roots()
    };
    if folders.is_empty() && code.is_empty() {
        cliclack::log::warning(
            "No folders chosen: `omafind index ~/Pictures ~/Documents` picks them later",
        )?;
    } else {
        config.set_folders(&folders, &code);
        let list = folders
            .iter()
            .chain(code.iter())
            .map(|p| tilde(p))
            .collect::<Vec<_>>()
            .join(", ");
        cliclack::log::success(format!("Indexing {list}"))?;
        if !dry {
            config.save()?;
        }
    }

    // 3. The daemon.
    if ui::confirm(
        "Start the indexing daemon now and at every login?",
        options.yes,
        WHAT,
    )? && !dry
    {
        let starting = cliclack::spinner();
        starting.start("Starting omafindd (systemd user service)");
        match omafind_setup::start_daemon(&std::env::current_exe()?.with_file_name("omafindd")) {
            Ok(()) => starting.stop("omafindd is running"),
            Err(error) => starting.error(format!(
                "Couldn't start omafindd with systemd ({error:#}); run `omafindd` to see why"
            )),
        }
        if omafind_client::Client::is_running()
            && let Ok(mut client) = omafind_client::Client::connect()
        {
            let _ = client.simple(&omafind_client::Request::SetRoots {
                folders: folders.clone(),
                code: code.clone(),
            });
        }
    }

    // 4. Hyprland: keybinding, window rules, autostart, in ~/.config/hypr/omafind.lua.
    let have = hypr::installed();
    let parts = [
        (
            "binding",
            have.binding,
            "SUPER + CTRL + G opens the search window",
        ),
        (
            "rules",
            have.rules,
            "Window rules: floating, centred, in its own special workspace",
        ),
        (
            "autostart",
            have.autostart,
            "Start the window with the session, so it opens instantly",
        ),
    ];
    for (_, _, label) in parts.iter().filter(|(_, present, _)| *present) {
        cliclack::log::success(format!("{label} (already in ~/.config/hypr/omafind.lua)"))?;
    }
    let absent: Vec<(&str, String, String)> = parts
        .iter()
        .filter(|(_, present, _)| !*present)
        .map(|(key, _, label)| (*key, label.to_string(), String::new()))
        .collect();
    if !absent.is_empty() {
        let on = absent.iter().map(|(k, _, _)| *k).collect();
        let chosen = ui::choose(
            "Add to ~/.config/hypr/omafind.lua:",
            &absent,
            on,
            options.yes,
            WHAT,
        )?;
        if !dry {
            let pick = |key: &str| chosen.contains(&key);
            if omafind_setup::install_hyprland(pick("binding"), pick("rules"), pick("autostart"))? {
                cliclack::log::success("Hyprland config written and reloaded")?;
            }
        }
    }

    // 5. The window, resident from now on.
    if !dry && omafind_setup::start_app(&std::env::current_exe()?.with_file_name("omafind-app"))? {
        cliclack::log::success("omafind-app started")?;
    }
    cliclack::outro(
        "Done. SUPER + CTRL + G searches; `omafind status` and `omafind doctor` say how it's going.",
    )?;
    Ok(())
}

/// `omafind uninstall`: stops the daemon, removes the config lines, and (asked) the data.
pub fn uninstall(yes: bool, keep_data: bool) -> Result<()> {
    cliclack::intro("Omafind uninstall")?;
    let _ = Command::new("systemctl")
        .args(["--user", "disable", "--now", "omafindd"])
        .output();
    let unit = omafind_core::paths::config_dir()
        .parent()
        .context("config dir")?
        .join("systemd/user/omafindd.service");
    if unit.exists() {
        std::fs::remove_file(&unit)?;
        let _ = Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .status();
        cliclack::log::success("Stopped omafindd and removed its service")?;
    }
    let _ = Command::new("pkill").args(["-x", "omafind-app"]).status();
    for path in hypr::uninstall()? {
        cliclack::log::success(format!("Removed Omafind's lines from {}", tilde(&path)))?;
    }
    hypr::reload();
    let dirs = [
        omafind_core::paths::data_dir(),
        omafind_core::paths::cache_dir(),
        omafind_core::paths::config_dir(),
    ];
    let bytes: u64 = dirs.iter().map(|d| size_of(d)).sum();
    if !keep_data
        && bytes > 0
        && ui::confirm(
            format!(
                "Delete the index, thumbnails, config and model files ({} MB)?",
                bytes / 1_000_000
            ),
            yes,
            "Uninstall",
        )?
    {
        for dir in &dirs {
            if dir.exists() {
                std::fs::remove_dir_all(dir)?;
                cliclack::log::success(format!("Removed {}", tilde(dir)))?;
            }
        }
    }
    cliclack::outro("Omafind is gone. The binaries are left for your package manager.")?;
    Ok(())
}

/// Bytes under `dir`, following no links.
fn size_of(dir: &std::path::Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => size_of(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

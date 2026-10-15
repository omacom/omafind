//! `omafind setup`: models, folders, the daemon, the window, the key. Each step asks (with `gum`) unless `--yes`.

use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result};
use omafind_core::config::Config;

use omafind_setup::{download, hypr};

pub struct Options {
    pub yes: bool,
    pub dry_run: bool,
    pub folders: Vec<PathBuf>,
    pub code: Vec<PathBuf>,
    pub text_only: bool,
}

fn have(cmd: &str) -> bool {
    Command::new("which")
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Yes/no: `gum confirm`, else stdin.
fn confirm(question: &str, yes: bool) -> bool {
    if yes {
        eprintln!("{question} yes");
        return true;
    }
    if have("gum") {
        return Command::new("gum")
            .args(["confirm", "--default=yes", question])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
    }
    eprint!("{question} [Y/n] ");
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).ok();
    !line.trim().to_lowercase().starts_with('n')
}

/// A multi-choice: `gum choose --no-limit`, else all preselected.
fn choose(header: &str, items: &[(String, bool)], yes: bool) -> Vec<String> {
    if yes || !have("gum") {
        return items
            .iter()
            .filter(|(_, on)| *on)
            .map(|(label, _)| label.clone())
            .collect();
    }
    let selected: Vec<&str> = items
        .iter()
        .filter(|(_, on)| *on)
        .map(|(l, _)| l.as_str())
        .collect();
    let output = Command::new("gum")
        .args(["choose", "--no-limit", "--header", header])
        .args(selected.iter().flat_map(|s| ["--selected", s]))
        .args(items.iter().map(|(l, _)| l.as_str()))
        .output();
    match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

pub fn run(options: Options) -> Result<()> {
    let dry = options.dry_run;
    eprintln!("Omafind setup");

    // 1. Models.
    let models = omafind_core::model::dir();
    let wanted: Vec<&download::ModelFile> = if options.text_only {
        vec![&download::TEXT]
    } else {
        vec![&download::TEXT, &download::MEDIA]
    };
    let missing: Vec<&&download::ModelFile> = wanted
        .iter()
        .filter(|f| !download::present(&models, f))
        .collect();
    if missing.is_empty() {
        eprintln!(
            "• model files present in {}",
            omafind_core::paths::tilde(&models)
        );
    } else {
        let mb: u64 = missing.iter().map(|f| f.bytes).sum::<u64>() / 1_000_000;
        if confirm(
            &format!(
                "Download the EmbeddingGemma 2 model files ({mb} MB) into {}?",
                omafind_core::paths::tilde(&models)
            ),
            options.yes,
        ) && !dry
        {
            for file in &missing {
                download::fetch(&models, file)?;
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
        eprintln!("• looking at your folders…");
        let candidates = omafind_setup::candidate_folders(&config);
        let labelled: Vec<(String, bool)> = candidates
            .iter()
            .map(|(s, on)| {
                (
                    format!(
                        "{:<14} {}",
                        omafind_core::paths::tilde(&s.path),
                        s.summary()
                    ),
                    *on,
                )
            })
            .collect();
        let chosen = choose(
            "Which folders should Omafind index? (space to toggle, enter to confirm)",
            &labelled,
            options.yes,
        );
        candidates
            .into_iter()
            .map(|(s, _)| s.path)
            .filter(|p| {
                chosen
                    .iter()
                    .any(|c| c.starts_with(&omafind_core::paths::tilde(p)))
            })
            .collect()
    };
    let code: Vec<PathBuf> = if !options.code.is_empty() {
        options
            .code
            .iter()
            .map(|p| omafind_index::indexer::resolve_root(p))
            .collect::<Result<_>>()?
    } else if !options.yes && have("gum") {
        let candidates = omafind_setup::candidate_code_folders(&config);
        if candidates.is_empty() {
            Vec::new()
        } else {
            let labelled: Vec<(String, bool)> = candidates
                .iter()
                .map(|(s, on)| {
                    (
                        format!(
                            "{:<14} {}",
                            omafind_core::paths::tilde(&s.path),
                            s.summary()
                        ),
                        *on,
                    )
                })
                .collect();
            let chosen = choose(
                "Index code too? Searched separately with a `code:` prefix (enter to skip)",
                &labelled,
                false,
            );
            candidates
                .into_iter()
                .map(|(s, _)| s.path)
                .filter(|p| {
                    chosen
                        .iter()
                        .any(|c| c.starts_with(&omafind_core::paths::tilde(p)))
                })
                .collect()
        }
    } else {
        config.code_roots()
    };
    if folders.is_empty() && code.is_empty() {
        eprintln!("• no folders chosen; `omafind index ~/Pictures ~/Documents` picks them later");
    } else {
        config.set_folders(&folders, &code);
        eprintln!(
            "• indexing {}",
            folders
                .iter()
                .chain(code.iter())
                .map(|p| omafind_core::paths::tilde(p))
                .collect::<Vec<_>>()
                .join(", ")
        );
        if !dry {
            config.save()?;
        }
    }

    // 3. The daemon.
    if confirm(
        "Start the indexing daemon now and at every login (systemd user service)?",
        options.yes,
    ) && !dry
    {
        match omafind_setup::start_daemon(&std::env::current_exe()?.with_file_name("omafindd")) {
            Ok(()) => eprintln!("• omafindd is running"),
            Err(error) => eprintln!(
                "• couldn't start omafindd with systemd ({error:#}); run `omafindd` by hand to see why"
            ),
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
    let mut wanted = hypr::Parts::default();
    for (part, slot, what) in [
        (
            have.binding,
            &mut wanted.binding,
            "the SUPER + CTRL + G keybinding",
        ),
        (
            have.rules,
            &mut wanted.rules,
            "the window rules (floating, centred, in the special:omafind workspace)",
        ),
        (
            have.autostart,
            &mut wanted.autostart,
            "starting omafind-app with the session",
        ),
    ] {
        if part {
            eprintln!("• {what} already in ~/.config/hypr/omafind.lua");
        } else if confirm(
            &format!("Add {what} to ~/.config/hypr/omafind.lua?"),
            options.yes,
        ) && !dry
        {
            *slot = true;
        }
    }
    if omafind_setup::install_hyprland(wanted.binding, wanted.rules, wanted.autostart)? {
        eprintln!("• Hyprland reloaded");
    }

    // 5. The window, resident from now on.
    if !dry && omafind_setup::start_app(&std::env::current_exe()?.with_file_name("omafind-app"))? {
        eprintln!("• omafind-app started; SUPER + CTRL + G opens it");
    }
    eprintln!("Done. `omafind status` shows the indexer, `omafind doctor` checks everything.");
    Ok(())
}

/// `omafind uninstall`: stops the daemon, removes the config lines, and (asked) the data.
pub fn uninstall(yes: bool, keep_data: bool) -> Result<()> {
    let _ = Command::new("systemctl")
        .args(["--user", "disable", "--now", "omafindd"])
        .status();
    let unit = omafind_core::paths::config_dir()
        .parent()
        .context("config dir")?
        .join("systemd/user/omafindd.service");
    if unit.exists() {
        std::fs::remove_file(&unit)?;
        let _ = Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .status();
    }
    let _ = Command::new("pkill").args(["-x", "omafind-app"]).status();
    for path in hypr::uninstall()? {
        eprintln!(
            "• removed Omafind's lines from {}",
            omafind_core::paths::tilde(&path)
        );
    }
    hypr::reload();
    if !keep_data
        && confirm(
            "Delete the index, thumbnails, config and model files (~1.3 GB)?",
            yes,
        )
    {
        for dir in [
            omafind_core::paths::data_dir(),
            omafind_core::paths::cache_dir(),
            omafind_core::paths::config_dir(),
        ] {
            if dir.exists() {
                std::fs::remove_dir_all(&dir)?;
                eprintln!("• removed {}", omafind_core::paths::tilde(&dir));
            }
        }
    }
    eprintln!("Omafind is gone. The binaries are left for your package manager.");
    Ok(())
}

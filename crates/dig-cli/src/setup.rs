//! `dig setup`: models, folders, the daemon, the window, the key. Each step asks (with `gum`) unless `--yes`.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use dig_core::FileKind;
use dig_core::config::Config;

use crate::{download, hypr};

pub struct Options {
    pub yes: bool,
    pub dry_run: bool,
    pub folders: Vec<PathBuf>,
    pub code: Vec<PathBuf>,
    pub text_only: bool,
}

fn have(cmd: &str) -> bool {
    Command::new("which").arg(cmd).output().map(|o| o.status.success()).unwrap_or(false)
}

/// Yes/no: `gum confirm`, else stdin.
fn confirm(question: &str, yes: bool) -> bool {
    if yes {
        eprintln!("{question} yes");
        return true;
    }
    if have("gum") {
        return Command::new("gum").args(["confirm", "--default=yes", question]).status().map(|s| s.success()).unwrap_or(false);
    }
    eprint!("{question} [Y/n] ");
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).ok();
    !line.trim().to_lowercase().starts_with('n')
}

/// A multi-choice: `gum choose --no-limit`, else all preselected.
fn choose(header: &str, items: &[(String, bool)], yes: bool) -> Vec<String> {
    if yes || !have("gum") {
        return items.iter().filter(|(_, on)| *on).map(|(label, _)| label.clone()).collect();
    }
    let selected: Vec<&str> = items.iter().filter(|(_, on)| *on).map(|(l, _)| l.as_str()).collect();
    let output = Command::new("gum")
        .args(["choose", "--no-limit", "--header", header])
        .args(selected.iter().flat_map(|s| ["--selected", s]))
        .args(items.iter().map(|(l, _)| l.as_str()))
        .output();
    match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).lines().map(str::to_owned).collect(),
        _ => Vec::new(),
    }
}

/// What a folder holds, for the picker: counts by kind and a rough first-pass time on this machine.
fn survey(path: &Path) -> (usize, String) {
    let (candidates, _) = dig_index::crawl(&dig_index::Root { path: path.to_path_buf(), code: false });
    let mut by_kind = std::collections::BTreeMap::new();
    let mut seconds = 0.0;
    for c in &candidates {
        *by_kind.entry(c.kind).or_insert(0usize) += 1;
        seconds += match c.kind {
            FileKind::Image => 0.8,
            FileKind::Screenshot => 1.8,
            FileKind::Doc => 0.4,
            FileKind::Pdf => 3.0,
            FileKind::Audio => 0.4 * (c.size as f64 / 1_000_000.0).max(0.5),
            FileKind::Video => 2.0 * (c.size as f64 / 20_000_000.0).max(0.5),
            FileKind::Code => 1.0,
        };
    }
    let parts: Vec<String> = by_kind.iter().map(|(k, n)| format!("{n} {}", plural(*k, *n))).collect();
    let time = if seconds < 60.0 { format!("{seconds:.0} s") } else if seconds < 3600.0 { format!("{:.0} min", seconds / 60.0) } else { format!("{:.1} h", seconds / 3600.0) };
    (candidates.len(), format!("{} (about {time} on the GPU)", if parts.is_empty() { "nothing to index".into() } else { parts.join(", ") }))
}

fn plural(kind: FileKind, n: usize) -> &'static str {
    match (kind, n) {
        (FileKind::Image, 1) => "photo",
        (FileKind::Image, _) => "photos",
        (FileKind::Screenshot, 1) => "screenshot",
        (FileKind::Screenshot, _) => "screenshots",
        (FileKind::Pdf, 1) => "PDF",
        (FileKind::Pdf, _) => "PDFs",
        (FileKind::Doc, 1) => "document",
        (FileKind::Doc, _) => "documents",
        (FileKind::Audio, 1) => "recording",
        (FileKind::Audio, _) => "recordings",
        (FileKind::Video, 1) => "video",
        (FileKind::Video, _) => "videos",
        (FileKind::Code, _) => "code files",
    }
}

pub fn run(options: Options) -> Result<()> {
    let home = PathBuf::from(std::env::var_os("HOME").context("no HOME")?);
    let dry = options.dry_run;
    eprintln!("Dig setup");

    // 1. Models.
    let models = dig_core::model::dir();
    let wanted: Vec<&download::ModelFile> = if options.text_only { vec![&download::TEXT] } else { vec![&download::TEXT, &download::MEDIA] };
    let missing: Vec<&&download::ModelFile> = wanted.iter().filter(|f| !download::present(&models, f)).collect();
    if missing.is_empty() {
        eprintln!("• model files present in {}", dig_core::paths::tilde(&models));
    } else {
        let mb: u64 = missing.iter().map(|f| f.bytes).sum::<u64>() / 1_000_000;
        if confirm(&format!("Download the EmbeddingGemma 2 model files ({mb} MB) into {}?", dig_core::paths::tilde(&models)), options.yes) && !dry {
            for file in &missing {
                download::fetch(&models, file)?;
            }
        }
    }

    // 2. Folders.
    let mut config = Config::load().unwrap_or_default();
    let folders: Vec<PathBuf> = if !options.folders.is_empty() {
        options.folders.iter().map(|p| dig_index::indexer::resolve_root(p)).collect::<Result<_>>()?
    } else {
        let candidates = ["Pictures", "Documents", "Downloads", "Videos", "Music", "Desktop", "Notes", "Books"];
        let existing: Vec<PathBuf> = candidates.iter().map(|c| home.join(c)).filter(|p| p.is_dir()).collect();
        let already = config.folders();
        eprintln!("• looking at your folders…");
        let labelled: Vec<(String, bool)> = existing
            .iter()
            .map(|p| {
                let (_, summary) = survey(p);
                let on = already.is_empty() && matches!(p.file_name().and_then(|n| n.to_str()), Some("Pictures" | "Documents")) || already.contains(p);
                (format!("{:<14} {summary}", dig_core::paths::tilde(p)), on)
            })
            .collect();
        let chosen = choose("Which folders should Dig index? (space to toggle, enter to confirm)", &labelled, options.yes);
        existing.into_iter().filter(|p| chosen.iter().any(|c| c.starts_with(&dig_core::paths::tilde(p)))).collect()
    };
    let code: Vec<PathBuf> = if !options.code.is_empty() {
        options.code.iter().map(|p| dig_index::indexer::resolve_root(p)).collect::<Result<_>>()?
    } else if !options.yes && have("gum") {
        let candidates: Vec<PathBuf> = ["dev", "src", "code", "projects", "Projects", "repos", "work"].iter().map(|c| home.join(c)).filter(|p| p.is_dir()).collect();
        if candidates.is_empty() {
            Vec::new()
        } else {
            let labelled: Vec<(String, bool)> = candidates.iter().map(|p| (dig_core::paths::tilde(p), config.code_roots().contains(p))).collect();
            let chosen = choose("Index code too? Searched separately with a `code:` prefix (enter to skip)", &labelled, false);
            candidates.into_iter().filter(|p| chosen.contains(&dig_core::paths::tilde(p))).collect()
        }
    } else {
        config.code_roots()
    };
    if folders.is_empty() && code.is_empty() {
        eprintln!("• no folders chosen; `dig index ~/Pictures ~/Documents` picks them later");
    } else {
        config.set_folders(&folders, &code);
        eprintln!("• indexing {}", folders.iter().chain(code.iter()).map(|p| dig_core::paths::tilde(p)).collect::<Vec<_>>().join(", "));
        if !dry {
            config.save()?;
        }
    }

    // 3. The daemon.
    if confirm("Start the indexing daemon now and at every login (systemd user service)?", options.yes) && !dry {
        install_unit()?;
        let ok = Command::new("systemctl").args(["--user", "daemon-reload"]).status().map(|s| s.success()).unwrap_or(false)
            && Command::new("systemctl").args(["--user", "enable", "--now", "digd"]).status().map(|s| s.success()).unwrap_or(false);
        if ok {
            eprintln!("• digd is running");
        } else {
            eprintln!("• couldn't start digd with systemd; run `digd` by hand to see why");
        }
        if dig_client::Client::is_running() {
            if let Ok(mut client) = dig_client::Client::connect() {
                let _ = client.simple(&dig_client::Request::SetRoots { folders: folders.clone(), code: code.clone() });
            }
        }
    }

    // 4. Hyprland: autostart, keybinding, window rules.
    let mut reload = false;
    for snippet in &hypr::SNIPPETS {
        if hypr::is_installed(snippet) {
            eprintln!("• {} already in ~/.config/hypr/{}", snippet.what, snippet.file);
            continue;
        }
        if confirm(&format!("Add {} to ~/.config/hypr/{}?", snippet.what, snippet.file), options.yes) && !dry {
            hypr::install(snippet)?;
            reload = true;
        }
    }
    if reload {
        hypr::reload();
        eprintln!("• Hyprland reloaded");
    }

    // 5. The window, resident from now on.
    if !dry && Command::new("hyprctl").args(["-j", "clients"]).output().map(|o| !String::from_utf8_lossy(&o.stdout).contains("\"class\": \"dig\"")).unwrap_or(false) {
        let exe = std::env::current_exe()?.with_file_name("dig-app");
        let program = if exe.exists() { exe } else { PathBuf::from("dig-app") };
        let _ = Command::new("setsid").arg("-f").arg(program).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
        eprintln!("• dig-app started; SUPER + CTRL + G opens it");
    }
    eprintln!("Done. `dig status` shows the indexer, `dig doctor` checks everything.");
    Ok(())
}

/// The user unit, pointing at this build's `digd`, unless a packaged unit exists in /usr/lib/systemd/user.
fn install_unit() -> Result<()> {
    if Path::new("/usr/lib/systemd/user/digd.service").exists() {
        return Ok(());
    }
    let digd = std::env::current_exe()?.with_file_name("digd");
    let unit = include_str!("../../../systemd/digd.service").replace("/usr/bin/digd", &digd.to_string_lossy());
    let dir = dig_core::paths::config_dir().parent().context("config dir")?.join("systemd/user");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("digd.service"), unit)?;
    Ok(())
}

/// `dig uninstall`: stops the daemon, removes the config lines, and (asked) the data.
pub fn uninstall(yes: bool, keep_data: bool) -> Result<()> {
    let _ = Command::new("systemctl").args(["--user", "disable", "--now", "digd"]).status();
    let unit = dig_core::paths::config_dir().parent().context("config dir")?.join("systemd/user/digd.service");
    if unit.exists() {
        std::fs::remove_file(&unit)?;
        let _ = Command::new("systemctl").args(["--user", "daemon-reload"]).status();
    }
    let _ = Command::new("pkill").args(["-x", "dig-app"]).status();
    for path in hypr::uninstall()? {
        eprintln!("• removed Dig's lines from {}", dig_core::paths::tilde(&path));
    }
    hypr::reload();
    if !keep_data && confirm("Delete the index, thumbnails, config and model files (~1.3 GB)?", yes) {
        for dir in [dig_core::paths::data_dir(), dig_core::paths::cache_dir(), dig_core::paths::config_dir()] {
            if dir.exists() {
                std::fs::remove_dir_all(&dir)?;
                eprintln!("• removed {}", dig_core::paths::tilde(&dir));
            }
        }
    }
    eprintln!("Dig is gone. The binaries are left for your package manager.");
    Ok(())
}

//! The three lines Dig adds to the user's Hyprland config (Omarchy's Lua config, one file per concern), added once
//! and removed cleanly: each carries a `-- dig` marker.

use std::path::PathBuf;

use anyhow::{Context, Result};

pub const MARKER: &str = "-- dig";

pub struct Snippet {
    pub file: &'static str,
    pub lines: &'static str,
    pub what: &'static str,
}

pub const SNIPPETS: [Snippet; 3] = [
    Snippet {
        file: "bindings.lua",
        what: "the SUPER + CTRL + G keybinding",
        lines: "o.bind(\"SUPER + CTRL + G\", \"Dig\", \"dig toggle\") -- dig",
    },
    Snippet {
        file: "windows.lua",
        what: "the window rules (floating, centred, in the special:dig workspace)",
        lines: "o.window({ class = \"^dig$\" }, { workspace = \"special:dig silent\" }) -- dig\n\
                o.window({ class = \"^dig$\" }, { float = true }) -- dig\n\
                o.window({ class = \"^dig$\" }, { center = true }) -- dig\n\
                o.window({ class = \"^dig$\" }, { size = { 880, 600 } }) -- dig",
    },
    Snippet {
        file: "autostart.lua",
        what: "starting dig-app with the session",
        lines: "o.launch_on_start(\"dig-app\") -- dig",
    },
];

pub fn config_dir() -> PathBuf {
    dig_core::paths::config_dir().parent().map(|p| p.join("hypr")).unwrap_or_else(|| PathBuf::from("~/.config/hypr"))
}

pub fn is_installed(snippet: &Snippet) -> bool {
    std::fs::read_to_string(config_dir().join(snippet.file)).map(|text| text.lines().any(|l| l.contains(MARKER) && l.contains("dig"))).unwrap_or(false)
        && std::fs::read_to_string(config_dir().join(snippet.file)).map(|text| snippet.lines.lines().all(|needed| text.contains(needed.trim()))).unwrap_or(false)
}

/// Appends the snippet (each line normalised) unless its lines are there already.
pub fn install(snippet: &Snippet) -> Result<()> {
    let path = config_dir().join(snippet.file);
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut added = false;
    for line in snippet.lines.lines().map(str::trim) {
        if !text.contains(line) {
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(line);
            text.push('\n');
            added = true;
        }
    }
    if added {
        std::fs::create_dir_all(config_dir())?;
        std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(())
}

/// Removes every marked line from the three files.
pub fn uninstall() -> Result<Vec<PathBuf>> {
    let mut touched = Vec::new();
    for snippet in &SNIPPETS {
        let path = config_dir().join(snippet.file);
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let kept: Vec<&str> = text.lines().filter(|l| !(l.contains(MARKER) && l.contains("dig"))).collect();
        if kept.len() != text.lines().count() {
            std::fs::write(&path, format!("{}\n", kept.join("\n")))?;
            touched.push(path);
        }
    }
    Ok(touched)
}

/// Asks Hyprland to re-read its config.
pub fn reload() {
    let _ = std::process::Command::new("hyprctl").arg("reload").output();
}

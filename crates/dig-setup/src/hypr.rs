//! Dig's Hyprland config: one file Dig owns, `~/.config/hypr/dig.lua`, with the keybinding, the window rules and
//! the autostart line, loaded by one line in `~/.config/hypr/hyprland.lua`:
//!
//! ```text
//! require("hypr.dig") -- dig
//! ```
//!
//! Omarchy's `hyprland.lua` requires `hypr.bindings`, `hypr.autostart` and friends; a `windows.lua` is not loaded
//! by every version of it, so a file of our own with a require is the only arrangement that works everywhere.
//! Earlier Dig builds appended marked lines to the three files; those are removed on install and uninstall.

use std::path::PathBuf;

use anyhow::{Context, Result};

pub const MARKER: &str = "-- dig";
pub const FILE: &str = "dig.lua";
pub const REQUIRE_LINE: &str = "require(\"hypr.dig\") -- dig";

pub const BINDING: &str = "o.bind(\"SUPER + CTRL + G\", \"Dig\", \"dig toggle\")";
pub const RULES: &str = "o.window({ class = \"^dig$\" }, { workspace = \"special:dig silent\" })\n\
o.window({ class = \"^dig$\" }, { float = true })\n\
o.window({ class = \"^dig$\" }, { center = true })\n\
o.window({ class = \"^dig$\" }, { size = { 880, 600 } })";
pub const AUTOSTART: &str = "o.launch_on_start(\"dig-app\")";

/// What `dig.lua` holds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Parts {
    pub binding: bool,
    pub rules: bool,
    pub autostart: bool,
}

pub fn config_dir() -> PathBuf {
    dig_core::paths::config_dir().parent().map(|p| p.join("hypr")).unwrap_or_else(|| PathBuf::from("~/.config/hypr"))
}

fn dig_lua() -> PathBuf {
    config_dir().join(FILE)
}

fn hyprland_lua() -> PathBuf {
    config_dir().join("hyprland.lua")
}

/// Which parts are installed right now (the file's contents, and only if `hyprland.lua` loads it).
pub fn installed() -> Parts {
    let required = std::fs::read_to_string(hyprland_lua()).map(|t| t.lines().any(|l| l.trim() == REQUIRE_LINE || l.trim() == "require(\"hypr.dig\")")).unwrap_or(false);
    if !required {
        return Parts::default();
    }
    let text = std::fs::read_to_string(dig_lua()).unwrap_or_default();
    Parts {
        binding: text.contains(BINDING),
        rules: RULES.lines().all(|l| text.contains(l)),
        autostart: text.contains(AUTOSTART),
    }
}

/// Writes `dig.lua` with the parts wanted (keeping parts already there) and makes sure it is required.
pub fn install(wanted: Parts) -> Result<bool> {
    let before = installed();
    let parts = Parts { binding: wanted.binding || before.binding, rules: wanted.rules || before.rules, autostart: wanted.autostart || before.autostart };
    remove_legacy_lines()?;
    let mut text = String::from("-- Dig: written by `dig setup`; `dig uninstall` removes it. Edit freely, Dig only adds what is missing.\n");
    if parts.binding {
        text.push_str("\n-- Show or hide the search window.\n");
        text.push_str(BINDING);
        text.push('\n');
    }
    if parts.rules {
        text.push_str("\n-- The window lives in its own special workspace, like the scratchpad console: showing it is a workspace\n-- toggle, and Hyprland dims what is behind it.\n");
        text.push_str(RULES);
        text.push('\n');
    }
    if parts.autostart {
        text.push_str("\n-- Keep the window ready so it opens instantly.\n");
        text.push_str(AUTOSTART);
        text.push('\n');
    }
    std::fs::create_dir_all(config_dir())?;
    let existing = std::fs::read_to_string(dig_lua()).unwrap_or_default();
    let mut changed = false;
    if existing != text {
        std::fs::write(dig_lua(), &text).with_context(|| format!("writing {}", dig_lua().display()))?;
        changed = true;
    }
    let hypr = hyprland_lua();
    let mut main = std::fs::read_to_string(&hypr).unwrap_or_default();
    if !main.lines().any(|l| l.trim() == REQUIRE_LINE || l.trim() == "require(\"hypr.dig\")") {
        if !main.is_empty() && !main.ends_with('\n') {
            main.push('\n');
        }
        main.push_str("\n-- Dig: search your files (dig.lua is written by `dig setup`).\n");
        main.push_str(REQUIRE_LINE);
        main.push('\n');
        std::fs::write(&hypr, main).with_context(|| format!("writing {}", hypr.display()))?;
        changed = true;
    }
    Ok(changed)
}

/// Earlier builds appended marked lines to bindings.lua, windows.lua and autostart.lua.
fn remove_legacy_lines() -> Result<Vec<PathBuf>> {
    let mut touched = Vec::new();
    for file in ["bindings.lua", "windows.lua", "autostart.lua"] {
        let path = config_dir().join(file);
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let kept: Vec<&str> = text.lines().filter(|l| !(l.contains(MARKER) && l.contains("dig"))).collect();
        if kept.len() != text.lines().count() {
            std::fs::write(&path, format!("{}\n", kept.join("\n")))?;
            touched.push(path);
        }
    }
    Ok(touched)
}

/// Removes `dig.lua`, its require line, and any legacy lines. Returns what was touched.
pub fn uninstall() -> Result<Vec<PathBuf>> {
    let mut touched = remove_legacy_lines()?;
    if dig_lua().exists() {
        std::fs::remove_file(dig_lua())?;
        touched.push(dig_lua());
    }
    let hypr = hyprland_lua();
    if let Ok(text) = std::fs::read_to_string(&hypr) {
        let kept: Vec<&str> = text.lines().filter(|l| !(l.contains("hypr.dig") || l.contains("-- Dig: search your files"))).collect();
        if kept.len() != text.lines().count() {
            std::fs::write(&hypr, format!("{}\n", kept.join("\n")))?;
            touched.push(hypr);
        }
    }
    Ok(touched)
}

/// Asks Hyprland to re-read its config.
pub fn reload() {
    let _ = std::process::Command::new("hyprctl").arg("reload").output();
}

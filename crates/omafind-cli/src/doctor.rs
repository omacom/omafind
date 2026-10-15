//! `omafind doctor`: everything Omafind needs, checked, with what to do about what's missing. On a terminal the
//! checks run group by group under a spinner and each group's results appear as it finishes.

use std::process::Command;

use anyhow::Result;
use omafind_core::paths::tilde;

use omafind_setup::{download, hypr};

use crate::ui;

struct Check {
    ok: bool,
    /// Missing but not needed: shown apart from a failure, and not counted as one.
    optional_missing: bool,
    what: String,
    detail: String,
}

impl Check {
    fn new(ok: bool, what: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            ok,
            optional_missing: false,
            what: what.into(),
            detail: detail.into(),
        }
    }
}

fn have(cmd: &str) -> bool {
    Command::new("which")
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn models() -> Vec<Check> {
    let dir = omafind_core::model::dir();
    [&download::TEXT, &download::MEDIA]
        .into_iter()
        .map(|file| {
            let size = std::fs::metadata(dir.join(file.name)).map(|m| m.len()).ok();
            let detail = match size {
                Some(s) if s == file.bytes => format!("{} MB in {}", s / 1_000_000, tilde(&dir)),
                Some(s) => format!("wrong size {s}: run `omafind setup`"),
                None => "missing: run `omafind setup`".into(),
            };
            Check::new(size == Some(file.bytes), file.name, detail)
        })
        .collect()
}

fn engine() -> Vec<Check> {
    let mut checks = Vec::new();
    let (backends, gpu) = omafind_embed::probe();
    checks.push(Check::new(
        !backends.is_empty(),
        format!("llama.cpp {}", omafind_embed::LLAMA_VERSION),
        if backends.is_empty() {
            "no ggml backends loaded".into()
        } else {
            format!(
                "{}{}",
                backends.join(", "),
                if gpu {
                    " · GPU found"
                } else {
                    " · no GPU, CPU only"
                }
            )
        },
    ));
    if have("vulkaninfo") {
        let device = Command::new("vulkaninfo")
            .arg("--summary")
            .output()
            .ok()
            .and_then(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .find(|l| l.contains("deviceName"))
                    .map(|l| l.split('=').nth(1).unwrap_or("").trim().to_owned())
            });
        checks.push(Check::new(
            device.is_some(),
            "Vulkan device",
            device
                .unwrap_or_else(|| "none (vulkan-icd-loader + a driver like vulkan-radeon)".into()),
        ));
    }
    checks
}

fn tools() -> Vec<Check> {
    let mut checks: Vec<Check> = [
        ("ffmpeg", "ffmpeg", "audio and video"),
        ("ffmpegthumbnailer", "ffmpegthumbnailer", "thumbnails"),
        ("pdftotext", "poppler", "PDF text"),
        ("pdftoppm", "poppler", "scanned pages and PDF thumbnails"),
        (
            "tesseract",
            "tesseract + tesseract-data-eng",
            "text in screenshots",
        ),
        ("mpv", "mpv", "opening recordings at a moment"),
        ("evince", "evince", "opening PDFs at a page"),
        ("imv", "imv", "opening photos"),
        ("wl-copy", "wl-clipboard", "copying files"),
    ]
    .into_iter()
    .map(|(cmd, package, why)| {
        let ok = have(cmd);
        Check::new(
            ok,
            cmd,
            if ok {
                why.to_owned()
            } else {
                format!("missing: sudo pacman -S {package} ({why})")
            },
        )
    })
    .collect();
    // Optional: without it only .doc and .rtf are skipped.
    let pandoc = have("pandoc");
    checks.push(Check {
        optional_missing: !pandoc,
        ..Check::new(
            true,
            "pandoc",
            if pandoc {
                ".doc and .rtf"
            } else {
                "optional, not installed: .doc and .rtf are skipped (pacman -S pandoc-cli)"
            },
        )
    });
    checks
}

fn index() -> Vec<Check> {
    let mut checks = Vec::new();
    match omafind_core::config::Config::load() {
        Ok(c) => {
            let all: Vec<String> = c
                .folders()
                .iter()
                .chain(c.code_roots().iter())
                .map(|p| tilde(p))
                .collect();
            checks.push(Check::new(
                !all.is_empty(),
                "folders",
                if all.is_empty() {
                    "none: run `omafind setup` or `omafind index <folders>`".to_owned()
                } else {
                    all.join(", ")
                },
            ));
        }
        Err(e) => checks.push(Check::new(false, "config", format!("{e:#}"))),
    }
    let path = omafind_core::paths::index_path();
    checks.push(Check::new(
        path.exists(),
        "index",
        if path.exists() {
            tilde(&path)
        } else {
            "none yet".into()
        },
    ));
    let running = omafind_client::Client::is_running();
    let status = running
        .then(|| {
            omafind_client::Client::connect()
                .and_then(|mut c| c.status())
                .ok()
        })
        .flatten();
    checks.push(Check::new(
        running,
        "omafindd",
        match &status {
            Some(s) => format!(
                "{} · {} indexed, {} pending, {} failed",
                s.state, s.indexed, s.pending, s.failed
            ),
            None if running => "answers, but not with a status".into(),
            None => "not running: systemctl --user enable --now omafindd".into(),
        },
    ));
    let enabled = omafind_setup::daemon_enabled();
    checks.push(Check::new(
        enabled,
        "omafindd at login",
        if enabled {
            "enabled"
        } else {
            "not enabled: run `omafind setup`"
        },
    ));
    checks
}

fn window() -> Vec<Check> {
    let mut checks = Vec::new();
    let resident = Command::new("hyprctl")
        .args(["-j", "clients"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("\"class\": \"omafind\""))
        .unwrap_or(false);
    checks.push(Check::new(
        resident,
        "omafind-app",
        if resident {
            "resident"
        } else {
            "not running: `omafind toggle` starts it"
        },
    ));
    let parts = hypr::installed();
    for (ok, what) in [
        (parts.binding, "SUPER + CTRL + G keybinding"),
        (parts.rules, "window rules"),
        (parts.autostart, "omafind-app at login"),
    ] {
        checks.push(Check::new(
            ok,
            what,
            if ok {
                "in ~/.config/hypr/omafind.lua"
            } else {
                "missing: run `omafind setup`"
            },
        ));
    }
    checks
}

type Group = (&'static str, &'static str, fn() -> Vec<Check>);

const GROUPS: [Group; 5] = [
    ("Model", "Looking for the model files", models),
    ("Engine", "Loading the engine and probing for a GPU", engine),
    (
        "Tools",
        "Looking for the programs Omafind reads and opens files with",
        tools,
    ),
    ("Indexing", "Asking the daemon how indexing goes", index),
    (
        "Window",
        "Checking the window and the Hyprland config",
        window,
    ),
];

pub fn run(json: bool) -> Result<bool> {
    if json {
        let checks: Vec<Check> = GROUPS.iter().flat_map(|(_, _, run)| run()).collect();
        let all_ok = checks.iter().all(|c| c.ok);
        let rows: Vec<_> = checks
            .iter()
            .map(|c| serde_json::json!({"ok": c.ok, "what": c.what, "detail": c.detail}))
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(all_ok);
    }
    ui::intro(
        "doctor",
        "checks the model, engine, tools, daemon and window",
    )?;
    let mut failed = 0;
    for (title, doing, run) in GROUPS {
        let checks = ui::step(doing, 350, run, |checks| {
            let ok = checks.iter().filter(|c| c.ok).count();
            let tally = format!("{ok}/{}", checks.len());
            let tally = if ok == checks.len() {
                console::style(tally).green().to_string()
            } else {
                console::style(tally).yellow().to_string()
            };
            format!("{}  {tally}", ui::bold(title))
        });
        let width = checks
            .iter()
            .map(|c| c.what.chars().count())
            .max()
            .unwrap_or(0);
        for c in &checks {
            let mark = if c.optional_missing {
                console::style("○").yellow().to_string()
            } else {
                ui::mark(c.ok)
            };
            ui::line(format!(
                "{} {:<width$}  {}",
                mark,
                c.what,
                ui::dim(&c.detail)
            ));
            ui::beat(30);
        }
        ui::line("");
        failed += checks.iter().filter(|c| !c.ok).count();
    }
    if failed == 0 {
        cliclack::outro("Everything looks fine")?;
    } else {
        cliclack::outro_cancel(format!(
            "{failed} {} attention",
            if failed == 1 {
                "check needs"
            } else {
                "checks need"
            }
        ))?;
    }
    Ok(failed == 0)
}

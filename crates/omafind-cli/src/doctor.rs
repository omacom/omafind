//! `omafind doctor`: everything Omafind needs, checked, with what to do about what's missing.

use std::process::Command;

use anyhow::Result;

use omafind_setup::{download, hypr};

struct Check {
    ok: bool,
    what: String,
    detail: String,
}

fn have(cmd: &str) -> bool {
    Command::new("which")
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn run(json: bool) -> Result<bool> {
    let mut checks = Vec::new();
    let models = omafind_core::model::dir();
    for file in [&download::TEXT, &download::MEDIA] {
        let path = models.join(file.name);
        let size = std::fs::metadata(&path).map(|m| m.len()).ok();
        checks.push(Check {
            ok: size == Some(file.bytes),
            what: format!("model {}", file.name),
            detail: match size {
                Some(s) if s == file.bytes => format!(
                    "{} MB in {}",
                    s / 1_000_000,
                    omafind_core::paths::tilde(&models)
                ),
                Some(s) => format!("wrong size {s}: run `omafind setup`"),
                None => "missing: run `omafind setup`".into(),
            },
        });
    }
    let (backends, gpu) = omafind_embed::probe();
    checks.push(Check {
        ok: !backends.is_empty(),
        what: format!("engine llama.cpp {}", omafind_embed::LLAMA_VERSION),
        detail: if backends.is_empty() {
            "no ggml backends loaded".into()
        } else {
            format!(
                "backends {}{}",
                backends.join(", "),
                if gpu {
                    " (GPU found)"
                } else {
                    " (no GPU: CPU only)"
                }
            )
        },
    });
    if have("vulkaninfo") {
        let out = Command::new("vulkaninfo").arg("--summary").output().ok();
        let device = out.and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .find(|l| l.contains("deviceName"))
                .map(|l| l.split('=').nth(1).unwrap_or("").trim().to_owned())
        });
        checks.push(Check {
            ok: device.is_some(),
            what: "Vulkan device".into(),
            detail: device
                .unwrap_or_else(|| "none (vulkan-icd-loader + a driver like vulkan-radeon)".into()),
        });
    }
    for (cmd, package, why) in [
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
    ] {
        checks.push(Check {
            ok: have(cmd),
            what: format!("{cmd} ({why})"),
            detail: if have(cmd) {
                "present".into()
            } else {
                format!("missing: sudo pacman -S {package}")
            },
        });
    }
    let pandoc = have("pandoc");
    checks.push(Check {
        ok: true,
        what: "pandoc (doc, rtf, optional)".into(),
        detail: if pandoc {
            "present".into()
        } else {
            "not installed: .doc/.rtf files are skipped (pacman -S pandoc-cli)".into()
        },
    });

    let config = omafind_core::config::Config::load();
    match &config {
        Ok(c) => {
            let folders = c.folders();
            checks.push(Check {
                ok: !folders.is_empty() || !c.code_roots().is_empty(),
                what: "folders to index".into(),
                detail: if folders.is_empty() && c.code_roots().is_empty() {
                    "none: run `omafind setup` or `omafind index <folders>`".into()
                } else {
                    folders
                        .iter()
                        .chain(c.code_roots().iter())
                        .map(|p| omafind_core::paths::tilde(p))
                        .collect::<Vec<_>>()
                        .join(", ")
                },
            });
        }
        Err(e) => checks.push(Check {
            ok: false,
            what: "config".into(),
            detail: format!("{e:#}"),
        }),
    }
    let index = omafind_core::paths::index_path();
    checks.push(Check {
        ok: index.exists(),
        what: "index".into(),
        detail: if index.exists() {
            omafind_core::paths::tilde(&index)
        } else {
            "none yet".into()
        },
    });

    let running = omafind_client::Client::is_running();
    let status = if running {
        omafind_client::Client::connect()
            .and_then(|mut c| c.status())
            .ok()
    } else {
        None
    };
    checks.push(Check {
        ok: running,
        what: "omafindd".into(),
        detail: match &status {
            Some(s) => format!(
                "{} · {} indexed, {} pending, {} failed",
                s.state, s.indexed, s.pending, s.failed
            ),
            None if running => "answers, but not with a status".into(),
            None => "not running: systemctl --user enable --now omafindd".into(),
        },
    });
    let unit_enabled = omafind_setup::daemon_enabled();
    checks.push(Check {
        ok: unit_enabled,
        what: "omafindd at login".into(),
        detail: if unit_enabled {
            "enabled".into()
        } else {
            "not enabled: run `omafind setup`".into()
        },
    });

    let window = Command::new("hyprctl")
        .args(["-j", "clients"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("\"class\": \"omafind\""))
        .unwrap_or(false);
    checks.push(Check {
        ok: window,
        what: "omafind-app window".into(),
        detail: if window {
            "resident".into()
        } else {
            "not running: `omafind toggle` starts it".into()
        },
    });
    let parts = hypr::installed();
    for (ok, what) in [
        (parts.binding, "SUPER + CTRL + G keybinding"),
        (
            parts.rules,
            "window rules (special:omafind, floating, centred)",
        ),
        (parts.autostart, "omafind-app at login"),
    ] {
        checks.push(Check {
            ok,
            what: format!("hypr/omafind.lua: {what}"),
            detail: if ok {
                "present".into()
            } else {
                "missing: run `omafind setup`".into()
            },
        });
    }

    let all_ok = checks.iter().all(|c| c.ok);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &checks
                    .iter()
                    .map(|c| serde_json::json!({"ok": c.ok, "what": c.what, "detail": c.detail}))
                    .collect::<Vec<_>>()
            )?
        );
    } else {
        cliclack::intro("Omafind doctor")?;
        for c in &checks {
            let line = format!("{}\n{}", c.what, c.detail);
            if c.ok {
                cliclack::log::success(line)?;
            } else {
                cliclack::log::error(line)?;
            }
        }
        if all_ok {
            cliclack::outro("Everything looks fine")?;
        } else {
            let failed = checks.iter().filter(|c| !c.ok).count();
            cliclack::outro_cancel(format!(
                "{failed} {} need attention",
                if failed == 1 { "check" } else { "checks" }
            ))?;
        }
    }
    Ok(all_ok)
}

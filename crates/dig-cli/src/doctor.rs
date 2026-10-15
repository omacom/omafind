//! `dig doctor`: everything Dig needs, checked, with what to do about what's missing.

use std::process::Command;

use anyhow::Result;

use dig_setup::{download, hypr};

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
    let models = dig_core::model::dir();
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
                    dig_core::paths::tilde(&models)
                ),
                Some(s) => format!("wrong size {s}: run `dig setup`"),
                None => "missing: run `dig setup`".into(),
            },
        });
    }
    let (backends, gpu) = dig_embed::probe();
    checks.push(Check {
        ok: !backends.is_empty(),
        what: format!("engine llama.cpp {}", dig_embed::LLAMA_VERSION),
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
        ("gum", "gum", "dig setup prompts"),
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

    let config = dig_core::config::Config::load();
    match &config {
        Ok(c) => {
            let folders = c.folders();
            checks.push(Check {
                ok: !folders.is_empty() || !c.code_roots().is_empty(),
                what: "folders to index".into(),
                detail: if folders.is_empty() && c.code_roots().is_empty() {
                    "none: run `dig setup` or `dig index <folders>`".into()
                } else {
                    folders
                        .iter()
                        .chain(c.code_roots().iter())
                        .map(|p| dig_core::paths::tilde(p))
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
    let index = dig_core::paths::index_path();
    checks.push(Check {
        ok: index.exists(),
        what: "index".into(),
        detail: if index.exists() {
            dig_core::paths::tilde(&index)
        } else {
            "none yet".into()
        },
    });

    let running = dig_client::Client::is_running();
    let status = if running {
        dig_client::Client::connect()
            .and_then(|mut c| c.status())
            .ok()
    } else {
        None
    };
    checks.push(Check {
        ok: running,
        what: "digd".into(),
        detail: match &status {
            Some(s) => format!(
                "{} · {} indexed, {} pending, {} failed",
                s.state, s.indexed, s.pending, s.failed
            ),
            None if running => "answers, but not with a status".into(),
            None => "not running: systemctl --user enable --now digd".into(),
        },
    });
    let unit_enabled = dig_setup::daemon_enabled();
    checks.push(Check {
        ok: unit_enabled,
        what: "digd at login".into(),
        detail: if unit_enabled {
            "enabled".into()
        } else {
            "not enabled: run `dig setup`".into()
        },
    });

    let window = Command::new("hyprctl")
        .args(["-j", "clients"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("\"class\": \"dig\""))
        .unwrap_or(false);
    checks.push(Check {
        ok: window,
        what: "dig-app window".into(),
        detail: if window {
            "resident".into()
        } else {
            "not running: `dig toggle` starts it".into()
        },
    });
    let parts = hypr::installed();
    for (ok, what) in [
        (parts.binding, "SUPER + CTRL + G keybinding"),
        (parts.rules, "window rules (special:dig, floating, centred)"),
        (parts.autostart, "dig-app at login"),
    ] {
        checks.push(Check {
            ok,
            what: format!("hypr/dig.lua: {what}"),
            detail: if ok {
                "present".into()
            } else {
                "missing: run `dig setup`".into()
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
        for c in &checks {
            println!(
                "{} {:<44} {}",
                if c.ok { "✓" } else { "✗" },
                c.what,
                c.detail
            );
        }
        println!(
            "{}",
            if all_ok {
                "everything looks fine"
            } else {
                "some checks failed"
            }
        );
    }
    Ok(all_ok)
}

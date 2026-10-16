//! Hyprland's IPC: dispatch commands over `.socket.sock`, and follow events over `.socket2.sock`, both under
//! `$XDG_RUNTIME_DIR/hypr/<instance>/`. No polling: the event socket pushes.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};

/// The special workspace the window lives in; `omafind toggle` shows and hides it.
pub const WORKSPACE: &str = "omafind";
/// The window's app id, matched by the window rules.
pub const APP_ID: &str = "omafind";

fn instance_dir() -> Result<PathBuf> {
    let runtime = omafind_core::paths::runtime_dir();
    let signature = match std::env::var("HYPRLAND_INSTANCE_SIGNATURE") {
        Ok(s) if !s.is_empty() => s,
        _ => {
            // Not inherited (started from a service or an agent shell): the one running instance.
            let mut entries: Vec<_> = std::fs::read_dir(runtime.join("hypr"))
                .context("no Hyprland instance")?
                .flatten()
                .collect();
            entries.sort_by_key(|e| e.metadata().and_then(|m| m.modified()).ok());
            entries
                .last()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .context("no Hyprland instance")?
        }
    };
    Ok(runtime.join("hypr").join(signature))
}

/// Runs one `hyprctl`-style command and returns its answer.
pub fn command(line: &str) -> Result<String> {
    let mut stream = UnixStream::connect(instance_dir()?.join(".socket.sock"))
        .context("connecting to Hyprland")?;
    stream.write_all(line.as_bytes())?;
    let mut answer = String::new();
    stream.read_to_string(&mut answer)?;
    Ok(answer)
}

pub fn dispatch(dispatcher: &str) -> Result<()> {
    let answer = command(&format!("dispatch {dispatcher}"))?;
    if answer.trim() != "ok" {
        bail!("hyprland: {}", answer.trim());
    }
    Ok(())
}

/// Shows the window if hidden, hides it if shown. Hyprland's dispatchers are Lua expressions now.
pub fn toggle() -> Result<()> {
    dispatch(&format!("hl.dsp.workspace.toggle_special(\"{WORKSPACE}\")"))
}

/// Gives the window keyboard focus. A toggle from the keybinding focuses it anyway; a window shown while another
/// had the keyboard (the first show after a launch) does not always.
pub fn focus_window() -> Result<()> {
    dispatch(&format!("hl.dsp.focus({{ window = \"class:{APP_ID}\" }})"))
}

/// This process's window: its address and the workspace it is on.
fn own_window() -> Result<(String, String)> {
    let clients: serde_json::Value = serde_json::from_str(&command("j/clients")?)?;
    let me = std::process::id() as i64;
    let window = clients
        .as_array()
        .and_then(|c| {
            c.iter()
                .find(|c| c["pid"].as_i64() == Some(me) && c["class"] == APP_ID)
        })
        .context("our window isn't mapped yet")?;
    Ok((
        window["address"].as_str().unwrap_or_default().to_owned(),
        window["workspace"]["name"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
    ))
}

/// Whether the window lives in its special workspace, as the window rules put it. Before setup (no rules) it is an
/// ordinary window, and hiding it means closing it.
pub fn in_own_workspace() -> Result<bool> {
    Ok(own_window()?.1 == format!("special:{WORKSPACE}"))
}

/// Moves a window opened before the window rules existed (the first run, from the app launcher) to where the rules
/// would have put it, floating in the special workspace, and shows it there: from then on it behaves as it always will.
pub fn adopt() -> Result<()> {
    let (address, workspace) = own_window()?;
    if workspace == format!("special:{WORKSPACE}") {
        return Ok(());
    }
    dispatch(&format!(
        "hl.dsp.window.float({{ window = \"address:{address}\", action = \"on\" }})"
    ))?;
    dispatch(&format!(
        "hl.dsp.window.move({{ workspace = \"special:{WORKSPACE}\", follow = false, window = \"address:{address}\" }})"
    ))?;
    if !is_shown()? {
        toggle()?;
    }
    Ok(())
}

/// Sizes this process's window to the monitor showing the omafind workspace (or the focused one) and centers it: about
/// 60 % × 68 % of the usable area, never smaller than the layout needs nor wider than reads well.
pub fn fit_window() -> Result<()> {
    let monitors: serde_json::Value = serde_json::from_str(&command("j/monitors")?)?;
    let list = monitors.as_array().context("monitors")?;
    let monitor = list
        .iter()
        .find(|m| m["specialWorkspace"]["name"] == format!("special:{WORKSPACE}"))
        .or_else(|| list.iter().find(|m| m["focused"] == true))
        .context("no monitor")?;
    let scale = monitor["scale"].as_f64().unwrap_or(1.0).max(0.1);
    let rotated = monitor["transform"].as_i64().unwrap_or(0) % 2 == 1;
    let (mut w, mut h) = (
        monitor["width"].as_f64().unwrap_or(1920.0) / scale,
        monitor["height"].as_f64().unwrap_or(1080.0) / scale,
    );
    if rotated {
        std::mem::swap(&mut w, &mut h);
    }
    let reserved: Vec<f64> = monitor["reserved"]
        .as_array()
        .map(|r| r.iter().filter_map(|v| v.as_f64()).collect())
        .unwrap_or_default();
    if let [left, top, right, bottom] = reserved[..] {
        w -= left + right;
        h -= top + bottom;
    }
    let width = (w * 0.6).clamp(900.0_f64.min(w - 40.0), 1400.0).round();
    let height = (h * 0.68).clamp(600.0_f64.min(h - 40.0), 960.0).round();
    let (address, _) = own_window()?;
    dispatch(&format!(
        "hl.dsp.window.resize({{ x = {width}, y = {height}, relative = false, window = \"address:{address}\" }})"
    ))?;
    dispatch(&format!(
        "hl.dsp.window.center({{ window = \"address:{address}\" }})"
    ))
}

/// Whether the omafind special workspace is open on any monitor.
pub fn is_shown() -> Result<bool> {
    let json = command("j/monitors")?;
    Ok(json.contains(&format!("\"name\": \"special:{WORKSPACE}\"")))
}

/// What the event stream tells the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Shown,
    Hidden,
}

/// Follows Hyprland's events, reporting when the omafind special workspace opens and closes, until the socket closes.
pub fn events(mut on_event: impl FnMut(Event)) -> Result<()> {
    let stream = UnixStream::connect(instance_dir()?.join(".socket2.sock"))
        .context("connecting to Hyprland events")?;
    let reader = BufReader::new(stream);
    let mut shown = false;
    for line in reader.lines() {
        let line = line?;
        // `activespecial>>special:omafind,DP-1` when one opens, `activespecial>>,DP-1` when it closes (v2 adds the id).
        let Some(rest) = line.strip_prefix("activespecial>>") else {
            continue;
        };
        let name = rest.split(',').next().unwrap_or("");
        let now = name == format!("special:{WORKSPACE}");
        if now != shown {
            shown = now;
            on_event(if shown { Event::Shown } else { Event::Hidden });
        }
    }
    Ok(())
}

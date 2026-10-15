//! Hyprland's IPC: dispatch commands over `.socket.sock`, and follow events over `.socket2.sock`, both under
//! `$XDG_RUNTIME_DIR/hypr/<instance>/`. No polling: the event socket pushes.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};

/// The special workspace the window lives in; `dig toggle` shows and hides it.
pub const WORKSPACE: &str = "dig";
/// The window's app id, matched by the window rules.
pub const APP_ID: &str = "dig";

fn instance_dir() -> Result<PathBuf> {
    let runtime = dig_core::paths::runtime_dir();
    let signature = match std::env::var("HYPRLAND_INSTANCE_SIGNATURE") {
        Ok(s) if !s.is_empty() => s,
        _ => {
            // Not inherited (started from a service or an agent shell): the one running instance.
            let mut entries: Vec<_> = std::fs::read_dir(runtime.join("hypr")).context("no Hyprland instance")?.flatten().collect();
            entries.sort_by_key(|e| e.metadata().and_then(|m| m.modified()).ok());
            entries.last().map(|e| e.file_name().to_string_lossy().into_owned()).context("no Hyprland instance")?
        }
    };
    Ok(runtime.join("hypr").join(signature))
}

/// Runs one `hyprctl`-style command and returns its answer.
pub fn command(line: &str) -> Result<String> {
    let mut stream = UnixStream::connect(instance_dir()?.join(".socket.sock")).context("connecting to Hyprland")?;
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

/// Whether the dig special workspace is open on any monitor.
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

/// Follows Hyprland's events, reporting when the dig special workspace opens and closes, until the socket closes.
pub fn events(mut on_event: impl FnMut(Event)) -> Result<()> {
    let stream = UnixStream::connect(instance_dir()?.join(".socket2.sock")).context("connecting to Hyprland events")?;
    let reader = BufReader::new(stream);
    let mut shown = false;
    for line in reader.lines() {
        let line = line?;
        // `activespecial>>special:dig,DP-1` when one opens, `activespecial>>,DP-1` when it closes (v2 adds the id).
        let Some(rest) = line.strip_prefix("activespecial>>") else { continue };
        let name = rest.split(',').next().unwrap_or("");
        let now = name == format!("special:{WORKSPACE}");
        if now != shown {
            shown = now;
            on_event(if shown { Event::Shown } else { Event::Hidden });
        }
    }
    Ok(())
}

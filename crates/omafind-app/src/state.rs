//! What the window knows about the daemon and about itself being shown, kept as a GPUI global. The threads that follow
//! omafindd and Hyprland send notes to the main thread, which updates this; views observe it and redraw.

use gpui_kit::Global;
use omafind_client::{Root, Status};

#[derive(Default)]
pub struct AppState {
    pub status: Option<Status>,
    /// Whether omafindd answers.
    pub daemon: bool,
    /// Whether the window is shown (Hyprland's special workspace is open).
    pub visible: bool,
    /// Counts the times the window was shown; the search screen starts fresh on each.
    pub shown: u64,
    /// Whether a config with folders exists: a daemon restart must not look like a first run.
    pub configured: bool,
}

impl Global for AppState {}

impl AppState {
    pub fn roots(&self) -> &[Root] {
        self.status.as_ref().map_or(&[], |s| &s.roots)
    }

    pub fn files(&self) -> i64 {
        self.status.as_ref().map_or(0, |s| s.files)
    }

    pub fn indexed(&self) -> i64 {
        self.status.as_ref().map_or(0, |s| s.indexed)
    }

    pub fn pending(&self) -> i64 {
        self.status.as_ref().map_or(0, |s| s.pending)
    }
}

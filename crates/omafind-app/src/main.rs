//! `omafind-app`: the Omafind search window, in GPUI Kit.
//!
//! The process stays resident. Hyprland keeps the window in the `special:omafind` workspace (window rules from setup),
//! and `omafind toggle` shows or hides that workspace, so showing is instant. The app follows Hyprland's event socket to
//! know when it is shown (warm the query encoder, focus the field) and hidden (let it idle), omafindd's status stream
//! for the footer, and Omarchy's theme for its colors.

mod actions;
mod format;
mod home;
mod hypr;
mod portal;
mod preview;
mod results;
mod search;
mod service;
mod settings;
mod state;
mod theme;
mod widgets;

use std::time::Duration;

use gpui_kit::{
    AppContext as _, BorrowAppContext as _, Bounds, KeyBinding, TitlebarOptions, WindowBounds,
    WindowOptions, px, size,
};
use omafind_client::{Client, Request, Status};

use crate::search::{NextFilter, PreviousFilter, SearchView};
use crate::service::tell_daemon;
use crate::state::AppState;

/// Where threads send notes for the main thread (set once the app runs).
static NOTES: std::sync::OnceLock<async_channel::Sender<Note>> = std::sync::OnceLock::new();

fn note(note: Note) {
    if let Some(tx) = NOTES.get() {
        let _ = tx.send_blocking(note);
    }
}

enum Note {
    Status(Box<Status>),
    Daemon(bool),
    Shown,
    Hidden,
    Theme,
    /// Closed before it was ever set up: nothing keeps the process around.
    Quit,
}

/// Closes the special workspace if it is open; an ordinary window (opened from the launcher before setup added the
/// window rules) has nothing to hide into, so the app quits. Off a thread: Hyprland answers in a few ms, but never
/// block a frame.
pub(crate) fn hide_window() {
    std::thread::spawn(|| {
        if matches!(hypr::in_own_workspace(), Ok(false)) {
            note(Note::Quit);
        } else if hypr::is_shown().unwrap_or(false)
            && let Err(error) = hypr::toggle()
        {
            tracing::warn!("hide: {error:#}");
        }
    });
}

/// Follows the daemon's status stream. While it is down, waits for its socket to appear (inotify on the runtime
/// directory) rather than retrying on a timer.
fn follow_daemon(tx: async_channel::Sender<Note>) {
    use notify::Watcher;
    let socket = omafind_client::socket_path();
    let (appeared_tx, appeared) = std::sync::mpsc::channel::<()>();
    let name = socket.file_name().map(|n| n.to_owned());
    let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if let Ok(event) = event
            && matches!(event.kind, notify::EventKind::Create(_))
            && event.paths.iter().any(|p| p.file_name() == name.as_deref())
        {
            let _ = appeared_tx.send(());
        }
    });
    // Watched before the first attempt, so a socket made in between is not missed. Without a watcher, fall back to
    // a slow retry.
    let watching = match (watcher, socket.parent()) {
        (Ok(mut w), Some(dir)) => w
            .watch(dir, notify::RecursiveMode::NonRecursive)
            .is_ok()
            .then_some(w),
        _ => None,
    };
    loop {
        match Client::connect() {
            Ok(client) => {
                let _ = tx.send_blocking(Note::Daemon(true));
                let tx2 = tx.clone();
                let _ = client.subscribe(|event| {
                    if let omafind_client::Event::Status(status) = event {
                        return tx2.send_blocking(Note::Status(status)).is_ok();
                    }
                    true
                });
                let _ = tx.send_blocking(Note::Daemon(false));
            }
            Err(_) => {
                let _ = tx.send_blocking(Note::Daemon(false));
            }
        }
        if watching.is_some() {
            while appeared.try_recv().is_ok() {}
            if !socket.exists() {
                let _ = appeared.recv();
            }
            // The socket is bound before the daemon listens on it; a moment later it answers.
            std::thread::sleep(Duration::from_millis(200));
        } else {
            std::thread::sleep(Duration::from_secs(30));
        }
    }
}

fn main() {
    if let Some(arg) = std::env::args().nth(1) {
        if arg == "--version" {
            println!("omafind-app {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        eprintln!("omafind-app: unknown argument {arg}");
        std::process::exit(2);
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("omafind_app=info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let (tx, rx) = async_channel::unbounded::<Note>();
    let _ = NOTES.set(tx.clone());
    {
        let tx = tx.clone();
        std::thread::Builder::new()
            .name("omafind-daemon".into())
            .spawn(move || follow_daemon(tx))
            .expect("thread");
    }
    {
        let tx = tx.clone();
        std::thread::Builder::new()
            .name("omafind-hypr".into())
            .spawn(move || {
                if let Err(error) = hypr::events(|event| {
                    let _ = tx.send_blocking(match event {
                        hypr::Event::Shown => Note::Shown,
                        hypr::Event::Hidden => Note::Hidden,
                    });
                }) {
                    tracing::warn!("not following Hyprland: {error:#}");
                }
            })
            .expect("thread");
    }
    // The watcher lives as long as the process: dropping it would stop the inotify stream.
    {
        let tx = tx.clone();
        match theme::watch(move || {
            let _ = tx.send_blocking(Note::Theme);
        }) {
            Ok(watcher) => std::mem::forget(watcher),
            Err(error) => tracing::warn!("not watching the theme: {error}"),
        }
    }

    // The first search must not pay for loading the query encoder: ask for it now, and again on every show.
    tell_daemon(Request::Warm);
    let font = theme::font_family();

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            match theme::Palette::load() {
                Ok(palette) => palette.apply(&font, cx),
                Err(error) => {
                    tracing::warn!("no Omarchy theme ({error:#}): using the default palette")
                }
            }
            cx.set_global(AppState {
                configured: service::configured(),
                ..Default::default()
            });
            // Tab is focus traversal unless something nearer binds it: these bindings hold inside the window.
            cx.bind_keys([
                KeyBinding::new("tab", NextFilter, Some("Omafind")),
                KeyBinding::new("shift-tab", PreviousFilter, Some("Omafind")),
            ]);
            // A window closed by the compositor (SUPER + W) ends the process; the next toggle starts it again.
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(1120.), px(700.)),
                    cx,
                ))),
                titlebar: Some(TitlebarOptions {
                    title: Some("Omafind".into()),
                    ..Default::default()
                }),
                app_id: Some(hypr::APP_ID.into()),
                ..Default::default()
            };
            let (window, _view) = gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| SearchView::new(window, cx))
            })
            .expect("failed to open the Omafind window");

            // Notes from the threads land on the main thread here, as changes to the app state the views observe.
            let font = font.clone();
            cx.spawn(async move |cx| {
                while let Ok(note) = rx.recv().await {
                    cx.update(|cx| match note {
                        Note::Status(status) => cx.update_global::<AppState, _>(|s, _| {
                            s.status = Some(*status);
                            s.configured = service::configured();
                        }),
                        Note::Daemon(up) => cx.update_global::<AppState, _>(|s, _| {
                            s.daemon = up;
                            s.configured = service::configured();
                        }),
                        Note::Shown => {
                            cx.update_global::<AppState, _>(|s, _| {
                                s.visible = true;
                                s.shown += 1;
                            });
                            tell_daemon(Request::Warm);
                            let _ = window.update(cx, |_, window, _| window.activate_window());
                            std::thread::spawn(|| {
                                if let Err(error) = hypr::fit_window() {
                                    tracing::warn!("fit: {error:#}");
                                }
                                if let Err(error) = hypr::focus_window() {
                                    tracing::warn!("focus: {error:#}");
                                }
                            });
                        }
                        Note::Hidden => {
                            cx.update_global::<AppState, _>(|s, _| s.visible = false);
                            tell_daemon(Request::Idle);
                        }
                        Note::Quit => cx.quit(),
                        Note::Theme => match theme::Palette::load() {
                            Ok(palette) => {
                                tracing::info!("theme changed: retinting");
                                palette.apply(&font, cx);
                            }
                            Err(error) => tracing::warn!("theme: {error:#}"),
                        },
                    });
                }
            })
            .detach();

            cx.activate(true);
            let _ = window.update(cx, |_, window, _| window.activate_window());
        });
}

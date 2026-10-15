//! `omafindd`: the daemon. One process, two jobs. The *coordinator* crawls and watches the chosen folders, keeps the
//! index's file list current, decides when indexing may run (not paused, not in power-saver), and answers the
//! socket. The *worker* is a child of the same binary (`omafindd --worker`) that loads the media encoders, embeds
//! everything pending and exits, so the heavy model's memory goes back to the system when there is nothing to do.
//! Searches run in the daemon with the text-only encoder, loaded on demand and unloaded when idle.
//!
//! No polling anywhere: inotify for files, D-Bus signals for power, the socket for the window and the CLI.

mod daemon;
mod power;
mod searcher;
mod server;
mod watcher;
mod worker;

use std::io::Write;

/// A log line to stderr (the journal, under systemd).
pub fn log(message: std::fmt::Arguments) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let _ = writeln!(std::io::stderr(), "[{}] {message}", now % 86400);
}

#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => { $crate::log(format_args!($($arg)*)) };
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("--worker") => worker::main(),
        Some("--version") => {
            println!("omafindd {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some(other) => Err(anyhow::anyhow!(
            "unknown argument {other}; usage: omafindd [--worker]"
        )),
        None => daemon::main(),
    };
    if let Err(error) = result {
        log!("omafindd: {error:#}");
        std::process::exit(1);
    }
}

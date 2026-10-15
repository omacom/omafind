//! inotify on the roots, debounced: a burst of writes becomes one `Msg::Changed` a second after it ends.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher};

use crate::daemon::Msg;

const QUIET: Duration = Duration::from_secs(1);

/// Watches `roots` until the watcher is dropped (returned handle) or the channel closes.
pub fn start(roots: Vec<PathBuf>, tx: Sender<Msg>) -> notify::Result<notify::RecommendedWatcher> {
    let (raw_tx, raw_rx) = channel::<notify::Result<notify::Event>>();
    let roots_for_filter = roots.clone();
    let mut watcher = notify::recommended_watcher(move |event| {
        let _ = raw_tx.send(event);
    })?;
    for root in &roots {
        if let Err(error) = watcher.watch(root, RecursiveMode::Recursive) {
            crate::log!("can't watch {}: {error}", root.display());
        }
    }
    std::thread::Builder::new()
        .name("dig-watch".into())
        .spawn(move || {
            let roots = roots_for_filter;
            let mut pending: HashSet<PathBuf> = HashSet::new();
            let mut last = Instant::now();
            loop {
                let timeout = if pending.is_empty() {
                    Duration::from_secs(3600)
                } else {
                    QUIET.saturating_sub(last.elapsed())
                };
                match raw_rx.recv_timeout(timeout) {
                    Ok(Ok(event)) => {
                        if !matters(&event.kind) {
                            continue;
                        }
                        for path in event.paths {
                            // Hidden and skipped directories below a root never matter.
                            let below_root = roots.iter().find_map(|r| path.strip_prefix(r).ok());
                            if below_root.is_none_or(|rel| {
                                rel.components().any(|c| {
                                    c.as_os_str()
                                        .to_str()
                                        .is_some_and(dig_index::rules::skip_dir)
                                })
                            }) {
                                continue;
                            }
                            pending.insert(path);
                        }
                        last = Instant::now();
                    }
                    Ok(Err(error)) => crate::log!("watch error: {error}"),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                }
                if !pending.is_empty() && last.elapsed() >= QUIET {
                    let batch: Vec<PathBuf> = pending.drain().collect();
                    if tx.send(Msg::Changed(batch)).is_err() {
                        return;
                    }
                }
            }
        })
        .expect("watcher thread");
    Ok(watcher)
}

fn matters(kind: &notify::EventKind) -> bool {
    use notify::EventKind::*;
    matches!(kind, Create(_) | Modify(_) | Remove(_) | Any)
}

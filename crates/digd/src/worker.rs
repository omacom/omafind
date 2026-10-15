//! `digd --worker`: loads the full model, indexes everything pending, streams progress as JSON lines on stdout and
//! exits. Commands arrive on stdin: `pause`, `resume`, `quit`.

use std::io::{BufRead, Write};
use std::sync::{Arc, Condvar, Mutex};

use anyhow::Result;
use dig_embed::{Embedder, Models, Options};
use dig_index::{Event, Indexer};
use dig_store::Store;

#[derive(serde::Serialize, serde::Deserialize, Debug)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Line {
    Ready {
        seconds: f64,
        device: String,
    },
    Reading {
        path: String,
        kind: String,
    },
    Indexed {
        path: String,
        kind: String,
        chunks: usize,
    },
    Failed {
        path: String,
        error: String,
    },
    Done {
        indexed: usize,
        failed: usize,
        seconds: f64,
    },
}

#[derive(Default)]
struct Control {
    paused: bool,
    quit: bool,
    /// The parent saw new pending files while this worker was lingering.
    more: bool,
}

/// How long a worker stays loaded after its queue empties, in case the next file is seconds away.
const LINGER: std::time::Duration = std::time::Duration::from_secs(60);

pub fn main() -> Result<()> {
    let config = dig_core::config::Config::load().unwrap_or_default();
    let control = Arc::new((Mutex::new(Control::default()), Condvar::new()));
    {
        let control = control.clone();
        std::thread::spawn(move || {
            let stdin = std::io::stdin();
            for line in stdin.lock().lines() {
                let Ok(line) = line else { break };
                let (lock, cv) = &*control;
                let mut c = lock.lock().unwrap();
                match line.trim() {
                    "pause" => c.paused = true,
                    "resume" => c.paused = false,
                    "more" => c.more = true,
                    "quit" => c.quit = true,
                    _ => {}
                }
                cv.notify_all();
            }
            // The parent went away: stop after the current file.
            let (lock, cv) = &*control;
            lock.lock().unwrap().quit = true;
            cv.notify_all();
        });
    }

    let index_path = std::env::var_os("DIG_INDEX")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(dig_core::paths::index_path);
    let mut store = Store::open(&index_path)?;
    let mut code_store = Store::open(&crate::daemon::code_index_path(&index_path))?;
    let models = Models::locate(&dig_core::model::dir())?;
    let cpu = config.search.device == "cpu";
    let started = std::time::Instant::now();
    let mut embedder = Embedder::new(
        &models,
        &Options {
            text_only: false,
            cpu_only: cpu,
            ..Options::default()
        },
    )?;
    emit(&Line::Ready {
        seconds: started.elapsed().as_secs_f64(),
        device: if dig_embed::gpu_found() && !cpu {
            "gpu".into()
        } else {
            "cpu".into()
        },
    });
    store.set_meta("llama_version", dig_embed::LLAMA_VERSION)?;

    let gate_control = control.clone();
    let on_event = |event: Event| {
        let line = match event {
            Event::Crawled { .. } => return,
            Event::Reading { path, kind } => Line::Reading {
                path: path.to_string_lossy().into_owned(),
                kind: kind.as_str().into(),
            },
            Event::Indexed { path, kind, chunks } => Line::Indexed {
                path: path.to_string_lossy().into_owned(),
                kind: kind.as_str().into(),
                chunks,
            },
            Event::Failed { path, error } => Line::Failed {
                path: path.to_string_lossy().into_owned(),
                error,
            },
            Event::Done {
                indexed,
                failed,
                seconds,
            } => Line::Done {
                indexed,
                failed,
                seconds,
            },
        };
        emit(&line);
    };
    let gate = move || {
        let (lock, cv) = &*gate_control;
        let mut c = lock.lock().unwrap();
        while c.paused && !c.quit {
            c = cv.wait(c).unwrap();
        }
        !c.quit
    };
    loop {
        // The main index first (what people look for most), then code.
        for store in [&mut store, &mut code_store] {
            let mut indexer = Indexer {
                store,
                embedder: &mut embedder,
                on_event: Box::new(on_event),
                gate: Box::new(gate.clone()),
            };
            indexer.index_pending()?;
        }
        // Linger: wait for more work, or go away after a while so the model's memory is freed.
        let (lock, cv) = &*control;
        let mut c = lock.lock().unwrap();
        c.more = false;
        let deadline = std::time::Instant::now() + LINGER;
        loop {
            if c.quit {
                return Ok(());
            }
            if c.more {
                break;
            }
            let now = std::time::Instant::now();
            if now >= deadline {
                return Ok(());
            }
            c = cv.wait_timeout(c, deadline - now).unwrap().0;
        }
    }
}

fn emit(line: &Line) {
    let mut out = std::io::stdout().lock();
    let _ = serde_json::to_writer(&mut out, line);
    let _ = out.write_all(b"\n");
    let _ = out.flush();
}

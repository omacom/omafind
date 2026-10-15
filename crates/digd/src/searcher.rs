//! The query encoder: the text-only model, loaded on the first search (or when the window is about to show) and
//! unloaded after a while without one. Searches run here, in the daemon, so the window gets an answer in tens of
//! milliseconds.

use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use dig_core::{FileKind, Hit};
use dig_embed::{Embedder, Models, Options};
use dig_index::SearchOptions;
use dig_store::Store;

struct Inner {
    embedder: Option<Embedder>,
    last_used: Instant,
    /// A hide → the encoder may go sooner.
    idle_since: Option<Instant>,
}

pub struct Searcher {
    inner: Mutex<Inner>,
    wake: Condvar,
    store: Mutex<Store>,
    code_store: Mutex<Store>,
    pub index_path: PathBuf,
    cpu: bool,
    unload_after: Duration,
}

/// How long after the window hides the encoder stays loaded.
const HIDE_GRACE: Duration = Duration::from_secs(120);

impl Searcher {
    pub fn new(index_path: PathBuf, code_index_path: PathBuf, cpu: bool, unload_after_minutes: u64) -> Result<Arc<Self>> {
        let store = Store::open(&index_path)?;
        let code_store = Store::open(&code_index_path)?;
        let searcher = Arc::new(Self {
            inner: Mutex::new(Inner { embedder: None, last_used: Instant::now(), idle_since: None }),
            wake: Condvar::new(),
            store: Mutex::new(store),
            code_store: Mutex::new(code_store),
            index_path,
            cpu,
            unload_after: Duration::from_secs(unload_after_minutes.max(1) * 60),
        });
        let keep = if unload_after_minutes == 0 { None } else { Some(searcher.clone()) };
        if let Some(s) = keep {
            std::thread::Builder::new().name("dig-unload".into()).spawn(move || s.unloader())?;
        }
        Ok(searcher)
    }

    pub fn is_loaded(&self) -> bool {
        self.inner.lock().unwrap().embedder.is_some()
    }

    /// Loads the encoder if it isn't loaded.
    pub fn warm(&self) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        inner.idle_since = None;
        inner.last_used = Instant::now();
        if inner.embedder.is_none() {
            let t = Instant::now();
            let models = Models::locate(&dig_core::model::dir())?;
            inner.embedder = Some(Embedder::new(&models, &Options { text_only: true, cpu_only: self.cpu, threads: 0 }).context("loading the query encoder")?);
            crate::log!("query encoder loaded in {:.2} s", t.elapsed().as_secs_f64());
        }
        self.wake.notify_all();
        Ok(())
    }

    /// The window hid: unload after a short grace period rather than the full idle time.
    pub fn idle(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.idle_since = Some(Instant::now());
        self.wake.notify_all();
    }

    /// A query starting with `code:` searches the code index with the code-retrieval prompt; anything else never
    /// sees code.
    pub fn search(&self, query: &str, limit: usize, kinds: Vec<FileKind>) -> Result<Vec<Hit>> {
        self.warm()?;
        let mut inner = self.inner.lock().unwrap();
        inner.last_used = Instant::now();
        let embedder = inner.embedder.as_mut().expect("warmed");
        match dig_index::code_query(query) {
            Some(code) => {
                let store = self.code_store.lock().unwrap();
                dig_index::search(&store, embedder, code, &SearchOptions { limit, kinds: Vec::new(), candidates: 200, code: true })
            }
            None => {
                let store = self.store.lock().unwrap();
                dig_index::search(&store, embedder, query, &SearchOptions { limit, kinds, candidates: 200, code: false })
            }
        }
    }

    /// Sleeps until the encoder has been idle long enough, then drops it. Woken by every use, so it never polls.
    fn unloader(self: Arc<Self>) {
        let mut inner = self.inner.lock().unwrap();
        loop {
            let deadline = match (&inner.embedder, inner.idle_since) {
                (None, _) => None,
                (Some(_), Some(hidden)) => Some((hidden + HIDE_GRACE).min(inner.last_used + self.unload_after)),
                (Some(_), None) => Some(inner.last_used + self.unload_after),
            };
            match deadline {
                None => inner = self.wake.wait(inner).unwrap(),
                Some(deadline) => {
                    let now = Instant::now();
                    if now >= deadline {
                        inner.embedder = None;
                        inner.idle_since = None;
                        crate::log!("query encoder unloaded (idle)");
                    } else {
                        inner = self.wake.wait_timeout(inner, deadline - now).unwrap().0;
                    }
                }
            }
        }
    }
}

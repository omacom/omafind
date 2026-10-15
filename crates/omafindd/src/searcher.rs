//! Searches: the text-only query encoder and the in-memory vector matrices, loaded on the first search (or when the
//! window is about to show) and dropped together after a while without one. Searches run here, in the daemon, so the
//! window gets an answer in tens of milliseconds.
//!
//! Two stages per keystroke: a quick one (names and keywords, never waits for the model, on its own connections) and
//! the full one. Full searches from one session queue for the encoder; when a newer one is already waiting, the older
//! answers `Superseded` instead of embedding a query nobody is looking at any more.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use omafind_client::{Recents, Reply};
use omafind_core::FileKind;
use omafind_embed::{Embedder, Models, Options};
use omafind_store::{Store, VectorIndex};

struct Inner {
    embedder: Option<Embedder>,
    last_used: Instant,
    /// A hide → the encoder may go sooner.
    idle_since: Option<Instant>,
}

/// The index and the code index, with their matrices once loaded.
struct Stores {
    main: Store,
    code: Store,
    vectors: Option<(VectorIndex, VectorIndex)>,
}

pub struct Searcher {
    inner: Mutex<Inner>,
    wake: Condvar,
    full: Mutex<Stores>,
    quick: Mutex<(Store, Store)>,
    /// Embedded queries, newest last.
    cache: Mutex<VecDeque<(String, Vec<f32>)>>,
    tickets: AtomicU64,
    latest: Mutex<HashMap<String, u64>>,
    pub index_path: PathBuf,
    cpu: bool,
    unload_after: Duration,
}

/// How long after the window hides the encoder stays loaded.
const HIDE_GRACE: Duration = Duration::from_secs(120);
const CACHE: usize = 128;

pub struct Query<'a> {
    pub text: &'a str,
    pub limit: usize,
    pub kinds: Vec<FileKind>,
    pub quick: bool,
    pub session: Option<&'a str>,
}

impl Searcher {
    pub fn new(
        index_path: PathBuf,
        code_index_path: PathBuf,
        cpu: bool,
        unload_after_minutes: u64,
    ) -> Result<Arc<Self>> {
        let searcher = Arc::new(Self {
            inner: Mutex::new(Inner {
                embedder: None,
                last_used: Instant::now(),
                idle_since: None,
            }),
            wake: Condvar::new(),
            full: Mutex::new(Stores {
                main: Store::open(&index_path)?,
                code: Store::open(&code_index_path)?,
                vectors: None,
            }),
            quick: Mutex::new((Store::open(&index_path)?, Store::open(&code_index_path)?)),
            cache: Mutex::new(VecDeque::new()),
            tickets: AtomicU64::new(0),
            latest: Mutex::new(HashMap::new()),
            index_path,
            cpu,
            unload_after: Duration::from_secs(unload_after_minutes.max(1) * 60),
        });
        let keep = if unload_after_minutes == 0 {
            None
        } else {
            Some(searcher.clone())
        };
        if let Some(s) = keep {
            std::thread::Builder::new()
                .name("omafind-unload".into())
                .spawn(move || s.unloader())?;
        }
        Ok(searcher)
    }

    pub fn is_loaded(&self) -> bool {
        self.inner.lock().unwrap().embedder.is_some()
    }

    /// Vectors held in memory and their size in bytes.
    pub fn vectors_loaded(&self) -> (usize, usize) {
        match &self.full.lock().unwrap().vectors {
            Some((a, b)) => (a.len() + b.len(), a.bytes() + b.bytes()),
            None => (0, 0),
        }
    }

    /// Loads the encoder and the matrices if they aren't loaded.
    pub fn warm(&self) -> Result<()> {
        {
            let mut inner = self.inner.lock().unwrap();
            inner.idle_since = None;
            inner.last_used = Instant::now();
            if inner.embedder.is_none() {
                let t = Instant::now();
                let models = Models::locate(&omafind_core::model::dir())?;
                inner.embedder = Some(
                    Embedder::new(
                        &models,
                        &Options {
                            text_only: true,
                            cpu_only: self.cpu,
                            ..Options::default()
                        },
                    )
                    .context("loading the query encoder")?,
                );
                crate::log!("query encoder loaded in {:.2} s", t.elapsed().as_secs_f64());
            }
        }
        self.wake.notify_all();
        let mut stores = self.full.lock().unwrap();
        if stores.vectors.is_none() {
            let t = Instant::now();
            let main = VectorIndex::load(&stores.main)?;
            let code = VectorIndex::load(&stores.code)?;
            crate::log!(
                "{} vectors loaded in {:.2} s ({} MB)",
                main.len() + code.len(),
                t.elapsed().as_secs_f64(),
                (main.bytes() + code.bytes()) >> 20
            );
            stores.vectors = Some((main, code));
        }
        Ok(())
    }

    /// The window hid: unload after a short grace period rather than the full idle time.
    pub fn idle(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.idle_since = Some(Instant::now());
        self.wake.notify_all();
    }

    fn cached(&self, prompted: &str) -> Option<Vec<f32>> {
        let mut cache = self.cache.lock().unwrap();
        let i = cache.iter().position(|(k, _)| k == prompted)?;
        let entry = cache.remove(i)?;
        let v = entry.1.clone();
        cache.push_back(entry);
        Some(v)
    }

    fn remember(&self, prompted: String, vector: Vec<f32>) {
        let mut cache = self.cache.lock().unwrap();
        cache.push_back((prompted, vector));
        while cache.len() > CACHE {
            cache.pop_front();
        }
    }

    /// The query vector, from the cache or the encoder. `None`: superseded while waiting.
    fn embed(&self, prompted: String, ticket: Option<(&str, u64)>) -> Result<Option<Vec<f32>>> {
        if let Some(v) = self.cached(&prompted) {
            return Ok(Some(v));
        }
        self.warm()?;
        let mut inner = self.inner.lock().unwrap();
        if let Some((session, ticket)) = ticket
            && self
                .latest
                .lock()
                .unwrap()
                .get(session)
                .is_some_and(|&t| t > ticket)
        {
            return Ok(None);
        }
        inner.last_used = Instant::now();
        let embedder = inner.embedder.as_mut().expect("warmed");
        let vector = embedder
            .embed_texts(std::slice::from_ref(&prompted))?
            .remove(0);
        drop(inner);
        self.remember(prompted, vector.clone());
        Ok(Some(vector))
    }

    /// A search as typed (filters included). `code:` / `type:code` searches the code index; nothing else sees code.
    pub fn search(&self, q: Query) -> Result<Reply> {
        let started = Instant::now();
        let (text, options, chips) = omafind_index::plan(q.text, &q.kinds, q.limit);
        if q.quick {
            let stores = self.quick.lock().unwrap();
            let store = if options.code { &stores.1 } else { &stores.0 };
            let answer = omafind_index::rank(store, None, &text, &options)?;
            return Ok(Reply {
                hits: answer.hits,
                took_ms: started.elapsed().as_secs_f64() * 1000.0,
                counts: answer.counts,
                chips,
                quick: true,
                superseded: false,
            });
        }
        let ticket = q.session.map(|s| {
            let t = self.tickets.fetch_add(1, Ordering::SeqCst) + 1;
            self.latest.lock().unwrap().insert(s.to_owned(), t);
            (s, t)
        });
        let vector = if text.trim().is_empty() {
            None
        } else {
            match self.embed(omafind_index::prompted(&text, options.code), ticket)? {
                Some(v) => Some(v),
                None => {
                    return Ok(Reply {
                        superseded: true,
                        ..Reply::default()
                    });
                }
            }
        };
        self.warm()?;
        let mut stores = self.full.lock().unwrap();
        let Stores {
            main,
            code,
            vectors,
        } = &mut *stores;
        let (main_index, code_index) = vectors.as_mut().expect("warmed");
        let (store, index) = if options.code {
            (&*code, code_index)
        } else {
            (&*main, main_index)
        };
        index.refresh(store)?;
        let answer = omafind_index::rank(
            store,
            vector.as_deref().map(|v| (&*index, v)),
            &text,
            &options,
        )?;
        if !q.text.trim().is_empty() {
            let _ = main.record_search(q.text);
        }
        Ok(Reply {
            hits: answer.hits,
            took_ms: started.elapsed().as_secs_f64() * 1000.0,
            counts: answer.counts,
            chips,
            quick: false,
            superseded: false,
        })
    }

    pub fn similar(&self, chunk_id: i64, limit: usize, code: bool) -> Result<Reply> {
        let started = Instant::now();
        self.warm()?;
        let mut stores = self.full.lock().unwrap();
        let Stores {
            main,
            code: code_store,
            vectors,
        } = &mut *stores;
        let (main_index, code_index) = vectors.as_mut().expect("warmed");
        let (store, index) = if code {
            (&*code_store, code_index)
        } else {
            (&*main, main_index)
        };
        index.refresh(store)?;
        let hits = omafind_index::similar(store, index, chunk_id, limit)?;
        Ok(Reply {
            hits,
            took_ms: started.elapsed().as_secs_f64() * 1000.0,
            ..Reply::default()
        })
    }

    pub fn opened(&self, path: &std::path::Path) -> Result<()> {
        let stores = self.quick.lock().unwrap();
        stores.0.record_open(path)?;
        stores.1.record_open(path)
    }

    pub fn context(&self, chunk_id: i64, code: bool) -> Result<omafind_client::Passage> {
        let stores = self.quick.lock().unwrap();
        let store = if code { &stores.1 } else { &stores.0 };
        let text = store
            .chunks(&[chunk_id])?
            .into_iter()
            .next()
            .map(|r| r.text)
            .unwrap_or_default();
        let (before, after) = store.neighbours(chunk_id)?;
        Ok(omafind_client::Passage {
            text,
            before,
            after,
        })
    }

    pub fn recents(&self) -> Result<Recents> {
        let stores = self.quick.lock().unwrap();
        let store = &stores.0;
        Ok(Recents {
            searches: store.recent_searches(8)?,
            opened: omafind_index::hits_for(store, &store.most_opened(6)?)?,
            added: omafind_index::hits_for(store, &store.recently_added(10)?)?,
        })
    }

    /// Sleeps until the encoder has been idle long enough, then drops it and the matrices. Woken by every use, so
    /// it never polls.
    fn unloader(self: Arc<Self>) {
        let mut inner = self.inner.lock().unwrap();
        loop {
            let deadline = match (&inner.embedder, inner.idle_since) {
                (None, _) => None,
                (Some(_), Some(hidden)) => {
                    Some((hidden + HIDE_GRACE).min(inner.last_used + self.unload_after))
                }
                (Some(_), None) => Some(inner.last_used + self.unload_after),
            };
            match deadline {
                None => inner = self.wake.wait(inner).unwrap(),
                Some(deadline) => {
                    let now = Instant::now();
                    if now >= deadline {
                        inner.embedder = None;
                        inner.idle_since = None;
                        self.full.lock().unwrap().vectors = None;
                        self.cache.lock().unwrap().clear();
                        crate::log!("query encoder and vectors unloaded (idle)");
                    } else {
                        inner = self.wake.wait_timeout(inner, deadline - now).unwrap().0;
                    }
                }
            }
        }
    }
}

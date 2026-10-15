//! The coordinator: owns the file list, the worker child and the power/pause state; everything else sends it
//! messages.

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Context, Result};
use omafind_client::{Event, Root, Status};
use omafind_core::config::Config;
use omafind_index::crawler::crawl;
use omafind_store::Store;

use crate::power::PowerState;
use crate::searcher::Searcher;
use crate::worker::Line;

pub enum Msg {
    Changed(Vec<PathBuf>),
    Crawl,
    Pause,
    Resume,
    SetRoots {
        folders: Vec<PathBuf>,
        code: Vec<PathBuf>,
    },
    Power(PowerState),
    Worker(Line),
    WorkerExited,
    Shutdown,
}

/// What every thread may read: the state the status reports, and where to send events.
#[derive(Default)]
pub struct Shared {
    pub state: Mutex<State>,
    pub subscribers: Mutex<Vec<Sender<Event>>>,
}

#[derive(Default, Clone)]
pub struct State {
    pub paused: bool,
    /// Setting up the folder watches (one inotify watch per directory): seconds on a big tree.
    pub starting: bool,
    pub power: PowerState,
    pub worker_running: bool,
    pub crawling: bool,
    pub current: Option<String>,
    pub roots: Vec<Root>,
    pub config: Config,
}

impl State {
    fn power_ok(&self) -> bool {
        !(self.config.power.pause_in_power_saver && self.power.power_saver)
            && !(self.config.power.wait_for_ac && self.power.on_battery)
    }

    pub fn state_name(&self) -> &'static str {
        if self.paused {
            "paused"
        } else if self.config.power.wait_for_ac && self.power.on_battery {
            "waiting_for_ac"
        } else if self.config.power.pause_in_power_saver && self.power.power_saver {
            "paused_power_saver"
        } else if self.starting {
            "starting"
        } else if self.crawling {
            "crawling"
        } else if self.worker_running {
            "indexing"
        } else {
            "idle"
        }
    }
}

/// The code index lives beside the main one: `index.sqlite` → `code.sqlite`.
pub fn code_index_path(index_path: &std::path::Path) -> PathBuf {
    index_path.with_file_name("code.sqlite")
}

/// The status, from the shared state and a store connection (code index counts included).
pub fn status(
    shared: &Shared,
    store: &Store,
    code_store: &Store,
    searcher: &Searcher,
) -> Result<Status> {
    let state = shared.state.lock().unwrap().clone();
    let stats = store.stats()?;
    let code = code_store.stats()?;
    let mut kinds: Vec<(omafind_core::FileKind, i64, i64)> = Vec::new();
    for (kind, done, total) in stats.progress.iter().chain(&code.progress) {
        let Some(kind) = omafind_core::FileKind::parse(kind) else {
            continue;
        };
        match kinds.iter_mut().find(|(k, _, _)| *k == kind) {
            Some(entry) => {
                entry.1 += done;
                entry.2 += total;
            }
            None => kinds.push((kind, *done, *total)),
        }
    }
    kinds.sort_by_key(|(k, _, _)| k.priority());
    let pending = stats.pending + code.pending;
    let done = stats.indexed + stats.failed + code.indexed + code.failed;
    let eta_seconds = {
        static RUN: Mutex<Option<(Instant, i64)>> = Mutex::new(None);
        let mut run = RUN.lock().unwrap();
        if pending == 0 {
            *run = None;
            None
        } else {
            let (since, at) = *run.get_or_insert((Instant::now(), done));
            let elapsed = since.elapsed().as_secs_f64();
            let rate = (done - at) as f64 / elapsed.max(1e-3);
            (elapsed > 20.0 && rate > 0.0).then(|| pending as f64 / rate)
        }
    };
    let (vectors_in_memory, vectors_bytes) = searcher.vectors_loaded();
    Ok(Status {
        version: env!("CARGO_PKG_VERSION").into(),
        roots: state.roots.clone(),
        files: stats.files + code.files,
        indexed: stats.indexed + code.indexed,
        pending: stats.pending + code.pending,
        failed: stats.failed + code.failed,
        chunks: stats.chunks + code.chunks,
        state: state.state_name().into(),
        current: state.current.clone(),
        paused: state.paused,
        on_battery: state.power.on_battery,
        power_saver: state.power.power_saver,
        query_encoder_loaded: searcher.is_loaded(),
        worker_running: state.worker_running,
        llama_version: store.get_meta("llama_version")?.unwrap_or_default(),
        vector_version: store.vector_version()?,
        kinds,
        eta_seconds,
        vectors_in_memory,
        vectors_bytes,
    })
}

pub fn broadcast(shared: &Shared, event: Event) {
    let mut subscribers = shared.subscribers.lock().unwrap();
    subscribers.retain(|s| s.send(event.clone()).is_ok());
}

struct Worker {
    child: Child,
    stdin: std::process::ChildStdin,
}

struct Coordinator {
    shared: Arc<Shared>,
    searcher: Arc<Searcher>,
    store: Store,
    code_store: Store,
    worker: Option<Worker>,
    tx: Sender<Msg>,
    watcher: Option<notify::RecommendedWatcher>,
    last_status: Instant,
}

pub fn main() -> Result<()> {
    let config = Config::load()?;
    let index_path = std::env::var_os("OMAFIND_INDEX")
        .map(PathBuf::from)
        .unwrap_or_else(omafind_core::paths::index_path);
    let code_index_path = code_index_path(&index_path);
    let store = Store::open(&index_path)?;
    let code_store = Store::open(&code_index_path)?;
    for s in [&store, &code_store] {
        if s.vector_version()? != omafind_core::model::VECTOR_VERSION {
            crate::log!(
                "vectors {} → {}: embedding everything again",
                s.vector_version()?,
                omafind_core::model::VECTOR_VERSION
            );
            s.reset_all()?;
            s.set_meta(
                "vector_version",
                &omafind_core::model::VECTOR_VERSION.to_string(),
            )?;
        }
    }
    let shared = Arc::new(Shared::default());
    let cpu = config.search.device == "cpu";
    let searcher = Searcher::new(
        index_path.clone(),
        code_index_path,
        cpu,
        config.search.unload_after_minutes,
    )?;
    let (tx, rx) = channel::<Msg>();

    // Roots: the config's, else what the store remembers from `omafind index`.
    let mut roots: Vec<Root> = config
        .folders()
        .into_iter()
        .map(|path| Root { path, code: false })
        .collect();
    roots.extend(
        config
            .code_roots()
            .into_iter()
            .map(|path| Root { path, code: true }),
    );
    if roots.is_empty() {
        roots = store
            .roots()?
            .into_iter()
            .map(|(path, code)| Root { path, code })
            .collect();
    }
    {
        let mut state = shared.state.lock().unwrap();
        state.paused = store.get_meta("paused")?.as_deref() == Some("1");
        state.starting = true;
        state.roots = roots.clone();
        state.config = config.clone();
    }

    // Threads: the socket server, the power watcher, signals.
    crate::server::start(shared.clone(), searcher.clone(), tx.clone())?;
    {
        let tx = tx.clone();
        std::thread::Builder::new()
            .name("omafind-power".into())
            .spawn(move || crate::power::watch(tx))?;
    }
    {
        let tx = tx.clone();
        let mut signals = signal_hook::iterator::Signals::new([
            signal_hook::consts::SIGTERM,
            signal_hook::consts::SIGINT,
            signal_hook::consts::SIGHUP,
        ])?;
        std::thread::Builder::new()
            .name("omafind-signals".into())
            .spawn(move || {
                for _ in signals.forever() {
                    let _ = tx.send(Msg::Shutdown);
                }
            })?;
    }

    let mut coordinator = Coordinator {
        shared,
        searcher,
        store,
        code_store,
        worker: None,
        tx: tx.clone(),
        watcher: None,
        last_status: Instant::now(),
    };
    coordinator.watch_roots();
    // The first crawl is queued below: report it as begun, so nobody sees an idle daemon in between.
    coordinator.shared.state.lock().unwrap().crawling = true;
    crate::log!(
        "omafindd {} up: {} roots, index {}",
        env!("CARGO_PKG_VERSION"),
        roots.len(),
        omafind_core::paths::tilde(&index_path)
    );
    let _ = tx.send(Msg::Crawl);
    coordinator.run(rx)
}

impl Coordinator {
    fn run(&mut self, rx: Receiver<Msg>) -> Result<()> {
        for msg in rx {
            match msg {
                Msg::Changed(paths) => self.changed(paths)?,
                Msg::Crawl => self.crawl_all()?,
                Msg::Pause => {
                    self.set_paused(true)?;
                }
                Msg::Resume => {
                    self.set_paused(false)?;
                }
                Msg::SetRoots { folders, code } => self.set_roots(folders, code)?,
                Msg::Power(power) => {
                    let changed = {
                        let mut state = self.shared.state.lock().unwrap();
                        let changed = state.power != power;
                        state.power = power;
                        changed
                    };
                    if changed {
                        crate::log!(
                            "power: battery {} power-saver {}",
                            power.on_battery,
                            power.power_saver
                        );
                        self.apply_gate();
                        self.send_status()?;
                    }
                }
                Msg::Worker(line) => self.worker_line(line)?,
                Msg::WorkerExited => {
                    if let Some(mut worker) = self.worker.take() {
                        let _ = worker.child.wait();
                    }
                    self.shared.state.lock().unwrap().worker_running = false;
                    self.shared.state.lock().unwrap().current = None;
                    // Files that landed while it was finishing.
                    self.maybe_start_worker()?;
                    self.send_status()?;
                }
                Msg::Shutdown => {
                    crate::log!("shutting down");
                    if let Some(mut worker) = self.worker.take() {
                        let _ = worker.stdin.write_all(b"quit\n");
                        let _ = worker.child.wait();
                    }
                    let _ = std::fs::remove_file(omafind_client::socket_path());
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    fn watch_roots(&mut self) {
        let roots: Vec<PathBuf> = self
            .shared
            .state
            .lock()
            .unwrap()
            .roots
            .iter()
            .map(|r| r.path.clone())
            .collect();
        self.watcher = None;
        // One inotify watch per directory takes a while on a big tree: say so rather than look idle.
        self.shared.state.lock().unwrap().starting = true;
        let _ = self.send_status();
        let started = crate::watcher::start(roots, self.tx.clone());
        self.shared.state.lock().unwrap().starting = false;
        match started {
            Ok(watcher) => self.watcher = Some(watcher),
            Err(error) => crate::log!("no file watcher: {error}"),
        }
    }

    fn set_roots(&mut self, folders: Vec<PathBuf>, code: Vec<PathBuf>) -> Result<()> {
        let mut roots: Vec<Root> = folders
            .into_iter()
            .map(|path| Root { path, code: false })
            .collect();
        roots.extend(code.into_iter().map(|path| Root { path, code: true }));
        {
            let mut state = self.shared.state.lock().unwrap();
            state.config.set_folders(
                &roots
                    .iter()
                    .filter(|r| !r.code)
                    .map(|r| r.path.clone())
                    .collect::<Vec<_>>(),
                &roots
                    .iter()
                    .filter(|r| r.code)
                    .map(|r| r.path.clone())
                    .collect::<Vec<_>>(),
            );
            if let Err(error) = state.config.save() {
                crate::log!("can't save config: {error:#}");
            }
            // Roots that went away take their files with them.
            for store in [&self.store, &self.code_store] {
                for (path, _) in store.roots()? {
                    if !roots.iter().any(|r| r.path == path) {
                        store.remove_missing(&path, &HashSet::new())?;
                        store.remove_root(&path)?;
                    }
                }
            }
            state.roots = roots.clone();
        }
        for root in &roots {
            let store = if root.code {
                &self.code_store
            } else {
                &self.store
            };
            store.add_root(&root.path, root.code)?;
        }
        self.watch_roots();
        self.crawl_all()
    }

    fn set_paused(&mut self, paused: bool) -> Result<()> {
        self.shared.state.lock().unwrap().paused = paused;
        self.store
            .set_meta("paused", if paused { "1" } else { "0" })?;
        self.apply_gate();
        self.send_status()
    }

    /// Pauses or resumes the worker to match the state, starting one when it may run and there is work.
    fn apply_gate(&mut self) {
        let may_run = {
            let state = self.shared.state.lock().unwrap();
            !state.paused && state.power_ok()
        };
        if let Some(worker) = &mut self.worker {
            let _ = worker
                .stdin
                .write_all(if may_run { b"resume\n" } else { b"pause\n" });
        } else if may_run {
            let _ = self.maybe_start_worker();
        }
    }

    fn crawl_all(&mut self) -> Result<()> {
        let roots = self.shared.state.lock().unwrap().roots.clone();
        self.shared.state.lock().unwrap().crawling = true;
        self.send_status()?;
        let excludes = self.shared.state.lock().unwrap().config.excludes();
        let started = Instant::now();
        let mut total = 0;
        let mut pending = 0;
        for root in &roots {
            let (candidates, _) = crawl(&omafind_index::Root {
                path: root.path.clone(),
                code: root.code,
            });
            let store = if root.code {
                &self.code_store
            } else {
                &self.store
            };
            let mut keep = HashSet::with_capacity(candidates.len());
            for c in candidates
                .iter()
                .filter(|c| !omafind_core::config::is_excluded(&c.path, &excludes))
            {
                let (_, changed) = store.see_file(&c.path, c.mtime, c.size as i64, c.kind)?;
                pending += changed as usize;
                keep.insert(c.path.clone());
            }
            total += keep.len();
            store.remove_missing(&root.path, &keep)?;
        }
        crate::log!(
            "crawled {} roots: {total} files, {pending} to index, {:.1} s",
            roots.len(),
            started.elapsed().as_secs_f64()
        );
        self.shared.state.lock().unwrap().crawling = false;
        self.maybe_start_worker()?;
        self.send_status()
    }

    /// Files the watcher saw change.
    fn changed(&mut self, paths: Vec<PathBuf>) -> Result<()> {
        let (roots, excludes) = {
            let state = self.shared.state.lock().unwrap();
            (state.roots.clone(), state.config.excludes())
        };
        let mut touched = 0;
        for path in paths {
            if omafind_core::config::is_excluded(&path, &excludes) {
                continue;
            }
            let Some(root) = roots.iter().find(|r| path.starts_with(&r.path)) else {
                continue;
            };
            let store = if root.code {
                &self.code_store
            } else {
                &self.store
            };
            if path.is_dir() {
                // A new or renamed folder: its contents in one go.
                let (candidates, _) = crawl(&omafind_index::Root {
                    path: path.clone(),
                    code: root.code,
                });
                let mut keep = HashSet::new();
                for c in &candidates {
                    store.see_file(&c.path, c.mtime, c.size as i64, c.kind)?;
                    keep.insert(c.path.clone());
                }
                store.remove_missing(&path, &keep)?;
                touched += candidates.len();
                continue;
            }
            match std::fs::metadata(&path) {
                Ok(meta) if meta.is_file() => {
                    let inside_code = !root.code
                        && path.parent().is_some_and(|p| {
                            p.ancestors()
                                .take_while(|a| a.starts_with(&root.path))
                                .any(omafind_index::rules::is_code_project)
                        });
                    let kind = if root.code {
                        omafind_extract::code::is_code(&path)
                            .then_some(omafind_core::FileKind::Code)
                    } else {
                        omafind_extract::classify::classify(&path)
                            .filter(|k| omafind_extract::supported(*k))
                    };
                    let hidden = path.strip_prefix(&root.path).ok().is_some_and(|rel| {
                        rel.components().any(|c| {
                            c.as_os_str()
                                .to_str()
                                .is_some_and(|s| s.starts_with('.') && s.len() > 1)
                        })
                    });
                    let wanted = kind.is_some_and(|k| {
                        !omafind_index::rules::is_secret(&path)
                            && omafind_index::rules::size_ok(k, meta.len())
                            && (!inside_code || k == omafind_core::FileKind::Screenshot)
                            && !hidden
                    });
                    match (wanted, kind) {
                        (true, Some(kind)) => {
                            let mtime = meta
                                .modified()
                                .ok()
                                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                                .map(|d| d.as_secs() as i64)
                                .unwrap_or(0);
                            store.see_file(&path, mtime, meta.len() as i64, kind)?;
                            touched += 1;
                        }
                        _ => {
                            store.remove_file(&path)?;
                        }
                    }
                }
                _ => {
                    // Gone (or not a plain file any more): the path itself, and everything that was under it when it
                    // was a folder.
                    let removed = store.remove_file(&path)? as usize
                        + store.remove_missing(&path, &HashSet::new())?;
                    if removed > 0 {
                        crate::log!(
                            "{} gone: {removed} files dropped",
                            omafind_core::paths::tilde(&path)
                        );
                        touched += removed;
                    }
                }
            }
        }
        if touched > 0 {
            self.maybe_start_worker()?;
            self.send_status()?;
        }
        Ok(())
    }

    fn maybe_start_worker(&mut self) -> Result<()> {
        if let Some(worker) = &mut self.worker {
            // Lingering or busy: either way it will look at the queue again.
            let _ = worker.stdin.write_all(b"more\n");
            return Ok(());
        }
        {
            let state = self.shared.state.lock().unwrap();
            if state.paused || !state.power_ok() {
                return Ok(());
            }
        }
        if self.store.stats()?.pending == 0 && self.code_store.stats()?.pending == 0 {
            return Ok(());
        }
        let exe = std::env::current_exe().context("own path")?;
        let mut child = Command::new(exe)
            .arg("--worker")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .context("starting the worker")?;
        let stdin = child.stdin.take().expect("piped");
        let stdout = child.stdout.take().expect("piped");
        let tx = self.tx.clone();
        std::thread::Builder::new()
            .name("omafind-worker-io".into())
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    if let Ok(parsed) = serde_json::from_str::<Line>(&line)
                        && tx.send(Msg::Worker(parsed)).is_err()
                    {
                        return;
                    }
                }
                let _ = tx.send(Msg::WorkerExited);
            })?;
        self.worker = Some(Worker { child, stdin });
        self.shared.state.lock().unwrap().worker_running = true;
        crate::log!("worker started");
        Ok(())
    }

    fn worker_line(&mut self, line: Line) -> Result<()> {
        match line {
            Line::Ready { seconds, device } => {
                crate::log!("worker ready in {seconds:.1} s ({device})")
            }
            Line::Reading { path, .. } => {
                self.shared.state.lock().unwrap().current = Some(path);
                self.send_status_throttled()?;
            }
            Line::Indexed { path, kind, .. } => {
                broadcast(
                    &self.shared,
                    Event::Indexed {
                        path: PathBuf::from(path),
                        kind: omafind_core::FileKind::parse(&kind)
                            .unwrap_or(omafind_core::FileKind::Doc),
                    },
                );
                self.send_status_throttled()?;
            }
            Line::Failed { path, error } => {
                crate::log!("failed {path}: {error}");
                broadcast(
                    &self.shared,
                    Event::Failed {
                        path: PathBuf::from(path),
                        error,
                    },
                );
            }
            Line::Done {
                indexed,
                failed,
                seconds,
            } => {
                crate::log!("worker done: {indexed} indexed, {failed} failed, {seconds:.1} s");
                self.shared.state.lock().unwrap().current = None;
            }
        }
        Ok(())
    }

    fn send_status(&mut self) -> Result<()> {
        self.last_status = Instant::now();
        let status = status(&self.shared, &self.store, &self.code_store, &self.searcher)?;
        broadcast(&self.shared, Event::Status(status));
        Ok(())
    }

    /// While indexing runs fast (small images), one status every 250 ms is plenty for a footer.
    fn send_status_throttled(&mut self) -> Result<()> {
        if self.last_status.elapsed().as_millis() >= 250 {
            self.send_status()?;
        }
        Ok(())
    }
}

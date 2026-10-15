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
use dig_client::{Event, Root, Status};
use dig_core::config::Config;
use dig_index::crawler::crawl;
use dig_store::Store;

use crate::power::PowerState;
use crate::searcher::Searcher;
use crate::worker::Line;

pub enum Msg {
    Changed(Vec<PathBuf>),
    Crawl,
    Pause,
    Resume,
    SetRoots { folders: Vec<PathBuf>, code: Vec<PathBuf> },
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
    pub power: PowerState,
    pub worker_running: bool,
    pub crawling: bool,
    pub current: Option<String>,
    pub roots: Vec<Root>,
    pub config: Config,
}

impl State {
    fn power_ok(&self) -> bool {
        !(self.config.power.pause_in_power_saver && self.power.power_saver) && !(self.config.power.wait_for_ac && self.power.on_battery)
    }

    pub fn state_name(&self) -> &'static str {
        if self.paused {
            "paused"
        } else if self.config.power.wait_for_ac && self.power.on_battery {
            "waiting_for_ac"
        } else if self.config.power.pause_in_power_saver && self.power.power_saver {
            "paused_power_saver"
        } else if self.crawling {
            "crawling"
        } else if self.worker_running {
            "indexing"
        } else {
            "idle"
        }
    }
}

/// The status, from the shared state and a store connection.
pub fn status(shared: &Shared, store: &Store, searcher: &Searcher) -> Result<Status> {
    let state = shared.state.lock().unwrap().clone();
    let stats = store.stats()?;
    Ok(Status {
        version: env!("CARGO_PKG_VERSION").into(),
        roots: state.roots.clone(),
        files: stats.files,
        indexed: stats.indexed,
        pending: stats.pending,
        failed: stats.failed,
        chunks: stats.chunks,
        state: state.state_name().into(),
        current: state.current.clone(),
        paused: state.paused,
        on_battery: state.power.on_battery,
        power_saver: state.power.power_saver,
        query_encoder_loaded: searcher.is_loaded(),
        worker_running: state.worker_running,
        llama_version: store.get_meta("llama_version")?.unwrap_or_default(),
        vector_version: store.vector_version()?,
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
    worker: Option<Worker>,
    tx: Sender<Msg>,
    watcher: Option<notify::RecommendedWatcher>,
    last_status: Instant,
}

pub fn main() -> Result<()> {
    let config = Config::load()?;
    let index_path = std::env::var_os("DIG_INDEX").map(PathBuf::from).unwrap_or_else(dig_core::paths::index_path);
    let store = Store::open(&index_path)?;
    if store.vector_version()? != dig_core::model::VECTOR_VERSION {
        crate::log!("vectors {} → {}: embedding everything again", store.vector_version()?, dig_core::model::VECTOR_VERSION);
        store.reset_all()?;
        store.set_meta("vector_version", &dig_core::model::VECTOR_VERSION.to_string())?;
    }
    let shared = Arc::new(Shared::default());
    let cpu = config.search.device == "cpu";
    let searcher = Searcher::new(index_path.clone(), cpu, config.search.unload_after_minutes)?;
    let (tx, rx) = channel::<Msg>();

    // Roots: the config's, else what the store remembers from `dig index`.
    let mut roots: Vec<Root> = config.folders().into_iter().map(|path| Root { path, code: false }).collect();
    roots.extend(config.code_roots().into_iter().map(|path| Root { path, code: true }));
    if roots.is_empty() {
        roots = store.roots()?.into_iter().map(|(path, code)| Root { path, code }).collect();
    }
    {
        let mut state = shared.state.lock().unwrap();
        state.paused = store.get_meta("paused")?.as_deref() == Some("1");
        state.roots = roots.clone();
        state.config = config.clone();
    }

    // Threads: the socket server, the power watcher, signals.
    crate::server::start(shared.clone(), searcher.clone(), tx.clone(), index_path.clone())?;
    {
        let tx = tx.clone();
        std::thread::Builder::new().name("dig-power".into()).spawn(move || crate::power::watch(tx))?;
    }
    {
        let tx = tx.clone();
        let mut signals = signal_hook::iterator::Signals::new([signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT, signal_hook::consts::SIGHUP])?;
        std::thread::Builder::new().name("dig-signals".into()).spawn(move || {
            for _ in signals.forever() {
                let _ = tx.send(Msg::Shutdown);
            }
        })?;
    }

    let mut coordinator = Coordinator { shared, searcher, store, worker: None, tx: tx.clone(), watcher: None, last_status: Instant::now() };
    coordinator.watch_roots();
    crate::log!("digd {} up: {} roots, index {}", env!("CARGO_PKG_VERSION"), roots.len(), dig_core::paths::tilde(&index_path));
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
                        crate::log!("power: battery {} power-saver {}", power.on_battery, power.power_saver);
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
                    let _ = std::fs::remove_file(dig_client::socket_path());
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    fn watch_roots(&mut self) {
        let roots: Vec<PathBuf> = self.shared.state.lock().unwrap().roots.iter().map(|r| r.path.clone()).collect();
        self.watcher = None;
        match crate::watcher::start(roots, self.tx.clone()) {
            Ok(watcher) => self.watcher = Some(watcher),
            Err(error) => crate::log!("no file watcher: {error}"),
        }
    }

    fn set_roots(&mut self, folders: Vec<PathBuf>, code: Vec<PathBuf>) -> Result<()> {
        let mut roots: Vec<Root> = folders.into_iter().map(|path| Root { path, code: false }).collect();
        roots.extend(code.into_iter().map(|path| Root { path, code: true }));
        {
            let mut state = self.shared.state.lock().unwrap();
            state.config.set_folders(&roots.iter().filter(|r| !r.code).map(|r| r.path.clone()).collect::<Vec<_>>(), &roots.iter().filter(|r| r.code).map(|r| r.path.clone()).collect::<Vec<_>>());
            if let Err(error) = state.config.save() {
                crate::log!("can't save config: {error:#}");
            }
            // Roots that went away take their files with them.
            for (path, _) in self.store.roots()? {
                if !roots.iter().any(|r| r.path == path) {
                    self.store.remove_missing(&path, &HashSet::new())?;
                    self.store.remove_root(&path)?;
                }
            }
            state.roots = roots.clone();
        }
        for root in &roots {
            self.store.add_root(&root.path, root.code)?;
        }
        self.watch_roots();
        self.crawl_all()
    }

    fn set_paused(&mut self, paused: bool) -> Result<()> {
        self.shared.state.lock().unwrap().paused = paused;
        self.store.set_meta("paused", if paused { "1" } else { "0" })?;
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
            let _ = worker.stdin.write_all(if may_run { b"resume\n" } else { b"pause\n" });
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
            let (candidates, _) = crawl(&dig_index::Root { path: root.path.clone(), code: root.code });
            let mut keep = HashSet::with_capacity(candidates.len());
            for c in candidates.iter().filter(|c| !dig_core::config::is_excluded(&c.path, &excludes)) {
                let (_, changed) = self.store.see_file(&c.path, c.mtime, c.size as i64, c.kind)?;
                pending += changed as usize;
                keep.insert(c.path.clone());
            }
            total += keep.len();
            self.store.remove_missing(&root.path, &keep)?;
        }
        crate::log!("crawled {} roots: {total} files, {pending} to index, {:.1} s", roots.len(), started.elapsed().as_secs_f64());
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
            if dig_core::config::is_excluded(&path, &excludes) {
                continue;
            }
            let Some(root) = roots.iter().find(|r| path.starts_with(&r.path)) else { continue };
            if path.is_dir() {
                // A new or renamed folder: its contents in one go.
                let (candidates, _) = crawl(&dig_index::Root { path: path.clone(), code: root.code });
                let mut keep = HashSet::new();
                for c in &candidates {
                    self.store.see_file(&c.path, c.mtime, c.size as i64, c.kind)?;
                    keep.insert(c.path.clone());
                }
                self.store.remove_missing(&path, &keep)?;
                touched += candidates.len();
                continue;
            }
            match std::fs::metadata(&path) {
                Ok(meta) if meta.is_file() => {
                    let inside_code = !root.code && path.parent().is_some_and(|p| p.ancestors().take_while(|a| a.starts_with(&root.path)).any(dig_index::rules::is_code_project));
                    let kind = dig_extract::classify::classify(&path).filter(|k| dig_extract::supported(*k));
                    let hidden = path.strip_prefix(&root.path).ok().is_some_and(|rel| rel.components().any(|c| c.as_os_str().to_str().is_some_and(|s| s.starts_with('.') && s.len() > 1)));
                    let wanted = kind.is_some_and(|k| {
                        !dig_index::rules::is_secret(&path)
                            && dig_index::rules::size_ok(k, meta.len())
                            && (!inside_code || k == dig_core::FileKind::Screenshot)
                            && !hidden
                    });
                    match (wanted, kind) {
                        (true, Some(kind)) => {
                            let mtime = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
                            self.store.see_file(&path, mtime, meta.len() as i64, kind)?;
                            touched += 1;
                        }
                        _ => {
                            self.store.remove_file(&path)?;
                        }
                    }
                }
                _ => {
                    // Gone (or not a plain file any more).
                    if self.store.remove_file(&path)? {
                        touched += 1;
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
        if self.store.stats()?.pending == 0 {
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
        std::thread::Builder::new().name("dig-worker-io".into()).spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Ok(parsed) = serde_json::from_str::<Line>(&line) {
                    if tx.send(Msg::Worker(parsed)).is_err() {
                        return;
                    }
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
            Line::Ready { seconds, device } => crate::log!("worker ready in {seconds:.1} s ({device})"),
            Line::Reading { path, .. } => {
                self.shared.state.lock().unwrap().current = Some(path);
                self.send_status_throttled()?;
            }
            Line::Indexed { path, kind, .. } => {
                broadcast(&self.shared, Event::Indexed { path: PathBuf::from(path), kind: dig_core::FileKind::parse(&kind).unwrap_or(dig_core::FileKind::Doc) });
                self.send_status_throttled()?;
            }
            Line::Failed { path, error } => {
                crate::log!("failed {path}: {error}");
                broadcast(&self.shared, Event::Failed { path: PathBuf::from(path), error });
            }
            Line::Done { indexed, failed, seconds } => {
                crate::log!("worker done: {indexed} indexed, {failed} failed, {seconds:.1} s");
                self.shared.state.lock().unwrap().current = None;
            }
        }
        Ok(())
    }

    fn send_status(&mut self) -> Result<()> {
        self.last_status = Instant::now();
        let status = status(&self.shared, &self.store, &self.searcher)?;
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

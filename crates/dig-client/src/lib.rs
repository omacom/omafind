//! Talking to `digd` over `$XDG_RUNTIME_DIR/dig.sock`: one JSON object per line each way. A request gets one
//! response, except `subscribe`, which keeps the connection and pushes an event per line until the client hangs
//! up. The window never polls: it subscribes.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use dig_core::{FileKind, Hit};
use serde::{Deserialize, Serialize};

pub fn socket_path() -> PathBuf {
    dig_core::paths::runtime_dir().join("dig.sock")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    Search { query: String, #[serde(default)] limit: Option<usize>, #[serde(default)] kinds: Vec<FileKind> },
    Status,
    /// Stop indexing until `resume` (the daemon remembers across restarts).
    Pause,
    Resume,
    /// Crawl the roots again now (new or changed files are picked up by the watcher anyway).
    Reindex,
    /// The window is about to be shown: load the query encoder if it isn't.
    Warm,
    /// The window was hidden: the query encoder may be unloaded after a while.
    Idle,
    /// Stream `Event`s until the connection closes.
    Subscribe,
    /// Change the indexed folders (persisted to config).
    SetRoots { folders: Vec<PathBuf>, #[serde(default)] code: Vec<PathBuf> },
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum Response {
    Hits { hits: Vec<Hit>, took_ms: f64 },
    Status(Status),
    Ok,
    Error { message: String },
}

/// Where indexing stands. Everything the footer of the window shows.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Status {
    pub version: String,
    pub roots: Vec<Root>,
    pub files: i64,
    pub indexed: i64,
    pub pending: i64,
    pub failed: i64,
    pub chunks: i64,
    /// `idle`, `indexing`, `paused`, `paused_power_saver`, `waiting_for_ac`, `crawling`.
    pub state: String,
    /// The file being read right now.
    pub current: Option<String>,
    pub paused: bool,
    pub on_battery: bool,
    pub power_saver: bool,
    pub query_encoder_loaded: bool,
    pub worker_running: bool,
    pub llama_version: String,
    pub vector_version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Root {
    pub path: PathBuf,
    pub code: bool,
}

/// Pushed to subscribers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// Counts changed (a file indexed, a crawl finished, a pause toggled). Carries the whole status so the client
    /// never has to ask.
    Status(Status),
    Indexed { path: PathBuf, kind: FileKind },
    Failed { path: PathBuf, error: String },
}

/// A connection to the daemon.
pub struct Client {
    stream: UnixStream,
    reader: BufReader<UnixStream>,
}

impl Client {
    /// Connects, or fails with a message that says the daemon isn't running.
    pub fn connect() -> Result<Self> {
        let path = socket_path();
        let stream = UnixStream::connect(&path).with_context(|| format!("digd isn't running ({} not open)", path.display()))?;
        stream.set_read_timeout(Some(Duration::from_secs(120)))?;
        let reader = BufReader::new(stream.try_clone()?);
        Ok(Self { stream, reader })
    }

    /// Whether the daemon answers at all.
    pub fn is_running() -> bool {
        UnixStream::connect(socket_path()).is_ok()
    }

    pub fn request(&mut self, request: &Request) -> Result<Response> {
        let mut line = serde_json::to_string(request)?;
        line.push('\n');
        self.stream.write_all(line.as_bytes())?;
        let mut answer = String::new();
        if self.reader.read_line(&mut answer)? == 0 {
            bail!("digd closed the connection");
        }
        Ok(serde_json::from_str(&answer)?)
    }

    pub fn search(&mut self, query: &str, limit: usize, kinds: Vec<FileKind>) -> Result<(Vec<Hit>, f64)> {
        match self.request(&Request::Search { query: query.into(), limit: Some(limit), kinds })? {
            Response::Hits { hits, took_ms } => Ok((hits, took_ms)),
            Response::Error { message } => bail!("{message}"),
            other => bail!("unexpected answer {other:?}"),
        }
    }

    pub fn status(&mut self) -> Result<Status> {
        match self.request(&Request::Status)? {
            Response::Status(status) => Ok(status),
            Response::Error { message } => bail!("{message}"),
            other => bail!("unexpected answer {other:?}"),
        }
    }

    pub fn simple(&mut self, request: &Request) -> Result<()> {
        match self.request(request)? {
            Response::Ok | Response::Status(_) => Ok(()),
            Response::Error { message } => bail!("{message}"),
            other => bail!("unexpected answer {other:?}"),
        }
    }

    /// Subscribes and hands every event to `f` until the daemon goes away or `f` returns false.
    pub fn subscribe(mut self, mut f: impl FnMut(Event) -> bool) -> Result<()> {
        let mut line = serde_json::to_string(&Request::Subscribe)?;
        line.push('\n');
        self.stream.write_all(line.as_bytes())?;
        self.stream.set_read_timeout(None)?;
        let mut buf = String::new();
        loop {
            buf.clear();
            if self.reader.read_line(&mut buf)? == 0 {
                return Ok(());
            }
            let event: Event = match serde_json::from_str(&buf) {
                Ok(event) => event,
                Err(_) => continue,
            };
            if !f(event) {
                return Ok(());
            }
        }
    }
}

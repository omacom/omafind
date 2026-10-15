//! `omafind-app`: the Omafind search window. A GPUI Shell host that runs the QuickJS UI in `ui/` and lends it one host
//! module, `omafind`. Nothing else reaches the scripts: no filesystem, no processes.
//!
//! The process stays resident. Hyprland keeps the window in the `special:omafind` workspace (window rules from
//! `omafind setup`), and `omafind toggle` shows or hides that workspace, so showing is instant. The app follows Hyprland's
//! event socket to know when it is shown (warm the query encoder, focus the field) and hidden (let it idle).
//!
//!   omafind-app [--dev] [--ui DIR]      --dev: hot-reload ui/ on save and write ui/gpui-kit.d.ts

mod actions;
mod hypr;
mod theme;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui_kit::{
    App, AssetSource, Bounds, SharedString, TitlebarOptions, WindowBounds, WindowOptions, px, size,
};
use gpui_shell::{
    AppAssets, Capabilities, HostError, HostModule, HostObject, HostValue, ShellRuntime,
};
use omafind_client::{Client, Request, Status};
use omafind_core::{FileKind, Locator};
use omafind_extract::thumbs;

const OMAFIND_TYPES: &str = r#"
/** One search hit: a file, and where in it the match is. */
export interface Result {
  id: number;
  path: string;
  /** The path with the home folder shortened, for display. */
  shortPath: string;
  name: string;
  kind: "image" | "screenshot" | "pdf" | "doc" | "audio" | "video" | "code";
  chunkKind: string;
  /** Page, seconds, line or character offset; null for a whole-file match. */
  locator: number | null;
  /** The locator for people: "p. 12", "03:41", "L88", or "". */
  /** Where the chunk ends: a code stretch's last line, a passage's end offset. */
  locatorEnd: number | null;
  locatorLabel: string;
  snippet: string;
  score: number;
  similarity: number | null;
  matchedWords: string[];
  /** What matched: "picture", "sound", "text", "name" or "words". */
  reason: string;
  /** Modification time, unix seconds; size in bytes. */
  mtime: number;
  size: number;
  /** The containing folder, home shortened. */
  folder: string;
  /** Near-duplicates folded into this result (copies, crops, edits, bursts), by path. */
  duplicates: string[];
}
export interface Chip { key: string; label: string }
export interface Answer {
  query: string;
  results: Result[];
  tookMs: number;
  /** Files per kind in the unfiltered list, for the filter chips. */
  counts: Record<string, number>;
  /** The filters read out of the query. */
  chips: Chip[];
  /** Names and keywords only, before the model answered. */
  quick: boolean;
  /** A newer search of the same session came first: ignore this one. */
  superseded: boolean;
}
export interface Recents { searches: { query: string; at: number }[]; opened: Result[]; added: Result[] }
export interface Passage { text: string; before: string; after: string }
export interface Line { n: number; text: string }
export interface Root { path: string; code: boolean }
export interface Status {
  /** Whether omafindd answers. */
  daemon: boolean;
  /** Whether the window is currently shown (Hyprland's special workspace is open). */
  visible: boolean;
  /** Increments every time the window is shown; the script resets its search on a change. */
  shown: number;
  /** Whether a config with folders exists (a daemon restart must not look like a first run). */
  configured: boolean;
  state: string;
  files: number; indexed: number; pending: number; failed: number;
  current: string | null;
  paused: boolean; onBattery: boolean; powerSaver: boolean; queryEncoderLoaded: boolean;
  roots: Root[];
  /** Per kind: indexed and total. */
  kinds: { kind: string; indexed: number; total: number }[];
  /** Seconds left, once there is a rate. */
  eta: number | null;
}
export function search(query: string, options?: { limit?: number; kinds?: string[]; quick?: boolean; session?: string }): Promise<Answer>;
/** Files like this result ("more like this"). */
export function similar(id: number, options?: { code?: boolean; limit?: number }): Promise<Answer>;
export function recents(): Promise<Recents>;
/** The matched passage in full, with its neighbours. */
export function passage(id: number, code?: boolean): Promise<Passage>;
/** Lines `from`..=`to` of a text file (code previews). */
export function lines(path: string, from: number, to: number): Promise<Line[]>;
/** A plain text file's lines around characters `start`..`end` (a passage), and the passage's first and last line;
 *  null for documents whose text isn't the file's own (office, HTML). */
export function section(path: string, start: number, end: number): Promise<{ lines: Line[]; first: number; last: number } | null>;
/** Theme colors for code beyond the gpui-base roles, or null before a theme loads. */
export function syntax(): { number: string; string: string } | null;
/** The picture for a result: "small" (rows, grid) or "large" (preview); null when the kind has none. */
export function thumb(path: string, kind: string, locator: number | null, size?: "small" | "large"): Promise<string | null>;
/** Moves the file to the trash. */
export function trash(path: string): boolean;
/** Puts text on the clipboard. */
export function copyText(text: string): boolean;
export function status(): Status;
/** `locator`: a PDF's page, a moment in seconds, a line of code or of a plain text document. */
export function open(path: string, kind: string, locator: number | null): boolean;
export function reveal(path: string): boolean;
export function copy(path: string): boolean;
export function hide(): void;

/** A folder the setup screen offers. */
export interface Folder { path: string; label: string; summary: string; selected: boolean; files: number }
export interface Candidates {
  folders: Folder[];
  /** Folders that look like code, for the optional code index. */
  code: Folder[];
  textModel: boolean; mediaModel: boolean;
  daemonEnabled: boolean;
  binding: boolean; rules: boolean; autostart: boolean;
}
/** Surveys the candidate folders (a few seconds at most). */
export function candidates(): Promise<Candidates>;
/** Saves the folders, starts the daemon at login if needed, adds the Hyprland lines; answers with a summary. */
export function applySetup(options: { folders: string[]; code: string[]; hyprland: boolean; autostart: boolean }): Promise<string>;
/** Downloads the model files that are missing; progress through `setupProgress()`. */
export function downloadModels(): Promise<boolean>;
export function setupProgress(): { message: string; fraction: number } | null;
export function pause(): void;
export function resume(): void;
"#;

/// The scripts' assets: their own directory (icons), plus the thumbnail cache by absolute path and nothing else.
struct OmafindAssets {
    app: AppAssets,
    thumbs: PathBuf,
}

impl AssetSource for OmafindAssets {
    fn load(&self, path: &str) -> anyhow::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        let candidate = Path::new(path);
        if candidate.is_absolute() {
            let Ok(full) = candidate.canonicalize() else {
                return Ok(None);
            };
            if !full.starts_with(&self.thumbs) {
                return Ok(None);
            }
            return Ok(Some(std::fs::read(full)?.into()));
        }
        self.app.load(path)
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        self.app.list(path)
    }
}

#[derive(Default)]
struct State {
    status: Option<Status>,
    daemon: bool,
    visible: bool,
    shown: u64,
    query_words: Vec<String>,
    /// What setup is doing right now (a download), for the screen.
    setup_progress: Option<(String, f64)>,
}

/// Where threads send notes for the main thread (set once the app runs).
static NOTES: std::sync::OnceLock<async_channel::Sender<Note>> = std::sync::OnceLock::new();

fn note(note: Note) {
    if let Some(tx) = NOTES.get() {
        let _ = tx.send_blocking(note);
    }
}

type Shared = Arc<Mutex<State>>;

enum Note {
    Status(Box<Status>),
    Daemon(bool),
    Shown,
    Hidden,
    Theme,
    /// Host state the script reads changed: re-describe.
    Refresh,
}

fn with_app<T>(f: impl FnOnce(&mut App) -> T) -> Result<T, HostError> {
    gpui_shell::with_current_app(f)
        .ok_or_else(|| HostError::new("omafind: no live App for this call"))
}

fn kind_of(name: &str) -> Result<FileKind, HostError> {
    FileKind::parse(name).ok_or_else(|| HostError::new(format!("unknown kind {name}")))
}

fn hit_value(hit: &omafind_core::Hit) -> HostValue {
    HostObject::new()
        .field("id", hit.chunk_id as f64)
        .field("path", hit.path.as_str())
        .field(
            "shortPath",
            omafind_core::paths::tilde(Path::new(&hit.path)),
        )
        .field("name", hit.name.as_str())
        .field("kind", hit.kind.as_str())
        .field("chunkKind", hit.chunk_kind.as_str())
        .field(
            "locator",
            hit.locator
                .map(|l| HostValue::Number(l.value()))
                .unwrap_or(HostValue::Null),
        )
        .field(
            "locatorEnd",
            hit.locator_end
                .map(HostValue::Number)
                .unwrap_or(HostValue::Null),
        )
        .field("locatorLabel", hit.locator_label.as_str())
        .field("snippet", hit.snippet.as_str())
        .field("score", hit.score)
        .field(
            "similarity",
            hit.similarity
                .map(|s| HostValue::Number(s as f64))
                .unwrap_or(HostValue::Null),
        )
        .field(
            "matchedWords",
            HostValue::Array(
                hit.matched_words
                    .iter()
                    .map(|w| HostValue::from(w.as_str()))
                    .collect(),
            ),
        )
        .field("reason", hit.reason.as_str())
        .field("mtime", hit.mtime as f64)
        .field("size", hit.size as f64)
        .field(
            "folder",
            Path::new(&hit.path)
                .parent()
                .map(omafind_core::paths::tilde)
                .unwrap_or_default(),
        )
        .field(
            "duplicates",
            HostValue::Array(
                hit.duplicates
                    .iter()
                    .map(|p| HostValue::from(p.as_str()))
                    .collect(),
            ),
        )
        .into()
}

fn reply_value(query: String, reply: &omafind_client::Reply, started: Instant) -> HostValue {
    let mut counts = HostObject::new();
    for (kind, n) in &reply.counts {
        counts = counts.field(kind.as_str(), *n as f64);
    }
    HostObject::new()
        .field("query", query)
        .field(
            "results",
            HostValue::Array(reply.hits.iter().map(hit_value).collect()),
        )
        .field("tookMs", started.elapsed().as_secs_f64() * 1000.0)
        .field("counts", counts)
        .field(
            "chips",
            HostValue::Array(
                reply
                    .chips
                    .iter()
                    .map(|c| {
                        HostObject::new()
                            .field("key", c.key.as_str())
                            .field("label", c.label.as_str())
                            .into()
                    })
                    .collect(),
            ),
        )
        .field("quick", reply.quick)
        .field("superseded", reply.superseded)
        .into()
}

fn number(value: Option<&HostValue>) -> Option<f64> {
    match value {
        Some(HostValue::Number(n)) => Some(*n),
        _ => None,
    }
}

fn status_value(state: &State) -> HostValue {
    let s = state.status.clone().unwrap_or_default();
    HostObject::new()
        .field("daemon", state.daemon)
        .field("visible", state.visible)
        .field("shown", state.shown as f64)
        .field(
            "configured",
            omafind_core::config::Config::load()
                .map(|c| !c.folders().is_empty() || !c.code_roots().is_empty())
                .unwrap_or(false),
        )
        .field(
            "state",
            if state.daemon {
                s.state.as_str()
            } else {
                "no_daemon"
            },
        )
        .field("files", s.files as f64)
        .field("indexed", s.indexed as f64)
        .field("pending", s.pending as f64)
        .field("failed", s.failed as f64)
        .field(
            "current",
            s.current
                .as_deref()
                .map(|c| HostValue::from(omafind_core::paths::tilde(Path::new(c))))
                .unwrap_or(HostValue::Null),
        )
        .field("paused", s.paused)
        .field("onBattery", s.on_battery)
        .field("powerSaver", s.power_saver)
        .field("queryEncoderLoaded", s.query_encoder_loaded)
        .field(
            "roots",
            HostValue::Array(
                s.roots
                    .iter()
                    .map(|r| {
                        HostObject::new()
                            .field("path", omafind_core::paths::tilde(&r.path))
                            .field("code", r.code)
                            .into()
                    })
                    .collect(),
            ),
        )
        .field(
            "kinds",
            HostValue::Array(
                s.kinds
                    .iter()
                    .map(|(k, done, total)| {
                        HostObject::new()
                            .field("kind", k.as_str())
                            .field("indexed", *done as f64)
                            .field("total", *total as f64)
                            .into()
                    })
                    .collect(),
            ),
        )
        .field(
            "eta",
            s.eta_seconds
                .map(HostValue::Number)
                .unwrap_or(HostValue::Null),
        )
        .into()
}

fn folder_value(survey: &omafind_index::survey::Survey, selected: bool) -> HostValue {
    HostObject::new()
        .field("path", survey.path.to_string_lossy().into_owned())
        .field("label", omafind_core::paths::tilde(&survey.path))
        .field("summary", survey.summary())
        .field("selected", selected)
        .field("files", survey.files as f64)
        .into()
}

fn strings(value: Option<&HostValue>) -> Vec<String> {
    match value {
        Some(HostValue::Array(items)) => items
            .iter()
            .filter_map(|i| match i {
                HostValue::Str(s) => Some(s.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn omafind_module(shared: Shared) -> HostModule {
    let for_status = shared.clone();
    let for_search = shared.clone();
    let for_open = shared.clone();
    let for_progress = shared.clone();
    let for_download = shared.clone();
    HostModule::new("omafind")
        .function("setupProgress", move |_| {
            Ok(match &for_progress.lock().unwrap().setup_progress {
                Some((message, fraction)) => HostObject::new()
                    .field("message", message.as_str())
                    .field("fraction", *fraction)
                    .into(),
                None => HostValue::Null,
            })
        })
        .function("pause", |_| {
            tell_daemon(Request::Pause);
            Ok(HostValue::Null)
        })
        .function("resume", |_| {
            tell_daemon(Request::Resume);
            Ok(HostValue::Null)
        })
        .async_function("candidates", |_| {
            let executor = with_app(|cx| cx.background_executor().clone())?;
            Ok(async move {
                let value = executor
                    .spawn(async move {
                        let config = omafind_core::config::Config::load().unwrap_or_default();
                        let folders = omafind_setup::candidate_folders(&config);
                        let code = omafind_setup::candidate_code_folders(&config);
                        let models = omafind_core::model::dir();
                        let hypr = omafind_setup::hypr::installed();
                        HostValue::from(
                            HostObject::new()
                                .field(
                                    "folders",
                                    HostValue::Array(
                                        folders
                                            .iter()
                                            .map(|(s, on)| folder_value(s, *on))
                                            .collect(),
                                    ),
                                )
                                .field(
                                    "code",
                                    HostValue::Array(
                                        code.iter().map(|(s, on)| folder_value(s, *on)).collect(),
                                    ),
                                )
                                .field(
                                    "textModel",
                                    omafind_setup::download::present(
                                        &models,
                                        &omafind_setup::download::TEXT,
                                    ),
                                )
                                .field(
                                    "mediaModel",
                                    omafind_setup::download::present(
                                        &models,
                                        &omafind_setup::download::MEDIA,
                                    ),
                                )
                                .field("daemonEnabled", omafind_setup::daemon_enabled())
                                .field("binding", hypr.binding)
                                .field("rules", hypr.rules)
                                .field("autostart", hypr.autostart),
                        )
                    })
                    .await;
                Ok(value)
            })
        })
        .async_function("downloadModels", move |_| {
            let executor = with_app(|cx| cx.background_executor().clone())?;
            let shared = for_download.clone();
            Ok(async move {
                let result = executor
                    .spawn(async move {
                        let models = omafind_core::model::dir();
                        for file in [
                            &omafind_setup::download::TEXT,
                            &omafind_setup::download::MEDIA,
                        ] {
                            if omafind_setup::download::present(&models, file) {
                                continue;
                            }
                            let name = if file.name == omafind_core::model::TEXT_FILE {
                                "text model"
                            } else {
                                "image and audio encoders"
                            };
                            let shared2 = shared.clone();
                            let outcome =
                                omafind_setup::download::fetch_with(&models, file, &mut |step| {
                                    use omafind_setup::download::Progress;
                                    shared2.lock().unwrap().setup_progress = Some(match step {
                                        Progress::Copying => (
                                            format!(
                                                "Copying the {name} from the Hugging Face cache"
                                            ),
                                            0.0,
                                        ),
                                        Progress::Downloading { done, total } => (
                                            format!(
                                                "Downloading the {name} ({} of {} MB)",
                                                done / 1_000_000,
                                                total / 1_000_000
                                            ),
                                            done as f64 / total.max(1) as f64,
                                        ),
                                        Progress::Checking => (format!("Checking the {name}"), 1.0),
                                    });
                                    note(Note::Refresh);
                                });
                            shared.lock().unwrap().setup_progress = None;
                            note(Note::Refresh);
                            outcome?;
                        }
                        Ok::<(), anyhow::Error>(())
                    })
                    .await;
                match result {
                    Ok(()) => Ok(HostValue::from(true)),
                    Err(error) => Err(HostError::new(format!("{error:#}"))),
                }
            })
        })
        .async_function("applySetup", |arguments| {
            let options = arguments.get(0);
            let folders: Vec<PathBuf> = strings(options.and_then(|o| o.get("folders")))
                .into_iter()
                .map(PathBuf::from)
                .collect();
            let code: Vec<PathBuf> = strings(options.and_then(|o| o.get("code")))
                .into_iter()
                .map(PathBuf::from)
                .collect();
            let hyprland = options
                .and_then(|o| o.get("hyprland"))
                .map(|v| matches!(v, HostValue::Bool(true)))
                .unwrap_or(true);
            let autostart = options
                .and_then(|o| o.get("autostart"))
                .map(|v| matches!(v, HostValue::Bool(true)))
                .unwrap_or(true);
            let executor = with_app(|cx| cx.background_executor().clone())?;
            Ok(async move {
                let result = executor
                    .spawn(async move {
                        let mut done = Vec::new();
                        omafind_setup::set_folders(&folders, &code)?;
                        done.push(format!(
                            "indexing {} folder{}",
                            folders.len() + code.len(),
                            if folders.len() + code.len() == 1 {
                                ""
                            } else {
                                "s"
                            }
                        ));
                        if !Client::is_running() || !omafind_setup::daemon_enabled() {
                            let omafindd = std::env::current_exe()?.with_file_name("omafindd");
                            omafind_setup::start_daemon(&omafindd)?;
                            done.push("daemon started".into());
                        }
                        if omafind_setup::install_hyprland(hyprland, hyprland, autostart)? {
                            done.push("Hyprland set up".into());
                        }
                        Ok::<String, anyhow::Error>(done.join(", "))
                    })
                    .await;
                match result {
                    Ok(message) => Ok(HostValue::from(message)),
                    Err(error) => Err(HostError::new(format!("{error:#}"))),
                }
            })
        })
        .declarations(OMAFIND_TYPES)
        .function("syntax", |_| {
            Ok(match theme::SYNTAX.lock().unwrap().clone() {
                Some((number, string)) => HostObject::new()
                    .field("number", number)
                    .field("string", string)
                    .into(),
                None => HostValue::Null,
            })
        })
        .async_function("section", |arguments| {
            let path = PathBuf::from(arguments.string(0)?);
            let start = number(arguments.get(1)).unwrap_or(0.0).max(0.0) as usize;
            let end = number(arguments.get(2))
                .unwrap_or(start as f64)
                .max(start as f64) as usize;
            let executor = with_app(|cx| cx.background_executor().clone())?;
            Ok(async move {
                let found = executor
                    .spawn(async move { actions::text_section(&path, start, end, 4, 80) })
                    .await;
                Ok(match found {
                    Some((lines, first, last)) => HostObject::new()
                        .field(
                            "lines",
                            HostValue::Array(
                                lines
                                    .into_iter()
                                    .map(|(n, text)| {
                                        HostObject::new()
                                            .field("n", n as f64)
                                            .field("text", text)
                                            .into()
                                    })
                                    .collect(),
                            ),
                        )
                        .field("first", first as f64)
                        .field("last", last as f64)
                        .into(),
                    None => HostValue::Null,
                })
            })
        })
        .function("status", move |_| {
            Ok(status_value(&for_status.lock().unwrap()))
        })
        .async_function("search", move |arguments| {
            let query = arguments.string(0)?.to_owned();
            let options = arguments.get(1);
            let limit = number(options.and_then(|o| o.get("limit")))
                .map(|n| n as usize)
                .unwrap_or(40);
            let quick = matches!(
                options.and_then(|o| o.get("quick")),
                Some(HostValue::Bool(true))
            );
            let session = match options.and_then(|o| o.get("session")) {
                Some(HostValue::Str(s)) => Some(s.to_string()),
                _ => None,
            };
            let kinds: Vec<FileKind> = options
                .and_then(|o| o.get("kinds"))
                .and_then(|v| match v {
                    HostValue::Array(items) => Some(
                        items
                            .iter()
                            .filter_map(|i| match i {
                                HostValue::Str(s) => FileKind::parse(s),
                                _ => None,
                            })
                            .collect(),
                    ),
                    _ => None,
                })
                .unwrap_or_default();
            for_search.lock().unwrap().query_words =
                omafind_core::ranking::query_words(&omafind_core::query::parse(&query).text);
            let executor = with_app(|cx| cx.background_executor().clone())?;
            Ok(async move {
                let started = Instant::now();
                let request = Request::Search {
                    query: query.clone(),
                    limit: Some(limit),
                    kinds,
                    quick,
                    session,
                };
                let reply = executor
                    .spawn(
                        async move { Client::connect().and_then(|mut c| c.search_with(&request)) },
                    )
                    .await
                    .map_err(|e| HostError::new(format!("{e:#}")))?;
                if !quick && !reply.superseded {
                    tracing::info!(
                        "search {query:?}: {} hits in {:.0} ms",
                        reply.hits.len(),
                        started.elapsed().as_secs_f64() * 1000.0
                    );
                }
                Ok(reply_value(query, &reply, started))
            })
        })
        .async_function("similar", |arguments| {
            let id = arguments
                .get(0)
                .and_then(|v| match v {
                    HostValue::Number(n) => Some(*n as i64),
                    _ => None,
                })
                .ok_or_else(|| HostError::new("similar(id)"))?;
            let options = arguments.get(1);
            let code = matches!(
                options.and_then(|o| o.get("code")),
                Some(HostValue::Bool(true))
            );
            let limit = number(options.and_then(|o| o.get("limit")))
                .map(|n| n as usize)
                .unwrap_or(40);
            let executor = with_app(|cx| cx.background_executor().clone())?;
            Ok(async move {
                let started = Instant::now();
                let request = Request::Similar {
                    chunk_id: id,
                    limit: Some(limit),
                    code,
                };
                let reply = executor
                    .spawn(
                        async move { Client::connect().and_then(|mut c| c.search_with(&request)) },
                    )
                    .await
                    .map_err(|e| HostError::new(format!("{e:#}")))?;
                Ok(reply_value(String::new(), &reply, started))
            })
        })
        .async_function("recents", |_| {
            let executor = with_app(|cx| cx.background_executor().clone())?;
            Ok(async move {
                let r = executor
                    .spawn(async move { Client::connect().and_then(|mut c| c.recents()) })
                    .await
                    .map_err(|e| HostError::new(format!("{e:#}")))?;
                Ok(HostObject::new()
                    .field(
                        "searches",
                        HostValue::Array(
                            r.searches
                                .iter()
                                .map(|(q, at)| {
                                    HostObject::new()
                                        .field("query", q.as_str())
                                        .field("at", *at as f64)
                                        .into()
                                })
                                .collect(),
                        ),
                    )
                    .field(
                        "opened",
                        HostValue::Array(r.opened.iter().map(hit_value).collect()),
                    )
                    .field(
                        "added",
                        HostValue::Array(r.added.iter().map(hit_value).collect()),
                    )
                    .into())
            })
        })
        .async_function("passage", |arguments| {
            let id = arguments
                .get(0)
                .and_then(|v| match v {
                    HostValue::Number(n) => Some(*n as i64),
                    _ => None,
                })
                .ok_or_else(|| HostError::new("passage(id)"))?;
            let code = matches!(arguments.get(1), Some(HostValue::Bool(true)));
            let executor = with_app(|cx| cx.background_executor().clone())?;
            Ok(async move {
                let p = executor
                    .spawn(async move { Client::connect().and_then(|mut c| c.context(id, code)) })
                    .await
                    .map_err(|e| HostError::new(format!("{e:#}")))?;
                Ok(HostObject::new()
                    .field("text", p.text)
                    .field("before", p.before)
                    .field("after", p.after)
                    .into())
            })
        })
        .async_function("lines", |arguments| {
            let path = PathBuf::from(arguments.string(0)?);
            let from = number(arguments.get(1)).unwrap_or(1.0).max(1.0) as usize;
            let to = number(arguments.get(2))
                .unwrap_or(from as f64 + 40.0)
                .max(from as f64) as usize;
            let executor = with_app(|cx| cx.background_executor().clone())?;
            Ok(async move {
                let lines = executor
                    .spawn(async move { actions::lines(&path, from, to) })
                    .await;
                Ok(HostValue::Array(
                    lines
                        .into_iter()
                        .map(|(n, text)| {
                            HostObject::new()
                                .field("n", n as f64)
                                .field("text", text)
                                .into()
                        })
                        .collect(),
                ))
            })
        })
        .async_function("thumb", |arguments| {
            let path = PathBuf::from(arguments.string(0)?);
            let kind = kind_of(arguments.string(1)?)?;
            let locator = number(arguments.get(2));
            let size = match arguments.get(3) {
                Some(HostValue::Str(s)) if &**s == "large" => thumbs::Size::Large,
                _ => thumbs::Size::Small,
            };
            let executor = with_app(|cx| cx.background_executor().clone())?;
            Ok(async move {
                let thumb = executor
                    .spawn(async move { thumbs::get(&path, kind, locator, size) })
                    .await;
                Ok(thumb
                    .map(|p| HostValue::from(p.to_string_lossy().into_owned()))
                    .unwrap_or(HostValue::Null))
            })
        })
        .function("trash", |arguments| {
            let path = PathBuf::from(arguments.string(0)?);
            actions::trash(&path).map_err(|e| HostError::new(format!("{e:#}")))?;
            Ok(HostValue::from(true))
        })
        .function("copyText", |arguments| {
            let text = arguments.string(0)?.to_owned();
            actions::copy_text(&text).map_err(|e| HostError::new(format!("{e:#}")))?;
            Ok(HostValue::from(true))
        })
        .function("open", move |arguments| {
            let path = PathBuf::from(arguments.string(0)?);
            let kind = kind_of(arguments.string(1)?)?;
            let locator = arguments.get(2).and_then(|v| match v {
                HostValue::Number(n) => Some(*n),
                _ => None,
            });
            let locator = match (kind, locator) {
                (FileKind::Pdf, Some(p)) => Some(Locator::Page(p as u32)),
                (FileKind::Audio | FileKind::Video, Some(s)) => Some(Locator::Seconds(s)),
                // A plain text file's line: the script passes one only when it found where the words are.
                (FileKind::Code | FileKind::Doc, Some(l)) => Some(Locator::Line(l as u32)),
                _ => None,
            };
            let words = for_open.lock().unwrap().query_words.clone();
            actions::open(&path, kind, locator, &words)
                .map_err(|e| HostError::new(format!("{e:#}")))?;
            tell_daemon(Request::Opened {
                path: path.clone(),
                query: None,
            });
            hide_window();
            Ok(HostValue::from(true))
        })
        .function("reveal", |arguments| {
            let path = PathBuf::from(arguments.string(0)?);
            actions::reveal(&path).map_err(|e| HostError::new(format!("{e:#}")))?;
            hide_window();
            Ok(HostValue::from(true))
        })
        .function("copy", |arguments| {
            let path = PathBuf::from(arguments.string(0)?);
            actions::copy(&path).map_err(|e| HostError::new(format!("{e:#}")))?;
            Ok(HostValue::from(true))
        })
        .function("hide", |_| {
            hide_window();
            Ok(HostValue::Null)
        })
}

/// Closes the special workspace if it is open. Off a thread: Hyprland answers in a few ms, but never block a frame.
fn hide_window() {
    std::thread::spawn(|| {
        if hypr::is_shown().unwrap_or(false)
            && let Err(error) = hypr::toggle()
        {
            tracing::warn!("hide: {error:#}");
        }
    });
}

/// Where the UI scripts are: `--ui DIR`, else `~/.local/share/omafind/ui` (install.sh), else `/usr/share/omafind/ui` (the
/// package), else the checkout's `ui/` (development).
fn ui_root(explicit: Option<PathBuf>) -> PathBuf {
    if let Some(dir) = explicit {
        return dir;
    }
    for candidate in [
        omafind_core::paths::data_dir().join("ui"),
        PathBuf::from("/usr/share/omafind/ui"),
    ] {
        if candidate.join("main.js").exists() {
            return candidate;
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../ui")
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from("ui"))
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
                        return tx2.send_blocking(Note::Status(Box::new(status))).is_ok();
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

fn tell_daemon(request: Request) {
    std::thread::spawn(move || {
        if let Ok(mut client) = Client::connect() {
            let _ = client.simple(&request);
        }
    });
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut dev = cfg!(debug_assertions);
    let mut ui = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dev" => dev = true,
            "--release-ui" => dev = false,
            "--ui" => ui = args.next().map(PathBuf::from),
            "--version" => {
                println!("omafind-app {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            other => {
                eprintln!("omafind-app: unknown argument {other}");
                std::process::exit(2);
            }
        }
    }
    let ui = ui_root(ui);
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new(
                    "omafind_app=info,gpui_shell=info,gpui_kit::shell::script=info",
                )
            }),
        )
        .with_writer(std::io::stderr)
        .init();

    let components = match gpui_shell::components::components() {
        Ok(components) => components,
        Err(error) => {
            eprintln!("omafind-app: {error:#}");
            std::process::exit(1);
        }
    };
    if dev {
        match gpui_shell::write_type_declarations_with_components(&ui, &components) {
            Ok(_) => tracing::info!("wrote {}", ui.join("gpui-kit.d.ts").display()),
            Err(error) => tracing::warn!("couldn't write gpui-kit.d.ts: {error}"),
        }
    }

    let shared: Shared = Arc::new(Mutex::new(State::default()));
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
    let thumbs_dir = omafind_core::paths::cache_dir().join("thumbs");
    let _ = std::fs::create_dir_all(&thumbs_dir);
    let thumbs_dir = thumbs_dir.canonicalize().unwrap_or(thumbs_dir);

    gpui_kit::application()
        .with_assets(OmafindAssets {
            app: AppAssets::new(ui.clone()),
            thumbs: thumbs_dir.clone(),
        })
        .run(move |cx| {
            gpui_shell::init_with_components(cx, &components);
            if dev {
                gpui_shell::set_development_mode(true);
            }
            match theme::Palette::load() {
                Ok(palette) => palette.apply(&font, cx),
                Err(error) => {
                    tracing::warn!("no Omarchy theme ({error:#}): using the default palette")
                }
            }
            // The scripts may read their own directory (icons) and the thumbnail cache, and nothing else.
            gpui_shell::set_capabilities(
                Capabilities::new().read_roots([ui.clone(), thumbs_dir.clone()]),
            );
            gpui_shell::export_module(omafind_module(shared.clone()))
                .expect("the `omafind` module matches its declarations");
            gpui_shell::on_exit_request(|_, _, cx| cx.quit());

            let runtime = match ShellRuntime::new_with_components(cx, components) {
                Ok(runtime) => runtime,
                Err(error) => {
                    eprintln!("omafind-app: failed to start the script runtime: {error}");
                    std::process::exit(1);
                }
            };

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
            let app_root = ui.clone();
            let runtime_for_window = runtime.clone();
            let root_cell: Arc<Mutex<Option<gpui_kit::Entity<gpui_shell::ShellRoot>>>> =
                Arc::new(Mutex::new(None));
            let root_sink = root_cell.clone();
            let window = cx
                .open_window(options, move |window, cx| {
                    let root = runtime_for_window.load(&app_root, window, cx);
                    if dev {
                        match runtime_for_window.watch(&root, window, cx) {
                            Ok(watch) => watch.forget(),
                            Err(error) => {
                                tracing::warn!("not watching {}: {error}", app_root.display())
                            }
                        }
                    }
                    *root_sink.lock().unwrap() = Some(root.clone());
                    root
                })
                .expect("failed to open the Omafind window");

            // Notes from the threads land on the main thread here: the script is told to re-describe itself.
            let shared_notes = shared.clone();
            let font = font.clone();
            cx.spawn(async move |cx| {
                while let Ok(note) = rx.recv().await {
                    cx.update(|cx| {
                        let mut refresh = true;
                        match note {
                            Note::Status(status) => {
                                shared_notes.lock().unwrap().status = Some(*status)
                            }
                            Note::Daemon(up) => shared_notes.lock().unwrap().daemon = up,
                            Note::Shown => {
                                {
                                    let mut s = shared_notes.lock().unwrap();
                                    s.visible = true;
                                    s.shown += 1;
                                }
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
                                shared_notes.lock().unwrap().visible = false;
                                tell_daemon(Request::Idle);
                                refresh = false;
                            }
                            Note::Refresh => {}
                            Note::Theme => match theme::Palette::load() {
                                Ok(palette) => {
                                    tracing::info!("theme changed: retinting");
                                    palette.apply(&font, cx);
                                }
                                Err(error) => tracing::warn!("theme: {error:#}"),
                            },
                        }
                        if refresh
                            && let Some(root) = root_cell.lock().unwrap().clone()
                            && let Err(error) = runtime.refresh(&root, cx)
                        {
                            tracing::warn!("refresh: {error}");
                        }
                    });
                }
            })
            .detach();

            cx.spawn(async move |cx| {
                cx.update(|cx| {
                    cx.activate(true);
                    let _ = window.update(cx, |_, window, _| window.activate_window());
                });
            })
            .detach();
        });
}

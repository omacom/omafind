//! What the window asks of the rest of Omafind: searches and passages from omafindd, folder surveys, setup, the model
//! download. Every function here blocks (a socket round trip, a directory walk, a download), so views run them on the
//! background executor and apply the result on the main thread.

use std::path::{Path, PathBuf};

use anyhow::Result;
use omafind_client::{Client, Passage, Recents, Reply, Request};
use omafind_core::FileKind;

/// One search request from the window.
pub struct SearchRequest {
    pub query: String,
    pub limit: usize,
    pub kinds: Vec<FileKind>,
    /// Names and keywords only, before the model answers.
    pub quick: bool,
}

/// Searches through omafindd. The window is one session: a newer search supersedes an older one still running.
pub fn search(request: SearchRequest) -> Result<Reply> {
    Client::connect()?.search_with(&Request::Search {
        query: request.query,
        limit: Some(request.limit),
        kinds: request.kinds,
        quick: request.quick,
        session: Some("window".into()),
    })
}

/// Files like this result ("more like this").
pub fn similar(chunk_id: i64, code: bool) -> Result<Reply> {
    Client::connect()?.search_with(&Request::Similar {
        chunk_id,
        limit: Some(40),
        code,
    })
}

pub fn recents() -> Result<Recents> {
    Client::connect()?.recents()
}

/// The matched passage in full, with its neighbours.
pub fn passage(chunk_id: i64, code: bool) -> Result<Passage> {
    Client::connect()?.context(chunk_id, code)
}

/// Tells omafindd something that needs no answer, off the calling thread.
pub fn tell_daemon(request: Request) {
    std::thread::spawn(move || {
        if let Ok(mut client) = Client::connect() {
            let _ = client.simple(&request);
        }
    });
}

/// A folder setup offers. `chosen`: in the config now (indexed); `selected`: what the screen shows ticked (the
/// defaults on a first run).
#[derive(Debug, Clone)]
pub struct Folder {
    pub path: PathBuf,
    pub label: String,
    /// What it holds and how long it would take: `4 PDFs, 6 documents · ~20 s`.
    pub summary: String,
    pub selected: bool,
    pub chosen: bool,
}

impl Folder {
    fn new(survey: &omafind_index::survey::Survey, selected: bool, chosen: bool) -> Self {
        Self {
            path: survey.path.clone(),
            label: omafind_core::paths::tilde(&survey.path),
            summary: survey.summary(),
            selected,
            chosen,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Power {
    pub pause_in_power_saver: bool,
    pub wait_for_ac: bool,
}

impl Default for Power {
    fn default() -> Self {
        let config = omafind_core::config::PowerConfig::default();
        Self {
            pause_in_power_saver: config.pause_in_power_saver,
            wait_for_ac: config.wait_for_ac,
        }
    }
}

/// What the setup screen starts from.
#[derive(Debug, Clone, Default)]
pub struct Candidates {
    pub folders: Vec<Folder>,
    /// Folders that look like code, for the optional code index.
    pub code: Vec<Folder>,
    pub power: Power,
    /// Folders are in the config: settings, not a first run.
    pub configured: bool,
    /// The index files' size on disk.
    pub index_bytes: u64,
    pub text_model: bool,
    pub media_model: bool,
    /// The SUPER + CTRL + G binding is already in ~/.config/hypr/omafind.lua.
    pub binding: bool,
}

/// Surveys the candidate folders (a few seconds at most).
pub fn candidates() -> Candidates {
    let config = omafind_core::config::Config::load().unwrap_or_default();
    let (chosen, chosen_code) = (config.folders(), config.code_roots());
    let models = omafind_core::model::dir();
    Candidates {
        folders: omafind_setup::candidate_folders(&config)
            .iter()
            .map(|(s, on)| Folder::new(s, *on, chosen.contains(&s.path)))
            .collect(),
        code: omafind_setup::candidate_code_folders(&config)
            .iter()
            .map(|(s, on)| Folder::new(s, *on, chosen_code.contains(&s.path)))
            .collect(),
        power: Power {
            pause_in_power_saver: config.power.pause_in_power_saver,
            wait_for_ac: config.power.wait_for_ac,
        },
        configured: !chosen.is_empty() || !chosen_code.is_empty(),
        index_bytes: index_bytes(),
        text_model: omafind_setup::download::present(&models, &omafind_setup::download::TEXT),
        media_model: omafind_setup::download::present(&models, &omafind_setup::download::MEDIA),
        binding: omafind_setup::hypr::installed().binding,
    }
}

/// Whether a config with folders exists: a daemon restart must not look like a first run.
pub fn configured() -> bool {
    omafind_core::config::Config::load()
        .map(|c| !c.folders().is_empty() || !c.code_roots().is_empty())
        .unwrap_or(false)
}

/// The index files on disk, SQLite's sidecars included.
fn index_bytes() -> u64 {
    [
        omafind_core::paths::index_path(),
        omafind_core::paths::code_index_path(),
    ]
    .iter()
    .flat_map(|p| {
        ["", "-wal", "-shm"].map(|suffix| {
            let mut name = p.clone().into_os_string();
            name.push(suffix);
            std::fs::metadata(name).map(|m| m.len()).unwrap_or(0)
        })
    })
    .sum()
}

/// What Save writes.
pub struct Plan {
    pub folders: Vec<PathBuf>,
    pub code: Vec<PathBuf>,
    pub power: Power,
    pub shortcut: bool,
}

/// Saves the folders and power settings, starts the daemon at login if needed, writes ~/.config/hypr/omafind.lua (window
/// rules and autostart always, the keybinding when asked); answers with a summary.
pub fn apply_setup(plan: Plan) -> Result<String> {
    let mut done = Vec::new();
    omafind_setup::set_folders(&plan.folders, &plan.code)?;
    omafind_setup::set_power(omafind_core::config::PowerConfig {
        pause_in_power_saver: plan.power.pause_in_power_saver,
        wait_for_ac: plan.power.wait_for_ac,
    })?;
    let n = plan.folders.len() + plan.code.len();
    done.push(format!(
        "indexing {n} folder{}",
        if n == 1 { "" } else { "s" }
    ));
    if !Client::is_running() || !omafind_setup::daemon_enabled() {
        let omafindd = std::env::current_exe()?.with_file_name("omafindd");
        omafind_setup::start_daemon(&omafindd)?;
        done.push("daemon started".into());
    }
    if omafind_setup::install_hyprland(plan.shortcut)? {
        done.push("Hyprland set up".into());
    }
    if let Err(error) = crate::hypr::adopt() {
        tracing::warn!("adopt: {error:#}");
    }
    Ok(done.join(", "))
}

/// Where a download is: what it says, and how far it is (0..1) through the file it is on.
#[derive(Debug, Clone, PartialEq)]
pub struct Progress {
    pub message: String,
    pub fraction: f64,
}

/// Downloads the model files that are missing, reporting each step.
pub fn download_models(mut report: impl FnMut(Option<Progress>)) -> Result<()> {
    use omafind_setup::download::{self, MEDIA, TEXT};
    let models = omafind_core::model::dir();
    for file in [&TEXT, &MEDIA] {
        if download::present(&models, file) {
            continue;
        }
        let name = if file.name == omafind_core::model::TEXT_FILE {
            "text model"
        } else {
            "image and audio encoders"
        };
        let outcome = download::fetch_with(&models, file, &mut |step| {
            report(Some(match step {
                download::Progress::Copying => Progress {
                    message: format!("Copying the {name} from the Hugging Face cache"),
                    fraction: 0.0,
                },
                download::Progress::Downloading { done, total } => Progress {
                    message: format!(
                        "Downloading the {name} ({} of {} MB)",
                        done / 1_000_000,
                        total / 1_000_000
                    ),
                    fraction: done as f64 / total.max(1) as f64,
                },
                download::Progress::Checking => Progress {
                    message: format!("Checking the {name}"),
                    fraction: 1.0,
                },
            }));
        });
        report(None);
        outcome?;
    }
    Ok(())
}

/// The desktop's folder chooser; `None` when cancelled.
pub fn pick_folder() -> Result<Option<PathBuf>> {
    crate::portal::pick_folder("Add a folder to index")
}

/// What a folder holds and how long it would take, as `candidates` reports it.
pub fn survey_folder(path: &Path, code: bool) -> Option<Folder> {
    omafind_index::survey::survey_all(&[path.to_path_buf()], code)
        .first()
        .map(|s| Folder::new(s, true, false))
}

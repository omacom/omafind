//! `omafind`: index folders and search them. Everything the window does is reachable here.

mod bench;
mod doctor;
mod eval;
mod setup;
mod ui;

use std::io::IsTerminal;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use omafind_client::{Client, Request};
use omafind_core::FileKind;
use omafind_embed::{Embedder, Models, Options};
use omafind_index::{Event, Indexer, Root};
use omafind_store::Store;

#[derive(Parser)]
#[command(
    name = "omafind",
    version,
    about = "Local semantic search over your files"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
    /// Run the model on the CPU even when a GPU is available.
    #[arg(long, global = true)]
    cpu: bool,
    /// The index file (default: ~/.local/share/omafind/index.sqlite).
    #[arg(long, global = true, env = "OMAFIND_INDEX")]
    index: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Command {
    /// Index folders (remembered for next time); with none, re-index the remembered ones.
    Index {
        folders: Vec<PathBuf>,
        /// Also index this folder as code (searched with `code:`).
        #[arg(long = "code")]
        code: Vec<PathBuf>,
        /// Forget every vector and embed everything again.
        #[arg(long)]
        reset: bool,
    },
    /// Search the index.
    Search {
        query: Vec<String>,
        #[arg(long)]
        json: bool,
        /// Only these kinds: screenshot, image, pdf, doc, audio, video.
        #[arg(long, value_delimiter = ',')]
        kind: Vec<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// What the index holds and what the daemon is doing.
    Status {
        #[arg(long)]
        json: bool,
        /// List the files that couldn't be read, and why.
        #[arg(long)]
        failed: bool,
    },
    /// Stop indexing until `omafind resume` (remembered across restarts).
    Pause,
    Resume,
    /// Follow the daemon's progress until indexing is done.
    Watch,
    /// Show or hide the search window (starts omafind-app if it isn't running).
    Toggle,
    /// Download the model, pick folders, start the daemon, add the key and window rules.
    Setup {
        /// Accept every step without asking.
        #[arg(long, short = 'y')]
        yes: bool,
        /// Say what would be done, do nothing.
        #[arg(long)]
        dry_run: bool,
        /// Folders to index (instead of the picker).
        #[arg(long = "folder")]
        folders: Vec<PathBuf>,
        /// Code roots to index (instead of the picker).
        #[arg(long = "code")]
        code: Vec<PathBuf>,
        /// Only the text model (310 MB): no photos, audio or video.
        #[arg(long)]
        text_only: bool,
    },
    /// Check the model, engine, tools, daemon, window and Hyprland config.
    Doctor {
        #[arg(long)]
        json: bool,
    },
    /// Time searches on synthetic indexes of these sizes (built once, cached), and the query encoder.
    Bench {
        #[arg(default_values_t = [10_000usize, 100_000, 300_000])]
        chunks: Vec<usize>,
        /// Skip timing the query encoder.
        #[arg(long)]
        no_model: bool,
    },
    /// Measure recall over a query set with known answers (default: evals/queries.json).
    Eval {
        #[arg(default_value = "evals/queries.json")]
        queries: PathBuf,
    },
    /// Stop the daemon, remove the Hyprland lines and (asked) the data.
    Uninstall {
        #[arg(long, short = 'y')]
        yes: bool,
        #[arg(long)]
        keep_data: bool,
    },
}

fn main() -> Result<()> {
    ui::init();
    let cli = Cli::parse();
    let index_path = cli
        .index
        .clone()
        .unwrap_or_else(omafind_core::paths::index_path);
    match cli.command {
        Command::Index {
            folders,
            code,
            reset,
        } => index(&index_path, folders, code, reset, cli.cpu),
        Command::Search {
            query,
            json,
            kind,
            limit,
        } => search(&index_path, &query.join(" "), json, &kind, limit, cli.cpu),
        Command::Status { failed: true, .. } => failed_files(&index_path),
        Command::Status { json, .. } => status(&index_path, json),
        Command::Pause => {
            Client::connect()?.simple(&Request::Pause)?;
            done("Indexing paused; `omafind resume` picks it up where it stopped")
        }
        Command::Resume => {
            Client::connect()?.simple(&Request::Resume)?;
            done("Indexing resumed")
        }
        Command::Watch => watch(true),
        Command::Toggle => toggle(),
        Command::Setup {
            yes,
            dry_run,
            folders,
            code,
            text_only,
        } => setup::run(setup::Options {
            yes,
            dry_run,
            folders,
            code,
            text_only,
        }),
        Command::Doctor { json } => doctor::run(json).map(|ok| {
            if !ok {
                std::process::exit(1)
            }
        }),
        Command::Eval { queries } => {
            let cpu = cli.cpu;
            let mut local: Option<(Store, Store, Embedder)> = None;
            let mut search = |query: &str| -> Result<Vec<omafind_core::Hit>> {
                if Client::is_running() {
                    return Client::connect()?
                        .search(query, 10, Vec::new())
                        .map(|(h, _)| h);
                }
                if local.is_none() {
                    local = Some((
                        Store::open(&index_path)?,
                        Store::open(&index_path.with_file_name("code.sqlite"))?,
                        load_embedder(true, cpu)?,
                    ));
                }
                let (store, code_store, embedder) = local.as_mut().unwrap();
                let (text, options, _) = omafind_index::plan(query, &[], 10);
                omafind_index::search(
                    if options.code { code_store } else { store },
                    embedder,
                    &text,
                    &options,
                )
            };
            eval::run(&queries, &mut search).map(|ok| {
                if !ok {
                    std::process::exit(1)
                }
            })
        }
        Command::Uninstall { yes, keep_data } => setup::uninstall(yes, keep_data),
        Command::Bench { chunks, no_model } => bench::run(&chunks, !no_model),
    }
}

/// Hyprland keeps the window in the `special:omafind` workspace; toggling that is instant. Without a window yet, the
/// app is started and the toggle waits for it to map (a few hundred milliseconds), the one time it is slow.
fn toggle() -> Result<()> {
    use std::process::{Command as Proc, Stdio};
    let hypr = |args: &[&str]| {
        Proc::new("hyprctl")
            .args(args)
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
    };
    let has_window = || {
        hypr(&["-j", "clients"])
            .map(|j| j.contains("\"class\": \"omafind\""))
            .unwrap_or(false)
    };
    if !has_window() {
        let exe = std::env::current_exe()?.with_file_name("omafind-app");
        let program = if exe.exists() {
            exe
        } else {
            PathBuf::from("omafind-app")
        };
        Proc::new("setsid")
            .arg("-f")
            .arg(program)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("starting omafind-app")?;
        let started = Instant::now();
        while !has_window() && started.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(50));
        }
        // A fresh window whose rule put it in the special workspace is hidden: fall through to show it. Without
        // the rules (not set up yet) the window is already on screen, and the toggle below would open an empty
        // special workspace, so check first.
        let in_special = hypr(&["-j", "clients"])
            .map(|j| j.contains("\"name\": \"special:omafind\""))
            .unwrap_or(false);
        if !in_special {
            return Ok(());
        }
    }
    let out = hypr(&["dispatch", "hl.dsp.workspace.toggle_special(\"omafind\")"])?;
    anyhow::ensure!(out.trim() == "ok", "hyprctl: {}", out.trim());
    // Shown: make sure the keyboard is on it (a toggle from another window doesn't always move focus).
    let shown = hypr(&["-j", "monitors"])
        .map(|j| j.contains("\"name\": \"special:omafind\""))
        .unwrap_or(false);
    if shown {
        let _ = hypr(&["dispatch", "hl.dsp.focus({ window = \"class:omafind\" })"]);
    }
    Ok(())
}

/// Prints the daemon's progress until nothing is pending and no worker runs.
/// Follows the daemon until indexing is done. `intro`: open the frame (false when `index` already did).
fn watch(intro: bool) -> Result<()> {
    let client = Client::connect()?;
    if !std::io::stderr().is_terminal() {
        let mut busy_seen = false;
        return client.subscribe(|event| {
            match event {
                omafind_client::Event::Status(s) => {
                    let current = s
                        .current
                        .as_deref()
                        .map(|c| {
                            format!("  {}", omafind_core::paths::tilde(std::path::Path::new(c)))
                        })
                        .unwrap_or_default();
                    eprintln!(
                        "{:<18} {} indexed, {} pending, {} failed{current}",
                        s.state, s.indexed, s.pending, s.failed
                    );
                    if finished(&s, &mut busy_seen) {
                        return false;
                    }
                }
                omafind_client::Event::Indexed { path, kind } => {
                    eprintln!(
                        "  {:<10} {}",
                        kind.as_str(),
                        omafind_core::paths::tilde(&path)
                    )
                }
                omafind_client::Event::Failed { path, error } => {
                    eprintln!(
                        "  failed     {}: {error}",
                        omafind_core::paths::tilde(&path)
                    )
                }
            }
            true
        });
    }
    // On a terminal: one bar over every file, the one being read as its message.
    if intro {
        ui::intro(
            "watch",
            "Ctrl+C stops watching · `omafind pause` stops indexing",
        )?;
    }
    // Before files are read there is a phase to explain (starting, crawling, loading the model, paused): a spinner
    // says which. Once files are being read, a bar takes over.
    let waiting = cliclack::spinner();
    waiting.start("Asking omafindd what it is doing");
    let mut bar: Option<cliclack::ProgressBar> = None;
    let mut summary = String::new();
    let mut busy_seen = false;
    let mut failures: Vec<(PathBuf, String)> = Vec::new();
    client.subscribe(|event| {
        match event {
            omafind_client::Event::Status(s) => {
                let current = s
                    .current
                    .as_deref()
                    .map(|c| omafind_core::paths::tilde(std::path::Path::new(c)));
                let phase = phase(&s);
                if finished(&s, &mut busy_seen) {
                    let line = format!(
                        "{} files indexed, {} failed",
                        thousands(s.indexed),
                        thousands(s.failed)
                    );
                    match bar.take() {
                        Some(bar) => {
                            bar.set_position((s.indexed + s.failed) as u64);
                            bar.stop(line);
                        }
                        None => waiting.stop(format!("Nothing new to index · {line}")),
                    }
                    summary = if s.failed == 0 {
                        "Everything is indexed".to_owned()
                    } else {
                        format!("Indexed, with {} files that couldn't be read", s.failed)
                    };
                    return false;
                }
                match (&bar, &current) {
                    // Files are being read: from now on, the bar.
                    (None, Some(_)) if s.state == "indexing" => {
                        waiting.stop(format!("{} files to index", thousands(s.pending)));
                        let b = cliclack::progress_bar((s.files as u64).max(1));
                        b.start(current.clone().unwrap_or_default());
                        b.set_position((s.indexed + s.failed) as u64);
                        bar = Some(b);
                    }
                    (None, _) => waiting.set_message(phase),
                    (Some(b), _) => {
                        b.set_length((s.files as u64).max(1));
                        b.set_position((s.indexed + s.failed) as u64);
                        b.set_message(match (&current, s.state.as_str()) {
                            (Some(c), "indexing") => c.clone(),
                            _ => phase,
                        });
                    }
                }
            }
            omafind_client::Event::Indexed { .. } => {}
            omafind_client::Event::Failed { path, error } => failures.push((path, error)),
        }
        true
    })?;
    report_failures(&failures)?;
    if summary.is_empty() {
        cliclack::outro_cancel("omafindd went away")?;
    } else {
        cliclack::outro(summary)?;
    }
    Ok(())
}

/// What the daemon is doing before (or between) files, in words.
fn phase(s: &omafind_client::Status) -> String {
    match s.state.as_str() {
        "starting" => "omafindd is starting: setting up a watch on every folder".into(),
        "crawling" => "Looking for new and changed files".into(),
        "paused" => "Paused · `omafind resume` goes on".into(),
        "paused_power_saver" => "Paused while the power profile is power-saver".into(),
        "waiting_for_ac" => "Waiting for a charger".into(),
        "indexing" if s.current.is_none() => "Loading the model".into(),
        "indexing" => "Reading files".into(),
        _ if s.pending > 0 => format!(
            "{} files to index, waiting for the indexer to start",
            thousands(s.pending)
        ),
        _ => "Waiting for omafindd".into(),
    }
}

/// Indexing is over: nothing pending and no crawl under way, after the daemon has been seen busy (or has files to
/// show for it, or no folders to index). A daemon that has only just started is idle before its first crawl; the
/// worker staying loaded for a minute after the last file doesn't count as work.
fn finished(s: &omafind_client::Status, busy_seen: &mut bool) -> bool {
    if s.pending > 0 || s.state == "crawling" || s.state == "starting" {
        *busy_seen = true;
        return false;
    }
    if s.state == "indexing" {
        *busy_seen = true;
    }
    *busy_seen || s.indexed + s.failed > 0 || s.roots.is_empty()
}

/// Files that couldn't be read, after a progress bar: the first few, and where to find the rest.
fn report_failures(failures: &[(PathBuf, String)]) -> Result<()> {
    for (path, error) in failures.iter().take(8) {
        cliclack::log::warning(format!("{}\n{error}", omafind_core::paths::tilde(path)))?;
    }
    if failures.len() > 8 {
        cliclack::log::remark(format!(
            "{} more: omafind status --failed",
            failures.len() - 8
        ))?;
    }
    Ok(())
}

fn load_embedder(text_only: bool, cpu: bool) -> Result<Embedder> {
    let models = Models::locate(&omafind_core::model::dir())?;
    Embedder::new(
        &models,
        &Options {
            text_only,
            cpu_only: cpu,
            ..Options::default()
        },
    )
    .context("loading the model")
}

fn index(
    index_path: &std::path::Path,
    folders: Vec<PathBuf>,
    code: Vec<PathBuf>,
    reset: bool,
    cpu: bool,
) -> Result<()> {
    let mut roots: Vec<Root> = folders
        .iter()
        .map(|p| omafind_index::indexer::resolve_root(p).map(|path| Root { path, code: false }))
        .collect::<Result<_>>()?;
    roots.extend(
        code.iter()
            .map(|p| omafind_index::indexer::resolve_root(p).map(|path| Root { path, code: true }))
            .collect::<Result<Vec<_>>>()?,
    );
    // The folders are remembered in the config, which the daemon reads too (not for a test index elsewhere).
    if !roots.is_empty() && *index_path == omafind_core::paths::index_path() {
        let mut config = omafind_core::config::Config::load().unwrap_or_default();
        config.set_folders(
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
        config.save()?;
    }
    if Client::is_running() && !reset && *index_path == omafind_core::paths::index_path() {
        let mut client = Client::connect()?;
        if roots.is_empty() {
            client.simple(&Request::Reindex)?;
        } else {
            client.simple(&Request::SetRoots {
                folders: roots
                    .iter()
                    .filter(|r| !r.code)
                    .map(|r| r.path.clone())
                    .collect(),
                code: roots
                    .iter()
                    .filter(|r| r.code)
                    .map(|r| r.path.clone())
                    .collect(),
            })?;
        }
        if std::io::stderr().is_terminal() {
            ui::intro("index", folders_line(&roots))?;
            cliclack::log::info(
                "omafindd is indexing. Ctrl+C stops watching; `omafind pause` stops the indexing, `omafind resume` goes on",
            )?;
        } else {
            eprintln!(
                "omafindd is indexing; following its progress (Ctrl-C to stop watching, it keeps going)"
            );
        }
        return watch(false);
    }
    let mut store = Store::open(index_path)?;
    let mut code_store = Store::open(&index_path.with_file_name("code.sqlite"))?;
    if roots.is_empty() {
        roots = omafind_index::indexer::stored_roots(&store)?;
        roots.extend(omafind_index::indexer::stored_roots(&code_store)?);
    }
    anyhow::ensure!(
        !roots.is_empty(),
        "nothing to index: `omafind index ~/Pictures ~/Documents` picks the folders"
    );
    if std::io::stderr().is_terminal() {
        ui::intro("index", folders_line(&roots))?;
    }
    for s in [&store, &code_store] {
        if reset || s.vector_version()? != omafind_core::model::VECTOR_VERSION {
            cliclack::log::warning(format!(
                "Embedding everything again (vectors {})",
                omafind_core::model::VECTOR_VERSION
            ))?;
            s.reset_all()?;
            s.set_meta(
                "vector_version",
                &omafind_core::model::VECTOR_VERSION.to_string(),
            )?;
        }
    }
    let started = Instant::now();
    let mut embedder = ui::step(
        "Loading the model",
        0,
        || load_embedder(false, cpu),
        |loaded| match loaded {
            Ok(e) => format!(
                "Model loaded in {:.1} s on the {}",
                e.load_seconds,
                if omafind_embed::gpu_found() && !cpu {
                    "GPU"
                } else {
                    "CPU"
                }
            ),
            Err(_) => "The model didn't load".into(),
        },
    )?;
    store.set_meta("llama_version", omafind_embed::LLAMA_VERSION)?;
    // One bar per run over the files the crawl found pending; what the crawl found, and failures, around it.
    let bar = std::cell::RefCell::new(None::<cliclack::ProgressBar>);
    let pending = std::cell::Cell::new(0u64);
    let failures = std::cell::RefCell::new(Vec::<(PathBuf, String)>::new());
    let on_event = |event: Event| match event {
        Event::Crawled {
            root,
            files,
            pending: new,
            removed,
            skipped_secrets,
            skipped_code_dirs,
            huge_dirs,
        } => {
            pending.set(pending.get() + new as u64);
            let mut line = format!(
                "{}: {files} files, {new} to index",
                omafind_core::paths::tilde(&root)
            );
            if removed > 0 {
                line += &format!(", {removed} gone");
            }
            if skipped_secrets > 0 {
                line += &format!(", {skipped_secrets} secrets skipped");
            }
            if skipped_code_dirs > 0 {
                line += &format!(", {skipped_code_dirs} code projects (screenshots only)");
            }
            for dir in huge_dirs {
                line += &format!(
                    "\nskipped {} (more than {} entries)",
                    omafind_core::paths::tilde(&dir),
                    omafind_index::rules::MAX_DIR_ENTRIES
                );
            }
            let _ = cliclack::log::step(line);
        }
        Event::Reading { path, .. } => {
            let mut bar = bar.borrow_mut();
            let bar = bar.get_or_insert_with(|| {
                let b = cliclack::progress_bar(pending.get().max(1));
                b.start("Reading");
                b
            });
            bar.set_message(omafind_core::paths::tilde(&path));
        }
        Event::Indexed { .. } => {
            if let Some(bar) = bar.borrow().as_ref() {
                bar.inc(1);
            }
        }
        Event::Failed { path, error } => {
            if let Some(bar) = bar.borrow().as_ref() {
                bar.inc(1);
            }
            failures.borrow_mut().push((path, error));
        }
        Event::Done {
            indexed,
            failed,
            seconds,
        } => {
            let line = format!("{indexed} files indexed, {failed} failed, {seconds:.1} s");
            match bar.borrow_mut().take() {
                Some(bar) => bar.stop(line),
                None => {
                    let _ = cliclack::log::success(line);
                }
            }
            pending.set(0);
            let _ = report_failures(&failures.borrow_mut().drain(..).collect::<Vec<_>>());
        }
    };
    let (files, code): (Vec<Root>, Vec<Root>) = roots.into_iter().partition(|r| !r.code);
    if !files.is_empty() {
        Indexer {
            store: &mut store,
            embedder: &mut embedder,
            on_event: Box::new(on_event),
            gate: Box::new(|| true),
        }
        .run(&files)?;
    }
    if !code.is_empty() {
        Indexer {
            store: &mut code_store,
            embedder: &mut embedder,
            on_event: Box::new(on_event),
            gate: Box::new(|| true),
        }
        .run(&code)?;
    }
    cliclack::outro(format!("Done in {:.1} s", started.elapsed().as_secs_f64()))?;
    Ok(())
}

/// `~/Pictures, ~/Documents`, or what indexing with no folders means.
fn folders_line(roots: &[Root]) -> String {
    if roots.is_empty() {
        return "the remembered folders".into();
    }
    roots
        .iter()
        .map(|r| omafind_core::paths::tilde(&r.path))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A one-line result: framed on a terminal, a plain line otherwise.
fn done(message: &str) -> Result<()> {
    if ui::framed() {
        cliclack::log::success(message)?;
    } else {
        println!("{message}");
    }
    Ok(())
}

fn failed_files(index_path: &std::path::Path) -> Result<()> {
    let mut any = false;
    for path in [
        index_path.to_path_buf(),
        index_path.with_file_name("code.sqlite"),
    ] {
        if !path.exists() {
            continue;
        }
        for (file, error) in Store::open(&path)?.failed_files(1000)? {
            any = true;
            println!("{}\n    {error}", omafind_core::paths::tilde(&file));
        }
    }
    if !any {
        println!("every file was read");
    }
    Ok(())
}

fn search(
    index_path: &std::path::Path,
    query: &str,
    json: bool,
    kinds: &[String],
    limit: usize,
    cpu: bool,
) -> Result<()> {
    let kinds: Vec<FileKind> = kinds
        .iter()
        .map(|k| FileKind::parse(k).with_context(|| format!("unknown kind {k}")))
        .collect::<Result<_>>()?;
    let framed = ui::framed() && !json;
    if framed {
        ui::intro("search", query)?;
    }
    let work = || -> Result<(Vec<omafind_core::Hit>, Duration, Duration)> {
        Ok(if Client::is_running() {
            let t = Instant::now();
            let (hits, took_ms) = Client::connect()?.search(query, limit, kinds)?;
            (
                hits,
                Duration::ZERO,
                Duration::from_secs_f64((took_ms / 1000.0).max(0.0)).min(t.elapsed()),
            )
        } else {
            anyhow::ensure!(
                index_path.exists(),
                "no index yet: run `omafind index <folders>` first"
            );
            let t0 = Instant::now();
            let mut embedder = load_embedder(true, cpu)?;
            let loaded = t0.elapsed();
            let t1 = Instant::now();
            let (text, options, _) = omafind_index::plan(query, &kinds, limit);
            let store = Store::open(&if options.code {
                index_path.with_file_name("code.sqlite")
            } else {
                index_path.to_path_buf()
            })?;
            let hits = if text.is_empty() && options.filter.is_empty() {
                Vec::new()
            } else if text.is_empty() {
                omafind_index::rank(&store, None, &text, &options)?.hits
            } else {
                omafind_index::search(&store, &mut embedder, &text, &options)?
            };
            (hits, loaded, t1.elapsed())
        })
    };
    let (hits, loaded, searched) = if framed {
        ui::step("Searching", 0, work, |r| match r {
            Ok((hits, loaded, searched)) if loaded.is_zero() => {
                format!(
                    "{} found in {:.0} ms",
                    count(hits.len(), "result"),
                    searched.as_secs_f64() * 1000.0
                )
            }
            Ok((hits, loaded, searched)) => format!(
                "{} found in {:.0} ms (the model took {:.1} s to load: omafindd keeps it ready)",
                count(hits.len(), "result"),
                searched.as_secs_f64() * 1000.0,
                loaded.as_secs_f64()
            ),
            Err(_) => "Search failed".into(),
        })?
    } else {
        work()?
    };
    if framed {
        for (i, hit) in hits.iter().enumerate() {
            let mut meta = vec![hit.kind.as_str().to_owned()];
            if !hit.locator_label.is_empty() {
                meta.push(hit.locator_label.clone());
            }
            if let Some(sim) = hit.similarity {
                meta.push(format!("{sim:.2}"));
            }
            ui::line(format!(
                "{} {}  {}",
                ui::dim(format!("{:>2}", i + 1)),
                ui::bold(&hit.name),
                ui::dim(meta.join(" · "))
            ));
            ui::line(format!(
                "   {}",
                ui::dim(omafind_core::paths::tilde(std::path::Path::new(&hit.path)))
            ));
            if !hit.snippet.is_empty() {
                let room = console::Term::stderr().size().1 as usize;
                let snippet = fit(&hit.snippet, room.saturating_sub(8).max(40));
                ui::line(format!("   {}", highlight(&snippet, &hit.matched_words)));
            }
            ui::line("");
            ui::beat(15);
        }
        if hits.is_empty() {
            cliclack::outro_cancel(format!(
                "Nothing found for {query:?}: try other words, or fewer filters"
            ))?;
        } else {
            cliclack::outro("Open one with `omafind toggle`, or by its path")?;
        }
        return Ok(());
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&hits)?);
        return Ok(());
    }
    if hits.is_empty() {
        println!("nothing found for {query:?}");
    }
    for (i, hit) in hits.iter().enumerate() {
        let locator = if hit.locator_label.is_empty() {
            String::new()
        } else {
            format!("  {}", hit.locator_label)
        };
        let sim = hit
            .similarity
            .map(|s| format!("  {s:.2}"))
            .unwrap_or_default();
        println!(
            "{:>2}. {}  [{}{locator}]{sim}",
            i + 1,
            hit.name,
            hit.kind.as_str()
        );
        println!(
            "    {}",
            omafind_core::paths::tilde(std::path::Path::new(&hit.path))
        );
        if !hit.snippet.is_empty() {
            println!("    {}", hit.snippet);
        }
    }
    if loaded.is_zero() {
        eprintln!(
            "({} results via omafindd, {:.0} ms)",
            hits.len(),
            searched.as_secs_f64() * 1000.0
        );
    } else {
        eprintln!(
            "({} results; model {:.2} s, search {:.0} ms)",
            hits.len(),
            loaded.as_secs_f64(),
            searched.as_secs_f64() * 1000.0
        );
    }
    Ok(())
}

/// `text` cut to `width` characters, an ellipsis marking the cut.
fn fit(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_owned();
    }
    let cut: String = text.chars().take(width.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

/// `3 results`, `1 result`.
fn count(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}

/// `text` with the query's words in the accent colour, as the window marks them.
fn highlight(text: &str, words: &[String]) -> String {
    let lower = text.to_lowercase();
    // Offsets into the lowercase copy only line up with the text when lowercasing kept every byte length.
    if lower.len() != text.len() {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut at = 0;
    while at < text.len() {
        let next = words
            .iter()
            .filter(|w| w.len() >= 2)
            .filter_map(|w| lower[at..].find(w.as_str()).map(|i| (at + i, w.len())))
            .min_by_key(|&(i, len)| (i, std::cmp::Reverse(len)));
        match next {
            Some((i, len)) if text.is_char_boundary(i) && text.is_char_boundary(i + len) => {
                out.push_str(&text[at..i]);
                out.push_str(&console::style(&text[i..i + len]).blue().bold().to_string());
                at = i + len;
            }
            _ => {
                out.push_str(&text[at..]);
                break;
            }
        }
    }
    out
}

/// `19,431`
fn thousands(n: i64) -> String {
    let digits = n.abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

fn kind_label(kind: FileKind) -> &'static str {
    match kind {
        FileKind::Screenshot => "Screenshots",
        FileKind::Image => "Photos",
        FileKind::Pdf => "PDFs",
        FileKind::Doc => "Documents",
        FileKind::Audio => "Recordings",
        FileKind::Video => "Videos",
        FileKind::Code => "Code",
    }
}

/// `■■■■■■□□□□` over `width` cells.
fn meter(done: i64, total: i64, width: usize) -> String {
    let filled = if total > 0 {
        ((done as f64 / total as f64) * width as f64).round() as usize
    } else {
        0
    };
    let filled = filled.min(width);
    format!(
        "{}{}",
        console::style("■".repeat(filled)).blue(),
        console::style("□".repeat(width - filled)).dim()
    )
}

fn status_framed(s: &omafind_client::Status) -> Result<()> {
    ui::intro("status", format!("omafindd {}", s.version))?;
    let current = s
        .current
        .as_deref()
        .map(|c| omafind_core::paths::tilde(std::path::Path::new(c)));
    match s.state.as_str() {
        "indexing" => cliclack::log::info(format!(
            "Indexing · `omafind pause` stops it{}",
            current
                .map(|c| format!("\n{}", ui::dim(c)))
                .unwrap_or_default()
        ))?,
        "crawling" => cliclack::log::info("Looking for new and changed files")?,
        "starting" => cliclack::log::info("Starting: setting up a watch on every folder")?,
        "paused" => cliclack::log::warning("Paused: `omafind resume` goes on")?,
        "paused_power_saver" => {
            cliclack::log::warning("Paused while the power profile is power-saver")?
        }
        "waiting_for_ac" => cliclack::log::warning("Waiting for a charger")?,
        _ => cliclack::log::success("Up to date, watching for changes")?,
    }
    ui::line(ui::bold("Folders"));
    for root in &s.roots {
        ui::line(format!(
            "  {}{}",
            omafind_core::paths::tilde(&root.path),
            if root.code {
                ui::dim("  (code, searched with code:)")
            } else {
                String::new()
            }
        ));
    }
    ui::line("");
    if !s.kinds.is_empty() {
        ui::line(ui::bold("Files"));
        let width = s
            .kinds
            .iter()
            .map(|(_, _, t)| thousands(*t).len())
            .max()
            .unwrap_or(1);
        for (kind, done, total) in &s.kinds {
            ui::line(format!(
                "  {:<12} {}  {:>width$} / {:<width$}",
                kind_label(*kind),
                meter(*done, *total, 24),
                thousands(*done),
                thousands(*total),
            ));
        }
        ui::line("");
    }
    ui::line(ui::dim(format!(
        "llama.cpp {} · {} · query encoder {} · {}",
        s.llama_version,
        if s.on_battery { "on battery" } else { "on AC" },
        if s.query_encoder_loaded {
            "loaded"
        } else {
            "unloaded"
        },
        if s.vectors_in_memory > 0 {
            format!(
                "{} vectors in memory ({} MB)",
                thousands(s.vectors_in_memory as i64),
                s.vectors_bytes >> 20
            )
        } else {
            "vectors not in memory".into()
        }
    )));
    ui::line("");
    let eta = s
        .eta_seconds
        .map(|e| format!(" · about {} left", minutes(e)))
        .unwrap_or_default();
    let failed = if s.failed > 0 {
        format!(
            " · {} couldn't be read (`omafind status --failed`)",
            thousands(s.failed)
        )
    } else {
        String::new()
    };
    cliclack::outro(format!(
        "{} of {} files indexed{eta}{failed}",
        thousands(s.indexed),
        thousands(s.files)
    ))?;
    Ok(())
}

/// `1 h 50 min`, `12 min`, `under a minute`.
fn minutes(seconds: f64) -> String {
    let m = (seconds / 60.0).round() as u64;
    match m {
        0 => "under a minute".into(),
        1..=59 => format!("{m} min"),
        _ => format!("{} h {} min", m / 60, m % 60),
    }
}

fn status(index_path: &std::path::Path, json: bool) -> Result<()> {
    if Client::is_running() {
        let status = Client::connect()?.status()?;
        if json {
            println!("{}", serde_json::to_string_pretty(&status)?);
            return Ok(());
        }
        if ui::framed() {
            return status_framed(&status);
        }
        println!(
            "omafindd {}: {}{}",
            status.version,
            status.state,
            status
                .current
                .as_deref()
                .map(|c| format!(" ({})", omafind_core::paths::tilde(std::path::Path::new(c))))
                .unwrap_or_default()
        );
        for root in &status.roots {
            println!(
                "root:  {}{}",
                omafind_core::paths::tilde(&root.path),
                if root.code { " (code)" } else { "" }
            );
        }
        println!(
            "files: {} ({} indexed, {} pending, {} failed); {} chunks",
            status.files, status.indexed, status.pending, status.failed, status.chunks
        );
        println!(
            "power: {}{}; query encoder {}; worker {}",
            if status.on_battery { "battery" } else { "AC" },
            if status.power_saver {
                ", power-saver"
            } else {
                ""
            },
            if status.query_encoder_loaded {
                "loaded"
            } else {
                "unloaded"
            },
            if status.worker_running {
                "running"
            } else {
                "idle"
            }
        );
        println!(
            "engine: llama.cpp {}, vectors {}",
            status.llama_version, status.vector_version
        );
        return Ok(());
    }
    anyhow::ensure!(
        index_path.exists(),
        "no index yet: run `omafind index <folders>` first"
    );
    let store = Store::open(index_path)?;
    let stats = store.stats()?;
    let roots = store.roots()?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "index": index_path,
                "roots": roots.iter().map(|(p, code)| serde_json::json!({"path": p, "code": code})).collect::<Vec<_>>(),
                "stats": stats,
                "model": store.get_meta("model_id")?,
                "model_revision": store.get_meta("model_revision")?,
                "vector_version": store.vector_version()?,
                "llama_version": store.get_meta("llama_version")?,
            }))?
        );
        return Ok(());
    }
    println!("index: {}", omafind_core::paths::tilde(index_path));
    for (path, code) in &roots {
        println!(
            "root:  {}{}",
            omafind_core::paths::tilde(path),
            if *code { " (code)" } else { "" }
        );
    }
    println!(
        "files: {} ({} indexed, {} pending, {} failed); {} chunks, {} vectors",
        stats.files, stats.indexed, stats.pending, stats.failed, stats.chunks, stats.vectors
    );
    for (kind, n) in &stats.by_kind {
        println!("  {kind:<11} {n}");
    }
    println!(
        "model: {} @ {}, vectors {}, llama.cpp {}",
        store.get_meta("model_id")?.unwrap_or_default(),
        &store.get_meta("model_revision")?.unwrap_or_default()[..8],
        store.vector_version()?,
        store.get_meta("llama_version")?.unwrap_or_default()
    );
    Ok(())
}

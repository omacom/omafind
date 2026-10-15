//! `dig`: index folders and search them. Everything the window does is reachable here.

use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use dig_client::{Client, Request};
use dig_core::FileKind;
use dig_embed::{Embedder, Models, Options};
use dig_index::{Event, Indexer, Root, SearchOptions};
use dig_store::Store;

#[derive(Parser)]
#[command(name = "dig", version, about = "Local semantic search over your files")]
struct Cli {
    #[command(subcommand)]
    command: Command,
    /// Run the model on the CPU even when a GPU is available.
    #[arg(long, global = true)]
    cpu: bool,
    /// The index file (default: ~/.local/share/dig/index.sqlite).
    #[arg(long, global = true, env = "DIG_INDEX")]
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
    },
    /// Stop indexing until `dig resume` (remembered across restarts).
    Pause,
    Resume,
    /// Follow the daemon's progress until indexing is done.
    Watch,
    /// Show or hide the search window (starts dig-app if it isn't running).
    Toggle,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let index_path = cli.index.clone().unwrap_or_else(dig_core::paths::index_path);
    match cli.command {
        Command::Index { folders, code, reset } => index(&index_path, folders, code, reset, cli.cpu),
        Command::Search { query, json, kind, limit } => search(&index_path, &query.join(" "), json, &kind, limit, cli.cpu),
        Command::Status { json } => status(&index_path, json),
        Command::Pause => Client::connect()?.simple(&Request::Pause).map(|_| println!("paused")),
        Command::Resume => Client::connect()?.simple(&Request::Resume).map(|_| println!("resumed")),
        Command::Watch => watch(),
        Command::Toggle => toggle(),
    }
}

/// Hyprland keeps the window in the `special:dig` workspace; toggling that is instant. Without a window yet, the
/// app is started and the toggle waits for it to map (a few hundred milliseconds), the one time it is slow.
fn toggle() -> Result<()> {
    use std::process::{Command as Proc, Stdio};
    let hypr = |args: &[&str]| Proc::new("hyprctl").args(args).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned());
    let has_window = || hypr(&["-j", "clients"]).map(|j| j.contains("\"class\": \"dig\"")).unwrap_or(false);
    if !has_window() {
        let exe = std::env::current_exe()?.with_file_name("dig-app");
        let program = if exe.exists() { exe } else { PathBuf::from("dig-app") };
        Proc::new("setsid").arg("-f").arg(program).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().context("starting dig-app")?;
        let started = Instant::now();
        while !has_window() && started.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(50));
        }
        // A fresh window whose rule put it in the special workspace is hidden: fall through to show it. Without
        // the rules (not set up yet) the window is already on screen, and the toggle below would open an empty
        // special workspace, so check first.
        let in_special = hypr(&["-j", "clients"]).map(|j| j.contains("\"name\": \"special:dig\"")).unwrap_or(false);
        if !in_special {
            return Ok(());
        }
    }
    let out = hypr(&["dispatch", "hl.dsp.workspace.toggle_special(\"dig\")"])?;
    anyhow::ensure!(out.trim() == "ok", "hyprctl: {}", out.trim());
    Ok(())
}

/// Prints the daemon's progress until nothing is pending and no worker runs.
fn watch() -> Result<()> {
    let client = Client::connect()?;
    client.subscribe(|event| {
        match event {
            dig_client::Event::Status(s) => {
                let current = s.current.as_deref().map(|c| format!("  {}", dig_core::paths::tilde(std::path::Path::new(c)))).unwrap_or_default();
                eprintln!("{:<18} {} indexed, {} pending, {} failed{current}", s.state, s.indexed, s.pending, s.failed);
                if s.state == "idle" && s.pending == 0 && !s.worker_running {
                    return false;
                }
            }
            dig_client::Event::Indexed { path, kind } => eprintln!("  {:<10} {}", kind.as_str(), dig_core::paths::tilde(&path)),
            dig_client::Event::Failed { path, error } => eprintln!("  failed     {}: {error}", dig_core::paths::tilde(&path)),
        }
        true
    })
}

fn load_embedder(text_only: bool, cpu: bool) -> Result<Embedder> {
    let models = Models::locate(&dig_core::model::dir())?;
    Embedder::new(&models, &Options { text_only, cpu_only: cpu, threads: 0 }).context("loading the model")
}

fn index(index_path: &std::path::Path, folders: Vec<PathBuf>, code: Vec<PathBuf>, reset: bool, cpu: bool) -> Result<()> {
    let mut roots: Vec<Root> = folders.iter().map(|p| dig_index::indexer::resolve_root(p).map(|path| Root { path, code: false })).collect::<Result<_>>()?;
    roots.extend(code.iter().map(|p| dig_index::indexer::resolve_root(p).map(|path| Root { path, code: true })).collect::<Result<Vec<_>>>()?);
    // The folders are remembered in the config, which the daemon reads too.
    if !roots.is_empty() {
        let mut config = dig_core::config::Config::load().unwrap_or_default();
        config.set_folders(&roots.iter().filter(|r| !r.code).map(|r| r.path.clone()).collect::<Vec<_>>(), &roots.iter().filter(|r| r.code).map(|r| r.path.clone()).collect::<Vec<_>>());
        config.save()?;
    }
    if Client::is_running() && !reset {
        let mut client = Client::connect()?;
        if roots.is_empty() {
            client.simple(&Request::Reindex)?;
        } else {
            client.simple(&Request::SetRoots {
                folders: roots.iter().filter(|r| !r.code).map(|r| r.path.clone()).collect(),
                code: roots.iter().filter(|r| r.code).map(|r| r.path.clone()).collect(),
            })?;
        }
        eprintln!("digd is indexing; following its progress (Ctrl-C to stop watching, it keeps going)");
        return watch();
    }
    let mut store = Store::open(index_path)?;
    if roots.is_empty() {
        roots = dig_index::indexer::stored_roots(&store)?;
    }
    anyhow::ensure!(!roots.is_empty(), "nothing to index: `dig index ~/Pictures ~/Documents` picks the folders");
    if reset || store.vector_version()? != dig_core::model::VECTOR_VERSION {
        eprintln!("embedding everything again (vectors {})", dig_core::model::VECTOR_VERSION);
        store.reset_all()?;
        store.set_meta("vector_version", &dig_core::model::VECTOR_VERSION.to_string())?;
    }
    let started = Instant::now();
    let mut embedder = load_embedder(false, cpu)?;
    store.set_meta("llama_version", dig_embed::LLAMA_VERSION)?;
    eprintln!("model loaded in {:.1} s ({})", embedder.load_seconds, if dig_embed::gpu_found() && !cpu { "GPU" } else { "CPU" });
    let stderr = std::io::stderr();
    let mut indexer = Indexer {
        store: &mut store,
        embedder: &mut embedder,
        on_event: Box::new(move |event| {
            let mut err = stderr.lock();
            let _ = match event {
                Event::Crawled { root, files, pending, removed, skipped_secrets, skipped_code_dirs, huge_dirs } => {
                    let mut line = format!("{}: {files} files, {pending} to index", dig_core::paths::tilde(&root));
                    if removed > 0 { line += &format!(", {removed} gone"); }
                    if skipped_secrets > 0 { line += &format!(", {skipped_secrets} secrets skipped"); }
                    if skipped_code_dirs > 0 { line += &format!(", {skipped_code_dirs} code projects (screenshots only)"); }
                    for dir in huge_dirs { line += &format!("\n  skipped {} (more than {} entries)", dig_core::paths::tilde(&dir), dig_index::rules::MAX_DIR_ENTRIES); }
                    writeln!(err, "{line}")
                }
                Event::Reading { .. } => Ok(()),
                Event::Indexed { path, kind, chunks } => writeln!(err, "  {:<10} {:>3} chunks  {}", kind.as_str(), chunks, dig_core::paths::tilde(&path)),
                Event::Failed { path, error } => writeln!(err, "  failed            {}: {error}", dig_core::paths::tilde(&path)),
                Event::Done { indexed, failed, seconds } => writeln!(err, "{indexed} files indexed, {failed} failed, {:.1} s", seconds),
            };
        }),
        gate: Box::new(|| true),
    };
    indexer.run(&roots)?;
    eprintln!("done in {:.1} s", started.elapsed().as_secs_f64());
    Ok(())
}

fn search(index_path: &std::path::Path, query: &str, json: bool, kinds: &[String], limit: usize, cpu: bool) -> Result<()> {
    let kinds: Vec<FileKind> = kinds.iter().map(|k| FileKind::parse(k).with_context(|| format!("unknown kind {k}"))).collect::<Result<_>>()?;
    let (hits, loaded, searched) = if Client::is_running() {
        let t = Instant::now();
        let (hits, took_ms) = Client::connect()?.search(query, limit, kinds)?;
        (hits, Duration::ZERO, Duration::from_secs_f64((took_ms / 1000.0).max(0.0)).min(t.elapsed()))
    } else {
        anyhow::ensure!(index_path.exists(), "no index yet: run `dig index <folders>` first");
        let store = Store::open(index_path)?;
        let t0 = Instant::now();
        let mut embedder = load_embedder(true, cpu)?;
        let loaded = t0.elapsed();
        let t1 = Instant::now();
        let hits = dig_index::search(&store, &mut embedder, query, &SearchOptions { limit, kinds, candidates: 200 })?;
        (hits, loaded, t1.elapsed())
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&hits)?);
        return Ok(());
    }
    if hits.is_empty() {
        println!("nothing found for {query:?}");
    }
    for (i, hit) in hits.iter().enumerate() {
        let locator = if hit.locator_label.is_empty() { String::new() } else { format!("  {}", hit.locator_label) };
        let sim = hit.similarity.map(|s| format!("  {s:.2}")).unwrap_or_default();
        println!("{:>2}. {}  [{}{locator}]{sim}", i + 1, hit.name, hit.kind.as_str());
        println!("    {}", dig_core::paths::tilde(std::path::Path::new(&hit.path)));
        if !hit.snippet.is_empty() {
            println!("    {}", hit.snippet);
        }
    }
    if loaded.is_zero() {
        eprintln!("({} results via digd, {:.0} ms)", hits.len(), searched.as_secs_f64() * 1000.0);
    } else {
        eprintln!("({} results; model {:.2} s, search {:.0} ms)", hits.len(), loaded.as_secs_f64(), searched.as_secs_f64() * 1000.0);
    }
    Ok(())
}

fn status(index_path: &std::path::Path, json: bool) -> Result<()> {
    if Client::is_running() {
        let status = Client::connect()?.status()?;
        if json {
            println!("{}", serde_json::to_string_pretty(&status)?);
            return Ok(());
        }
        println!("digd {}: {}{}", status.version, status.state, status.current.as_deref().map(|c| format!(" ({})", dig_core::paths::tilde(std::path::Path::new(c)))).unwrap_or_default());
        for root in &status.roots {
            println!("root:  {}{}", dig_core::paths::tilde(&root.path), if root.code { " (code)" } else { "" });
        }
        println!("files: {} ({} indexed, {} pending, {} failed); {} chunks", status.files, status.indexed, status.pending, status.failed, status.chunks);
        println!("power: {}{}; query encoder {}; worker {}", if status.on_battery { "battery" } else { "AC" }, if status.power_saver { ", power-saver" } else { "" }, if status.query_encoder_loaded { "loaded" } else { "unloaded" }, if status.worker_running { "running" } else { "idle" });
        println!("engine: llama.cpp {}, vectors {}", status.llama_version, status.vector_version);
        return Ok(());
    }
    anyhow::ensure!(index_path.exists(), "no index yet: run `dig index <folders>` first");
    let store = Store::open(index_path)?;
    let stats = store.stats()?;
    let roots = store.roots()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({
            "index": index_path,
            "roots": roots.iter().map(|(p, code)| serde_json::json!({"path": p, "code": code})).collect::<Vec<_>>(),
            "stats": stats,
            "model": store.get_meta("model_id")?,
            "model_revision": store.get_meta("model_revision")?,
            "vector_version": store.vector_version()?,
            "llama_version": store.get_meta("llama_version")?,
        }))?);
        return Ok(());
    }
    println!("index: {}", dig_core::paths::tilde(index_path));
    for (path, code) in &roots {
        println!("root:  {}{}", dig_core::paths::tilde(path), if *code { " (code)" } else { "" });
    }
    println!("files: {} ({} indexed, {} pending, {} failed); {} chunks, {} vectors", stats.files, stats.indexed, stats.pending, stats.failed, stats.chunks, stats.vectors);
    for (kind, n) in &stats.by_kind {
        println!("  {kind:<11} {n}");
    }
    println!("model: {} @ {}, vectors {}, llama.cpp {}", store.get_meta("model_id")?.unwrap_or_default(), &store.get_meta("model_revision")?.unwrap_or_default()[..8], store.vector_version()?, store.get_meta("llama_version")?.unwrap_or_default());
    Ok(())
}

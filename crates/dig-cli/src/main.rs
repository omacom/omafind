//! `dig`: index folders and search them. Everything the window does is reachable here.

use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
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
    /// What the index holds.
    Status {
        #[arg(long)]
        json: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let index_path = cli.index.clone().unwrap_or_else(dig_core::paths::index_path);
    match cli.command {
        Command::Index { folders, code, reset } => index(&index_path, folders, code, reset, cli.cpu),
        Command::Search { query, json, kind, limit } => search(&index_path, &query.join(" "), json, &kind, limit, cli.cpu),
        Command::Status { json } => status(&index_path, json),
    }
}

fn load_embedder(text_only: bool, cpu: bool) -> Result<Embedder> {
    let models = Models::locate(&dig_core::model::dir())?;
    Embedder::new(&models, &Options { text_only, cpu_only: cpu, threads: 0 }).context("loading the model")
}

fn index(index_path: &std::path::Path, folders: Vec<PathBuf>, code: Vec<PathBuf>, reset: bool, cpu: bool) -> Result<()> {
    let mut store = Store::open(index_path)?;
    let mut roots: Vec<Root> = folders.iter().map(|p| dig_index::indexer::resolve_root(p).map(|path| Root { path, code: false })).collect::<Result<_>>()?;
    roots.extend(code.iter().map(|p| dig_index::indexer::resolve_root(p).map(|path| Root { path, code: true })).collect::<Result<Vec<_>>>()?);
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
                Event::Indexed { path, kind, chunks } => writeln!(err, "  {:<10} {:>3} chunks  {}", kind.as_str(), chunks, dig_core::paths::tilde(&path)),
                Event::Failed { path, error } => writeln!(err, "  failed            {}: {error}", dig_core::paths::tilde(&path)),
                Event::Done { indexed, failed, seconds } => writeln!(err, "{indexed} files indexed, {failed} failed, {:.1} s", seconds),
            };
        }),
    };
    indexer.run(&roots)?;
    eprintln!("done in {:.1} s", started.elapsed().as_secs_f64());
    Ok(())
}

fn search(index_path: &std::path::Path, query: &str, json: bool, kinds: &[String], limit: usize, cpu: bool) -> Result<()> {
    anyhow::ensure!(index_path.exists(), "no index yet: run `dig index <folders>` first");
    let store = Store::open(index_path)?;
    let kinds: Vec<FileKind> = kinds.iter().map(|k| FileKind::parse(k).with_context(|| format!("unknown kind {k}"))).collect::<Result<_>>()?;
    let t0 = Instant::now();
    let mut embedder = load_embedder(true, cpu)?;
    let loaded = t0.elapsed();
    let t1 = Instant::now();
    let hits = dig_index::search(&store, &mut embedder, query, &SearchOptions { limit, kinds, candidates: 200 })?;
    let searched = t1.elapsed();
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
    eprintln!("({} results; model {:.2} s, search {:.0} ms)", hits.len(), loaded.as_secs_f64(), searched.as_secs_f64() * 1000.0);
    Ok(())
}

fn status(index_path: &std::path::Path, json: bool) -> Result<()> {
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

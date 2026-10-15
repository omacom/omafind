//! Phase 0 spike: does EmbeddingGemma 2 run here, and does it rank right?
//!
//!   embed-spike demo [--cpu] [--models DIR] [--testbed DIR]
//!       "a dog on the beach" against evals/testbed/img/*.jpg, and a sleep query against evals/testbed/audio/*.wav
//!   embed-spike golden [--cpu] [--models DIR] <reference dir> [--write]
//!       the vectors this engine makes for the fixed inputs in evals/reference, against its vectors.json
//!       (cosine ≥ 0.999); --write records them
//!   embed-spike text [--cpu] "query" "doc 1" "doc 2" …

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use dig_core::prompts;
use dig_embed::{Embedder, Models, Options, cosine};

struct Args {
    command: String,
    rest: Vec<String>,
    cpu: bool,
    text_only: bool,
    models: Option<PathBuf>,
    testbed: Option<PathBuf>,
}

fn parse() -> Result<Args> {
    let mut args = std::env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "help".into());
    let mut parsed = Args {
        command,
        rest: Vec::new(),
        cpu: false,
        text_only: false,
        models: None,
        testbed: None,
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--cpu" => parsed.cpu = true,
            "--text-only" => parsed.text_only = true,
            "--models" => {
                parsed.models = Some(args.next().context("--models needs a directory")?.into())
            }
            "--testbed" => {
                parsed.testbed = Some(args.next().context("--testbed needs a directory")?.into())
            }
            _ => parsed.rest.push(arg),
        }
    }
    Ok(parsed)
}

fn files(dir: &Path, extensions: &[&str]) -> Result<Vec<PathBuf>> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| extensions.contains(&e.to_ascii_lowercase().as_str()))
        })
        .collect();
    out.sort();
    Ok(out)
}

fn name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn load(args: &Args) -> Result<Embedder> {
    let dir = args.models.clone().unwrap_or_else(dig_core::model::dir);
    let models = Models::locate(&dir)?;
    let started = Instant::now();
    let embedder = Embedder::new(
        &models,
        &Options {
            text_only: args.text_only,
            cpu_only: args.cpu,
            ..Options::default()
        },
    )?;
    eprintln!(
        "loaded in {:.2} s: llama.cpp {}, backends {:?}, gpu {}{}",
        started.elapsed().as_secs_f64(),
        dig_embed::LLAMA_VERSION,
        dig_embed::backend_names(),
        if dig_embed::gpu_found() {
            "found"
        } else {
            "none"
        },
        if args.cpu { " (left out: --cpu)" } else { "" }
    );
    Ok(embedder)
}

/// A file to rank: its name, its vector (when embedding worked) and the error (when it didn't).
type Candidate = (String, Option<Vec<f32>>, Option<String>);

/// Prints every candidate's cosine with the query, best first, and whether `expected` came first.
fn rank(label: &str, query: &[f32], candidates: &[Candidate], expected: &str) -> bool {
    let mut scored: Vec<(f32, &str)> = candidates
        .iter()
        .filter_map(|(name, vector, _)| vector.as_ref().map(|v| (cosine(query, v), name.as_str())))
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    println!("\n{label}");
    for (score, name) in &scored {
        println!("  {score:.4}  {name}");
    }
    for (name, _, error) in candidates {
        if let Some(error) = error {
            println!("  failed  {name}: {error}");
        }
    }
    let passed = scored
        .first()
        .is_some_and(|(_, name)| name.contains(expected));
    println!(
        "  → {}",
        if passed {
            format!("PASS: {expected} ranks first")
        } else {
            format!("FAIL: expected {expected} first")
        }
    );
    passed
}

fn demo(args: &Args) -> Result<bool> {
    let testbed = args
        .testbed
        .clone()
        .unwrap_or_else(|| PathBuf::from("evals/testbed"));
    let mut embedder = load(args)?;
    let mut all_passed = true;

    let images = files(&testbed.join("img"), &["jpg", "jpeg", "png"])?;
    let started = Instant::now();
    let image_vectors = embedder.embed_images(&images, 140)?;
    eprintln!(
        "{} images embedded in {:.2} s",
        images.len(),
        started.elapsed().as_secs_f64()
    );
    let image_candidates: Vec<_> = images
        .iter()
        .zip(image_vectors)
        .map(|(p, e)| (name(p), e.vector, e.error))
        .collect();
    for query in [
        "a dog on the beach",
        "a red car",
        "a plate of food",
        "snowy mountains",
    ] {
        let expected = match query {
            "a dog on the beach" => "dog_beach",
            "a red car" => "red_car",
            "a plate of food" => "spaghetti",
            _ => "mountain",
        };
        let started = Instant::now();
        let q = embedder.embed_texts(&[prompts::query(query)])?.remove(0);
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        all_passed &= rank(
            &format!("image query {query:?} ({ms:.0} ms to embed)"),
            &q,
            &image_candidates,
            expected,
        );
    }

    let clips = files(&testbed.join("audio"), &["wav", "mp3", "flac"])?;
    let started = Instant::now();
    let audio_vectors = embedder.embed_audio(&clips)?;
    eprintln!(
        "{} clips embedded in {:.2} s",
        clips.len(),
        started.elapsed().as_secs_f64()
    );
    let audio_candidates: Vec<_> = clips
        .iter()
        .zip(audio_vectors)
        .map(|(p, e)| (name(p), e.vector, e.error))
        .collect();
    for (query, expected) in [
        ("where they talk about sleep", "sleep"),
        ("someone who can't sleep and wakes up at night", "sleep"),
        ("buying groceries", "groceries"),
        ("a delayed flight", "flight"),
    ] {
        let q = embedder.embed_texts(&[prompts::query(query)])?.remove(0);
        all_passed &= rank(
            &format!("audio query {query:?}"),
            &q,
            &audio_candidates,
            expected,
        );
    }

    // Text against text, the plain case.
    let docs = [
        (
            "lease-pets",
            prompts::document(
                Some("Lease"),
                "Tenants may keep up to two pets with the landlord's written consent; a pet deposit of one month's rent applies.",
            ),
        ),
        (
            "lease-rent",
            prompts::document(
                Some("Lease"),
                "Rent is due on the first of each month by bank transfer; late payments incur a 5% fee after ten days.",
            ),
        ),
        (
            "recipe",
            prompts::document(
                None,
                "Preheat the oven to 180 degrees and mix the flour, sugar and cocoa powder.",
            ),
        ),
        (
            "error",
            prompts::document(
                None,
                "Your payment was declined. Please check your card details and try again.",
            ),
        ),
    ];
    let doc_vectors =
        embedder.embed_texts(&docs.iter().map(|(_, t)| t.clone()).collect::<Vec<_>>())?;
    let text_candidates: Vec<_> = docs
        .iter()
        .zip(doc_vectors)
        .map(|((n, _), v)| (n.to_string(), Some(v), None))
        .collect();
    for (query, expected) in [
        ("the clause about pets in the lease", "lease-pets"),
        ("payment declined error", "error"),
    ] {
        let q = embedder.embed_texts(&[prompts::query(query)])?.remove(0);
        all_passed &= rank(
            &format!("text query {query:?}"),
            &q,
            &text_candidates,
            expected,
        );
    }
    Ok(all_passed)
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Reference {
    vector_version: u32,
    made_with: String,
    vectors: std::collections::BTreeMap<String, Vec<f32>>,
}

/// The fixed inputs: five texts through the prompts, the screenshot at 280 tokens, the photo at 140 and as a 70-token
/// video frame, and the speech clip. `evals/make-reference.sh` makes the files.
fn reference_vectors(
    embedder: &mut Embedder,
    folder: &Path,
) -> Result<std::collections::BTreeMap<String, Vec<f32>>> {
    let long: String = (1..=60)
        .map(|i| format!("Note {i}: the review moved to room {} because the projector in the main hall failed again, and the team agreed to send the slides the night before.", i % 7 + 2))
        .collect::<Vec<_>>()
        .join(" ");
    let texts = [
        ("query", prompts::query("payment declined error")),
        ("query-bn", prompts::query("ইলিশ মাছ রান্না")),
        ("query-ar", prompts::query("متى تغلق المكتبة")),
        (
            "document",
            prompts::document(
                Some("Tokyo itinerary"),
                "Flight to Tokyo Haneda on November 3. Then the train to Kamakura to see the Great Buddha, and back for dinner in Shibuya.",
            ),
        ),
        // About 1,800 tokens: past the sliding attention window, so the global layers count as well.
        ("document-long", prompts::document(None, &long)),
    ];
    let mut made = std::collections::BTreeMap::new();
    for ((name, _), vector) in texts
        .iter()
        .zip(embedder.embed_texts(&texts.iter().map(|(_, t)| t.clone()).collect::<Vec<_>>())?)
    {
        made.insert(name.to_string(), vector);
    }
    for (name, file, budget) in [
        ("screen-280", "screen.png", 280),
        ("photo-140", "photo.jpg", 140),
        ("frame-70", "photo.jpg", 70),
    ] {
        let embedding = embedder
            .embed_images(&[folder.join(file)], budget)?
            .remove(0);
        made.insert(
            name.to_string(),
            embedding
                .vector
                .with_context(|| format!("{file}: {}", embedding.error.unwrap_or_default()))?,
        );
    }
    let speech = embedder
        .embed_audio(&[folder.join("speech.wav")])?
        .remove(0);
    made.insert(
        "speech".into(),
        speech
            .vector
            .with_context(|| format!("speech.wav: {}", speech.error.unwrap_or_default()))?,
    );
    Ok(made)
}

/// `golden <dir>`: the vectors this engine makes for the fixed inputs against `<dir>/vectors.json`.
/// `golden <dir> --write`: records them instead (a new engine that changed them needs a VECTOR_VERSION bump first).
fn golden(args: &Args) -> Result<bool> {
    let folder = args
        .rest
        .iter()
        .find(|a| !a.starts_with("--"))
        .map(PathBuf::from)
        .context("golden needs a reference folder with screen.png, photo.jpg and speech.wav")?;
    let write = args.rest.iter().any(|a| a == "--write");
    let mut embedder = load(args)?;
    if embedder.text_only {
        bail!("golden needs the media encoders");
    }
    let made = reference_vectors(&mut embedder, &folder)?;
    let path = folder.join("vectors.json");
    if write {
        let reference = Reference {
            vector_version: dig_core::model::VECTOR_VERSION,
            made_with: format!("llama.cpp {}", dig_embed::LLAMA_VERSION),
            vectors: made,
        };
        std::fs::write(&path, serde_json::to_vec_pretty(&reference)?)?;
        println!(
            "wrote {} (vectors {}, {})",
            path.display(),
            reference.vector_version,
            reference.made_with
        );
        return Ok(true);
    }
    let reference: Reference = serde_json::from_slice(
        &std::fs::read(&path)
            .with_context(|| format!("reading {}; write it with --write", path.display()))?,
    )?;
    println!(
        "reference: vectors {} made with {}; this engine: llama.cpp {}",
        reference.vector_version,
        reference.made_with,
        dig_embed::LLAMA_VERSION
    );
    let mut passed = reference.vector_version == dig_core::model::VECTOR_VERSION;
    if !passed {
        println!(
            "the reference is for vectors {}, this build makes {}",
            reference.vector_version,
            dig_core::model::VECTOR_VERSION
        );
    }
    println!("\n{:<14} {:>8}", "input", "cosine");
    for (name, vector) in &made {
        let Some(expected) = reference.vectors.get(name) else {
            println!("{name:<14} missing from the reference");
            passed = false;
            continue;
        };
        let c = cosine(vector, expected);
        let ok = c >= 0.999;
        passed &= ok;
        println!(
            "{name:<14} {c:>8.5} {}",
            if ok { "" } else { "← below 0.999" }
        );
    }
    println!(
        "\n→ {}",
        if passed {
            "PASS: every vector matches the reference within cosine 0.999"
        } else {
            "FAIL: this engine makes different vectors; keep the old one or bump VECTOR_VERSION and re-embed"
        }
    );
    Ok(passed)
}

fn text(args: &Args) -> Result<bool> {
    let mut embedder = load(args)?;
    let (query, docs) = args
        .rest
        .split_first()
        .context("text needs a query and documents")?;
    let q = embedder.embed_texts(&[prompts::query(query)])?.remove(0);
    let vectors = embedder.embed_texts(
        &docs
            .iter()
            .map(|d| prompts::document(None, d))
            .collect::<Vec<_>>(),
    )?;
    let candidates: Vec<_> = docs
        .iter()
        .zip(vectors)
        .map(|(d, v)| (d.clone(), Some(v), None))
        .collect();
    rank(&format!("query {query:?}"), &q, &candidates, "");
    Ok(true)
}

/// Text throughput at several batch sizes: 96 passages of ~350 tokens each.
fn bench(args: &Args) -> Result<bool> {
    let models = Models::locate(&args.models.clone().unwrap_or_else(dig_core::model::dir))?;
    let passages: Vec<String> = (0..96)
        .map(|i| prompts::document(Some("notes"), &format!("Meeting note {i}: the review moved to room {} because the projector failed again, and the team agreed to send the slides the night before. ", i % 7 + 2).repeat(9)))
        .collect();
    for (tokens, seqs) in [(2048usize, 16i32), (4096, 32), (8192, 64), (16384, 64)] {
        let mut embedder = Embedder::new(
            &models,
            &Options {
                text_only: false,
                cpu_only: args.cpu,
                threads: 0,
                batch_tokens: tokens,
                batch_sequences: seqs,
            },
        )?;
        let started = Instant::now();
        let out = embedder.embed_texts(&passages)?;
        let secs = started.elapsed().as_secs_f64();
        println!(
            "batch {tokens:>5} tokens × {seqs:>2} seqs: {} passages in {secs:.2} s = {:.1} passages/s",
            out.len(),
            out.len() as f64 / secs
        );
    }
    let images = files(
        &args
            .testbed
            .clone()
            .unwrap_or_else(|| PathBuf::from("evals/testbed"))
            .join("img"),
        &["jpg", "jpeg", "png"],
    )?;
    let mut embedder = Embedder::new(
        &models,
        &Options {
            cpu_only: args.cpu,
            ..Options::default()
        },
    )?;
    let started = Instant::now();
    let n = embedder.embed_images(&images, 140)?.len();
    println!(
        "images: {n} in {:.2} s = {:.2} s each",
        started.elapsed().as_secs_f64(),
        started.elapsed().as_secs_f64() / n.max(1) as f64
    );
    Ok(true)
}

fn main() -> Result<()> {
    let args = parse()?;
    let passed = match args.command.as_str() {
        "bench" => bench(&args)?,
        "demo" => demo(&args)?,
        "golden" => golden(&args)?,
        "text" => text(&args)?,
        _ => {
            eprintln!(
                "usage: embed-spike demo|golden <reference dir> [--write]|text <query> <doc>… [--cpu] [--text-only] [--models DIR] [--testbed DIR]"
            );
            return Ok(());
        }
    };
    if !passed {
        std::process::exit(1);
    }
    Ok(())
}

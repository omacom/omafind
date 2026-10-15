//! The model files from Hugging Face, resumable, checked by size and SHA-256 before they get their final name.
//! A copy in the Hugging Face cache at the right size is used instead of downloading.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

pub struct ModelFile {
    pub name: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
}

pub const TEXT: ModelFile = ModelFile {
    name: dig_core::model::TEXT_FILE,
    bytes: 309_855_456,
    sha256: "2188ac1deca4b77dffefd603c2776a9d76d9d74ec01841392982ebb840b09135",
};
pub const MEDIA: ModelFile = ModelFile {
    name: dig_core::model::MEDIA_FILE,
    bytes: 554_821_024,
    sha256: "c4a8a52691ecef40618438928bdf9e68379b854e24166f292592353db0aab64f",
};

fn hf_snapshot() -> PathBuf {
    let hub = std::env::var_os("HF_HUB_CACHE")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HF_HOME").map(|h| PathBuf::from(h).join("hub")))
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache/huggingface/hub"));
    hub.join(format!("models--{}", dig_core::model::REPOSITORY.replace('/', "--"))).join("snapshots").join(dig_core::model::REVISION)
}

/// Whether `file` is in `dir` already.
pub fn present(dir: &Path, file: &ModelFile) -> bool {
    dir.join(file.name).exists()
}

/// Fetches `file` into `dir` unless it is there, reporting progress on stderr.
pub fn fetch(dir: &Path, file: &ModelFile) -> Result<()> {
    fetch_with(dir, file, &mut |done, total| {
        eprint!("\r{}: {:>3}% of {} MB   ", file.name, done * 100 / total.max(1), total / 1_000_000);
    })
}

/// Like `fetch`, with progress as (bytes done, bytes total).
pub fn fetch_with(dir: &Path, file: &ModelFile, progress: &mut dyn FnMut(u64, u64)) -> Result<()> {
    let target = dir.join(file.name);
    if target.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(dir)?;
    let part = dir.join(format!("{}.part", file.name));
    let cached = hf_snapshot().join(file.name);
    if std::fs::metadata(&cached).map(|m| m.len() == file.bytes).unwrap_or(false) {
        eprintln!("{}: copying from the Hugging Face cache", file.name);
        std::fs::copy(&cached, &part)?;
    } else {
        download(&part, file, progress)?;
    }
    let size = std::fs::metadata(&part)?.len();
    if size != file.bytes {
        bail!("{}: wrong size ({size} bytes, expected {})", file.name, file.bytes);
    }
    eprint!("{}: checking…", file.name);
    let got = sha256_of(&part)?;
    if got != file.sha256 {
        let _ = std::fs::remove_file(&part);
        bail!("{}: SHA-256 mismatch ({got})", file.name);
    }
    eprintln!(" ok");
    std::fs::rename(&part, &target)?;
    Ok(())
}

fn download(part: &Path, file: &ModelFile, progress: &mut dyn FnMut(u64, u64)) -> Result<()> {
    let url = format!("https://huggingface.co/{}/resolve/{}/{}", dig_core::model::REPOSITORY, dig_core::model::REVISION, file.name);
    let have = std::fs::metadata(part).map(|m| m.len()).unwrap_or(0);
    if have >= file.bytes {
        return Ok(());
    }
    let agent = ureq::Agent::config_builder().timeout_global(None).build().new_agent();
    let mut request = agent.get(&url);
    if have > 0 {
        request = request.header("Range", &format!("bytes={have}-"));
    }
    let response = request.call().with_context(|| format!("downloading {}", file.name))?;
    let status = response.status().as_u16();
    let resume = status == 206;
    if !(status == 200 || resume) {
        bail!("{}: HTTP {status}", file.name);
    }
    let mut out = std::fs::OpenOptions::new().create(true).append(resume).write(true).truncate(!resume).open(part)?;
    let mut done = if resume { have } else { 0 };
    let mut body = response.into_body().into_reader();
    let mut buffer = vec![0u8; 1 << 20];
    let mut last_report = std::time::Instant::now();
    loop {
        let n = body.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        out.write_all(&buffer[..n])?;
        done += n as u64;
        if last_report.elapsed().as_millis() >= 200 {
            last_report = std::time::Instant::now();
            progress(done, file.bytes);
        }
    }
    progress(done, file.bytes);
    Ok(())
}

fn sha256_of(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 8 << 20];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

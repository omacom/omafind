//! Audio and video through ffmpeg (on every Omarchy install). Audio becomes overlapping 30-second windows of
//! 16 kHz mono, each a chunk at its start time. Video becomes keyframes (scene changes, and at least one frame
//! every `FRAME_FLOOR_SECONDS`) plus its soundtrack as audio windows. The model hears 30 s at a time, so that is
//! the window; the hop leaves an overlap so a sentence on a boundary is heard whole in one of them.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use dig_core::ChunkKind;

use crate::{Chunk, Embed, FRAME_BUDGET};

pub const SAMPLE_RATE: u32 = 16_000;
pub const WINDOW_SECONDS: f64 = 30.0;
pub const HOP_SECONDS: f64 = 20.0;
/// At least one frame this often, between scene changes.
pub const FRAME_FLOOR_SECONDS: f64 = 10.0;
/// Scene-change threshold for ffmpeg's `select=gt(scene,…)`.
pub const SCENE_THRESHOLD: f64 = 0.3;
/// Caps per file in this phase; the rest of a very long recording waits for the long-file pass.
pub const MAX_WINDOWS: usize = 360; // 2 hours at a 20 s hop
pub const MAX_FRAMES: usize = 400;
/// Quieter than this (RMS of 16-bit samples) a window is silence and skipped.
const SILENCE_RMS: f64 = 60.0;

/// Where windows and frames are written while a file is being indexed. The indexer clears it after each round.
pub fn scratch_dir() -> PathBuf {
    dig_core::paths::cache_dir().join("scratch")
}

pub fn extract_audio(path: &Path) -> Result<Vec<Chunk>> {
    let samples = decode_audio(path)?;
    audio_windows(path, &samples)
}

pub fn extract_video(path: &Path) -> Result<Vec<Chunk>> {
    let mut chunks = frames(path)?;
    match decode_audio(path) {
        Ok(samples) => chunks.extend(audio_windows(path, &samples)?),
        // A silent video is fine.
        Err(_) => {}
    }
    if chunks.is_empty() {
        bail!("no frames and no sound");
    }
    Ok(chunks)
}

/// The whole file as 16 kHz mono 16-bit samples.
fn decode_audio(path: &Path) -> Result<Vec<i16>> {
    let output = Command::new("ffmpeg")
        .args(["-nostdin", "-v", "error", "-i"])
        .arg(path)
        .args(["-vn", "-ac", "1", "-ar"])
        .arg(SAMPLE_RATE.to_string())
        .args(["-f", "s16le", "-"])
        .stderr(Stdio::piped())
        .output()
        .context("running ffmpeg")?;
    if !output.status.success() {
        bail!("ffmpeg: {}", String::from_utf8_lossy(&output.stderr).lines().last().unwrap_or("failed"));
    }
    if output.stdout.len() < 2 * SAMPLE_RATE as usize / 2 {
        bail!("no audio");
    }
    Ok(output.stdout.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect())
}

/// Windows of `samples` as WAV files in the scratch directory, one chunk each, silent ones skipped.
fn audio_windows(path: &Path, samples: &[i16]) -> Result<Vec<Chunk>> {
    let dir = scratch_dir().join(format!("{:016x}", hash(path)));
    std::fs::create_dir_all(&dir)?;
    let window = (WINDOW_SECONDS * SAMPLE_RATE as f64) as usize;
    let hop = (HOP_SECONDS * SAMPLE_RATE as f64) as usize;
    let mut chunks = Vec::new();
    let mut start = 0usize;
    while start < samples.len() && chunks.len() < MAX_WINDOWS {
        let end = (start + window).min(samples.len());
        let slice = &samples[start..end];
        // The last sliver after a hop is already covered by the previous window.
        if end - start < SAMPLE_RATE as usize * 3 && start > 0 {
            break;
        }
        if rms(slice) >= SILENCE_RMS {
            let seconds = start as f64 / SAMPLE_RATE as f64;
            let file = dir.join(format!("{:08.1}.wav", seconds));
            write_wav(&file, slice)?;
            chunks.push(Chunk {
                kind: ChunkKind::Audio,
                locator: Some(seconds),
                locator_end: Some(end as f64 / SAMPLE_RATE as f64),
                text: String::new(),
                embed: Embed::Audio { path: file },
            });
        }
        start += hop;
    }
    Ok(chunks)
}

/// Keyframes: scene changes, and one frame every `FRAME_FLOOR_SECONDS` otherwise, as 768-px JPEGs in the
/// scratch directory with the time they were taken.
fn frames(path: &Path) -> Result<Vec<Chunk>> {
    let dir = scratch_dir().join(format!("{:016x}-frames", hash(path)));
    std::fs::create_dir_all(&dir)?;
    let select = format!("select='gt(scene,{SCENE_THRESHOLD})+isnan(prev_selected_t)+gte(t-prev_selected_t,{FRAME_FLOOR_SECONDS})',showinfo,scale=768:-2");
    let output = Command::new("ffmpeg")
        .args(["-nostdin", "-v", "info", "-i"])
        .arg(path)
        .args(["-an", "-vf", &select, "-fps_mode", "vfr", "-frames:v"])
        .arg(MAX_FRAMES.to_string())
        .args(["-q:v", "4"])
        .arg(dir.join("%05d.jpg"))
        .stderr(Stdio::piped())
        .output()
        .context("running ffmpeg")?;
    if !output.status.success() {
        bail!("ffmpeg: {}", String::from_utf8_lossy(&output.stderr).lines().last().unwrap_or("failed"));
    }
    // showinfo logs one line per selected frame with its presentation time: `... n:  3 pts: 123 pts_time:4.1 ...`
    let log = String::from_utf8_lossy(&output.stderr);
    let times: Vec<f64> = log
        .lines()
        .filter(|l| l.contains("showinfo") && l.contains("pts_time:"))
        .filter_map(|l| l.split("pts_time:").nth(1)?.split_whitespace().next()?.parse().ok())
        .collect();
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)?.filter_map(|e| e.ok().map(|e| e.path())).collect();
    files.sort();
    Ok(files
        .into_iter()
        .enumerate()
        .map(|(i, file)| Chunk {
            kind: ChunkKind::Frame,
            locator: Some(times.get(i).copied().unwrap_or(i as f64 * FRAME_FLOOR_SECONDS)),
            locator_end: None,
            text: String::new(),
            embed: Embed::Image { path: file, budget: FRAME_BUDGET },
        })
        .collect())
}

fn rms(samples: &[i16]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / samples.len() as f64).sqrt()
}

/// 16 kHz mono 16-bit PCM WAV.
fn write_wav(path: &Path, samples: &[i16]) -> Result<()> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    out.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, out)?;
    Ok(())
}

fn hash(path: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_overlap_and_skip_silence() {
        let dir = tempfile::tempdir().unwrap();
        // 70 s: 0–35 s a tone, 35–70 s silence.
        let n = 70 * SAMPLE_RATE as usize;
        let samples: Vec<i16> = (0..n).map(|i| if i < 35 * SAMPLE_RATE as usize { ((i as f64 * 0.1).sin() * 8000.0) as i16 } else { 0 }).collect();
        let chunks = audio_windows(&dir.path().join("x.wav"), &samples).unwrap();
        let starts: Vec<f64> = chunks.iter().map(|c| c.locator.unwrap()).collect();
        // Windows at 0 and 20 s hold sound; 40 s and 60 s are silent.
        assert_eq!(starts, vec![0.0, 20.0]);
        assert_eq!(chunks[0].locator_end, Some(30.0));
        assert!(matches!(chunks[0].embed, Embed::Audio { .. }));
        let _ = std::fs::remove_dir_all(scratch_dir());
    }
}

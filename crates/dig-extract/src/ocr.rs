//! Text read off screenshots with tesseract (on every Omarchy install), for keyword search: an error message in a
//! screenshot is found by its exact words.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};

/// The text tesseract reads in `image`, or an empty string when it finds none. Languages: English plus whatever
/// `tesseract-data-*` packs are installed and named in `$DIG_OCR_LANGS` (e.g. `eng+deu`).
pub fn read(image: &Path) -> Result<String> {
    let langs = std::env::var("DIG_OCR_LANGS").unwrap_or_else(|_| "eng".into());
    let output = Command::new("tesseract")
        .arg(image)
        .arg("stdout")
        .args(["-l", &langs, "--psm", "3"])
        .output()
        .context("running tesseract")?;
    if !output.status.success() {
        anyhow::bail!("tesseract: {}", String::from_utf8_lossy(&output.stderr).lines().last().unwrap_or("failed"));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(clean(&text))
}

/// Collapses whitespace and drops lines that are mostly punctuation noise.
fn clean(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|line| {
            let letters = line.chars().filter(|c| c.is_alphanumeric()).count();
            letters >= 2 && letters * 2 >= line.chars().count()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    #[test]
    fn noise_lines_are_dropped() {
        assert_eq!(super::clean("Hello world\n|| -- ..\n  a1  \nx"), "Hello world\na1");
    }
}

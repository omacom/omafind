//! Office documents without an office suite: `.docx`, `.odt` and `.epub` are zip archives of XML, and their
//! paragraphs are the text between the tags. HTML is the same without the zip. Anything else (`.doc`, `.rtf`) goes
//! through `pandoc` when it is installed.

use std::io::Read;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

/// The document's text with paragraphs separated by blank lines.
pub fn read(path: &Path, ext: &str) -> Result<String> {
    match ext {
        "docx" => zipped_xml(path, &["word/document.xml"], &["w:p", "w:tab", "w:br"]),
        "odt" => zipped_xml(
            path,
            &["content.xml"],
            &["text:p", "text:h", "text:line-break", "text:tab"],
        ),
        "epub" => epub(path),
        "html" | "htm" => Ok(strip_tags(&std::fs::read_to_string(path)?)),
        _ => pandoc(path),
    }
}

/// Text of the named XML members of a zip, with a paragraph break at each of `breaks`.
fn zipped_xml(path: &Path, members: &[&str], breaks: &[&str]) -> Result<String> {
    let file = std::fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(file).context("not a zip archive")?;
    let mut out = String::new();
    for member in members {
        let mut entry = match archive.by_name(member) {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let mut xml = String::new();
        entry.read_to_string(&mut xml)?;
        out.push_str(&xml_text(&xml, breaks));
    }
    if out.trim().is_empty() {
        bail!("no text in {}", members.join(", "));
    }
    Ok(out)
}

/// An EPUB: every XHTML chapter in spine order (the order of the files in the zip is close enough).
fn epub(path: &Path) -> Result<String> {
    let file = std::fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(file).context("not a zip archive")?;
    let mut names: Vec<String> = (0..archive.len())
        .filter_map(|i| archive.by_index(i).ok().map(|e| e.name().to_owned()))
        .filter(|n| n.ends_with(".xhtml") || n.ends_with(".html") || n.ends_with(".htm"))
        .collect();
    names.sort();
    let mut out = String::new();
    for name in names {
        let mut entry = archive.by_name(&name)?;
        let mut html = String::new();
        if entry.read_to_string(&mut html).is_ok() {
            out.push_str(&strip_tags(&html));
            out.push_str("\n\n");
        }
    }
    if out.trim().is_empty() {
        bail!("no chapters");
    }
    Ok(out)
}

fn pandoc(path: &Path) -> Result<String> {
    let output = Command::new("pandoc")
        .arg(path)
        .args(["-t", "plain", "--wrap=none"])
        .output();
    match output {
        Ok(output) if output.status.success() => {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        }
        Ok(output) => bail!("pandoc: {}", String::from_utf8_lossy(&output.stderr).trim()),
        Err(_) => bail!("needs pandoc (pacman -S pandoc-cli)"),
    }
}

/// Text content of XML, with a paragraph break after each closing tag in `breaks`, entities decoded.
pub fn xml_text(xml: &str, breaks: &[&str]) -> String {
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut out = String::new();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(quick_xml::events::Event::Text(t)) => {
                if let Ok(text) = t.unescape() {
                    out.push_str(&text);
                }
            }
            Ok(quick_xml::events::Event::End(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                if breaks.contains(&name.as_str()) {
                    out.push_str("\n\n");
                }
            }
            Ok(quick_xml::events::Event::Empty(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                if breaks.contains(&name.as_str()) {
                    out.push(if name.ends_with("tab") { '\t' } else { '\n' });
                }
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    out
}

/// HTML to text: scripts, styles and the head dropped, block tags become paragraph breaks, entities decoded.
pub fn strip_tags(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    let lower_all = html.to_ascii_lowercase();
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let Some(end) = rest.find('>') else { break };
        let tag = &rest[1..end];
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        rest = &rest[end + 1..];
        if matches!(name.as_str(), "script" | "style" | "head")
            && !tag.starts_with('/')
            && !tag.ends_with('/')
        {
            // Skip to the matching close tag, whatever is inside (a `<` in a script is not a tag).
            let offset = html.len() - rest.len();
            let closing = format!("</{name}");
            match lower_all[offset..].find(&closing) {
                Some(at) => {
                    rest = &rest[at..];
                    if let Some(gt) = rest.find('>') {
                        rest = &rest[gt + 1..];
                    }
                }
                None => return decode_entities(&out),
            }
        } else if matches!(
            name.as_str(),
            "p" | "div"
                | "br"
                | "li"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "tr"
                | "section"
                | "article"
                | "blockquote"
                | "pre"
                | "td"
                | "th"
        ) {
            out.push_str(if name == "td" || name == "th" {
                " "
            } else {
                "\n\n"
            });
        }
    }
    out.push_str(rest);
    decode_entities(&out)
}

fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let Some(semi) = rest.find(';').filter(|&s| s < 12) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..semi];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some(' '),
            e if e.starts_with("#x") || e.starts_with("#X") => u32::from_str_radix(&e[2..], 16)
                .ok()
                .and_then(char::from_u32),
            e if e.starts_with('#') => e[1..].parse::<u32>().ok().and_then(char::from_u32),
            _ => None,
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_becomes_paragraphs() {
        let html = "<html><head><title>T</title><style>p{}</style></head><body><h1>Lease</h1><p>Pets &amp; <b>dogs</b> allowed.</p><script>x<y</script><p>Rent &#8364;1,450</p></body></html>";
        let text = strip_tags(html);
        assert!(text.contains("Lease"), "{text}");
        assert!(text.contains("Pets & dogs allowed."), "{text}");
        assert!(text.contains("Rent €1,450"), "{text}");
        assert!(!text.contains("x<y") && !text.contains("p{}"), "{text}");
    }

    #[test]
    fn docx_xml_paragraphs() {
        let xml = r#"<w:document><w:body><w:p><w:r><w:t>Hello</w:t></w:r><w:r><w:t xml:space="preserve"> world</w:t></w:r></w:p><w:p><w:r><w:t>Second &amp; last</w:t></w:r></w:p></w:body></w:document>"#;
        assert_eq!(
            xml_text(xml, &["w:p"]).trim(),
            "Hello world\n\nSecond & last"
        );
    }
}

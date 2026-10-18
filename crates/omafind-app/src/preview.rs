//! The preview of the selected result, on the right: the picture (a photo, a PDF page, a video frame), the matched
//! passage, or numbered lines for code and plain text with the matched paragraph tinted.

use gpui_kit::base::{StyledExt as _, h_flex, v_flex};
use gpui_kit::component::text::TextView;
use gpui_kit::{
    AnyElement, App, Context, IntoElement, ParentElement as _, Styled as _, div, img,
    prelude::FluentBuilder as _, px,
};
use omafind_client::Passage;
use omafind_core::{FileKind, Hit, Locator, Reason};

use crate::format::{self, escape_markdown, marks, plain};
use crate::search::{Preview, SearchView};
use crate::theme::Syntax;
use crate::widgets::{alpha, colors, pill, radius};

/// Loads what the preview of `hit` needs. Blocks: run it on the background executor.
pub(crate) fn load(hit: &Hit) -> Preview {
    let mut entry = Preview::default();
    let path = std::path::Path::new(&hit.path);
    if format::has_thumb(hit.kind) {
        entry.large = omafind_extract::thumbs::get(
            path,
            hit.kind,
            format::locator_value(hit),
            omafind_extract::thumbs::Size::Large,
        );
    }
    let code = hit.kind == FileKind::Code;
    match (hit.kind, hit.locator, hit.locator_end) {
        (FileKind::Code, locator, end) => {
            let first = match locator {
                Some(Locator::Line(l)) => l as usize,
                _ => 1,
            };
            let last = end.map_or(first, |e| e as usize).max(first).min(first + 79);
            entry.first = first;
            entry.last = last;
            entry.lines = Some(crate::actions::lines(
                path,
                first.saturating_sub(4).max(1),
                last + 4,
            ));
            entry.best = Some(best_line(&entry, &hit.matched_words));
        }
        // A plain text file shows as lines, like code; an office document's text isn't the file's, so a passage.
        (FileKind::Doc, Some(Locator::Offset(start)), Some(end)) => {
            match crate::actions::text_section(
                path,
                start as usize,
                end.max(start as f64) as usize,
                4,
                80,
            ) {
                Some((lines, first, last)) => {
                    entry.first = first;
                    entry.last = last;
                    entry.lines = Some(lines);
                    entry.best = Some(best_line(&entry, &hit.matched_words));
                }
                None => entry.passage = crate::service::passage(hit.chunk_id, code).ok(),
            }
        }
        (FileKind::Doc | FileKind::Pdf | FileKind::Screenshot, _, _) => {
            entry.passage = crate::service::passage(hit.chunk_id, code).ok();
        }
        _ => {}
    }
    entry
}

impl SearchView {
    pub(crate) fn render_preview(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = colors(cx);
        let Some(hit) = self.results.get(self.selected) else {
            return div().flex_1().into_any_element();
        };
        let key = format::thumb_key(hit);
        let data = self.previews.get(&key);
        let picture = data
            .and_then(|d| d.large.clone())
            .or_else(|| self.thumbs.get(&key).cloned().flatten());
        let body: AnyElement = if let Some(lines) = data.and_then(|d| d.lines.as_ref()) {
            lines_view(hit, data.unwrap(), lines, cx).into_any_element()
        } else if let Some(picture) = picture.filter(|_| hit.kind != FileKind::Doc) {
            let pane = div()
                .flex_1()
                .min_h_0()
                .rounded(radius(cx))
                .overflow_hidden()
                .bg(c.surface)
                .flex()
                .items_center()
                .justify_center()
                .child(img(picture).size_full());
            let passage = data
                .and_then(|d| d.passage.as_ref())
                .filter(|p| !p.text.is_empty() && hit.kind == FileKind::Pdf)
                .map(|p| passage_view(hit, &from_match(p, &hit.snippet), Some(140.), cx));
            v_flex()
                .flex_1()
                .min_h_0()
                .gap(px(10.))
                .child(pane)
                .children(passage)
                .into_any_element()
        } else if let Some(passage) = data.and_then(|d| d.passage.as_ref()) {
            passage_view(hit, passage, None, cx).into_any_element()
        } else if !hit.snippet.is_empty() {
            let passage = Passage {
                text: hit.snippet.clone(),
                before: String::new(),
                after: String::new(),
            };
            passage_view(hit, &passage, None, cx).into_any_element()
        } else {
            let sound = (hit.reason == Reason::Sound).then(|| match hit.locator_label.as_str() {
                "" => "Matched by sound".to_owned(),
                at => format!("Matched by sound, {at}"),
            });
            v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap(px(8.))
                .child(
                    div()
                        .text_size(px(48.))
                        .text_color(c.muted_foreground)
                        .child(format::glyph(hit.kind)),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(c.muted_foreground)
                        .child(sound.unwrap_or_default()),
                )
                .into_any_element()
        };
        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .px(px(20.))
            .py(px(16.))
            .gap(px(12.))
            .border_l_1()
            .border_color(c.border)
            // The list row already shows the name, tag, folder and date; only the grid, whose tiles carry none,
            // repeats them.
            .when(self.is_grid(), |pane| pane.child(grid_heading(hit, cx)))
            .child(body)
            .into_any_element()
    }
}

/// The selected tile's name, tag, look-alikes and where it is, over its preview in the grid.
fn grid_heading(hit: &Hit, cx: &App) -> impl IntoElement {
    let c = colors(cx);
    let mut info = vec![format::folder(hit)];
    if hit.kind == FileKind::Pdf
        && let Some(Locator::Page(page)) = hit.locator
    {
        info.push(format!("page {page}"));
    }
    info.push(format::when(hit.mtime));
    info.push(format::bytes(hit.size.max(0) as u64));
    let info = info
        .into_iter()
        .filter(|i| !i.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    let alike = (!hit.duplicates.is_empty()).then(|| {
        let names: Vec<&str> = hit
            .duplicates
            .iter()
            .take(3)
            .map(|d| d.rsplit('/').next().unwrap_or(d))
            .collect();
        let more = if hit.duplicates.len() > 3 { "…" } else { "" };
        pill(
            format!(
                "{} more like it: {}{more}",
                hit.duplicates.len(),
                names.join(", ")
            ),
            false,
            cx,
        )
    });
    v_flex()
        .gap(px(3.))
        .child(
            h_flex()
                .gap(px(8.))
                .items_center()
                .child(
                    div()
                        .text_size(px(14.))
                        .font_medium()
                        .text_color(c.foreground)
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(hit.name.clone()),
                )
                .child(pill(format::tag(hit), true, cx))
                .children(alike),
        )
        .child(
            div()
                .text_size(px(11.))
                .text_color(c.muted_foreground)
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(info),
        )
}

/// Numbered lines of code or plain text, the matched stretch tinted and its likeliest line (the one holding the most
/// query words) stronger. Code is coloured by a small tokenizer; text wraps.
fn lines_view(hit: &Hit, data: &Preview, lines: &[(usize, String)], cx: &App) -> AnyElement {
    let c = colors(cx);
    let code = hit.kind == FileKind::Code;
    let best = data.best.unwrap_or(data.first);
    let (top, bottom) = block(data, lines, best);
    let inside = |n: usize| n >= top && n <= bottom;
    let lang = code.then(|| language(&hit.path));
    let syntax = cx.try_global::<Syntax>().copied();
    let color = |kind: Token| match kind {
        Token::Plain => c.foreground,
        Token::Keyword => c.primary,
        Token::Number => syntax.map_or(c.primary, |s| s.number),
        Token::String => syntax.map_or(c.foreground, |s| s.string),
        Token::Comment => c.muted_foreground,
    };
    // The pane fits about 30 lines: start a little above the strongest line when it would fall below them.
    let from = lines
        .first()
        .map_or(1, |(n, _)| *n)
        .max(best.saturating_sub(8));
    let near_start = best.saturating_sub(data.first) < 22;
    v_flex()
        .flex_1()
        .min_h_0()
        .overflow_hidden()
        .rounded(radius(cx))
        .bg(c.surface)
        .py(px(8.))
        .children(
            lines
                .iter()
                .filter(|(n, _)| *n >= from || near_start)
                .map(|(n, text)| {
                    let n = *n;
                    let strength = if n == best {
                        0.18
                    } else if inside(n) {
                        0.1
                    } else {
                        0.
                    };
                    let gutter = if n == best {
                        c.foreground
                    } else if inside(n) {
                        c.muted_foreground
                    } else {
                        alpha(c.muted_foreground, 0.5)
                    };
                    let text = if text.is_empty() { " " } else { text.as_str() };
                    let body: AnyElement = if let Some(lang) = &lang {
                        h_flex()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .children(tokens(text, lang).into_iter().map(|(kind, piece)| {
                                div().flex_none().text_color(color(kind)).child(piece)
                            }))
                            .into_any_element()
                    } else {
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .flex_wrap()
                            .children(marks(text, &hit.matched_words).into_iter().flat_map(|m| {
                                let tint = if m.hit { c.primary } else { c.foreground };
                                format::words_of(m.text)
                                    .map(move |part| {
                                        div().flex_none().text_color(tint).child(part.to_owned())
                                    })
                                    .collect::<Vec<_>>()
                            }))
                            .into_any_element()
                    };
                    h_flex()
                        .items_start()
                        .px(px(14.))
                        .py(px(if code { 1. } else { 2. }))
                        .bg(if strength > 0. {
                            alpha(c.primary, strength)
                        } else {
                            c.surface
                        })
                        .text_size(px(12.))
                        .child(
                            div()
                                .w(px(40.))
                                .flex_none()
                                .text_color(gutter)
                                .child(n.to_string()),
                        )
                        .child(body)
                }),
        )
        .into_any_element()
}

/// The passage, as selectable Markdown: what comes before and after in plain text, the match with the query's words in
/// bold. `height` fixes the box (under a PDF page), else it fills the pane.
fn passage_view(hit: &Hit, p: &Passage, height: Option<f32>, cx: &App) -> AnyElement {
    let c = colors(cx);
    let bold: String = marks(&plain(&p.text), &hit.matched_words)
        .into_iter()
        .map(|m| {
            if m.hit {
                format!("**{}**", escape_markdown(m.text))
            } else {
                escape_markdown(m.text)
            }
        })
        .collect();
    let trim = |t: &str, n: usize, from_end: bool| -> String {
        let chars: Vec<char> = t.chars().collect();
        if chars.len() <= n {
            t.to_owned()
        } else if from_end {
            format!("…{}", chars[chars.len() - n..].iter().collect::<String>())
        } else {
            format!("{}…", chars[..n].iter().collect::<String>())
        }
    };
    let md = [
        (!p.before.is_empty()).then(|| escape_markdown(&trim(&plain(&p.before), 400, true))),
        Some(bold),
        (!p.after.is_empty()).then(|| escape_markdown(&trim(&plain(&p.after), 400, false))),
    ]
    .into_iter()
    .flatten()
    .filter(|s| !s.is_empty())
    .collect::<Vec<_>>()
    .join("\n\n");
    let view = TextView::markdown(format!("passage-{}", hit.chunk_id), md)
        .selectable(true)
        .scrollable(true);
    let frame = div()
        .w_full()
        .rounded(radius(cx))
        .bg(c.surface)
        .px(px(14.))
        .py(px(12.))
        .text_size(px(12.))
        .text_color(c.foreground)
        .overflow_hidden()
        .child(view);
    match height {
        Some(h) => frame.h(px(h)).into_any_element(),
        None => frame.flex_1().min_h_0().into_any_element(),
    }
}

/// The line of the matched stretch holding the most query words (the first on a tie, the stretch's start if none).
fn best_line(data: &Preview, words: &[String]) -> usize {
    let mut best = data.first;
    let mut most = 0;
    for (n, text) in data.lines.iter().flatten() {
        if *n < data.first || *n > data.last {
            continue;
        }
        let lower = text.to_lowercase();
        let count = words
            .iter()
            .filter(|w| lower.contains(&w.to_lowercase()))
            .count();
        if count > most {
            best = *n;
            most = count;
        }
    }
    best
}

/// The paragraph around `best`: the lines between the blank lines either side, within the matched stretch. A passage is
/// ~1,500 characters (a short file is one passage), so tinting all of it says little; the paragraph holding the words
/// is what the match is about.
fn block(data: &Preview, lines: &[(usize, String)], best: usize) -> (usize, usize) {
    let text = |n: usize| lines.iter().find(|(m, _)| *m == n).map(|(_, t)| t.as_str());
    let filled = |n: usize| text(n).is_some_and(|t| !t.trim().is_empty());
    let (mut from, mut to) = (best, best);
    while from > data.first && filled(from - 1) {
        from -= 1;
    }
    while to < data.last && filled(to + 1) {
        to += 1;
    }
    (from, to)
}

/// A page's text from shortly before where the row's snippet starts (where the query's words cluster), so the small
/// passage box under a PDF page shows the match rather than the page's first lines.
fn from_match(passage: &Passage, snippet: &str) -> Passage {
    // pdftotext keeps the page's layout as runs of spaces; the snippet doesn't, so compare with spaces collapsed.
    let flat = |t: &str| plain(t).split_whitespace().collect::<Vec<_>>().join(" ");
    let anchor: String = flat(snippet)
        .trim_start_matches('…')
        .trim_start()
        .chars()
        .take(40)
        .collect();
    let anchor = anchor.trim();
    let text = flat(&passage.text);
    let at = if anchor.is_empty() {
        None
    } else {
        text.find(anchor)
    };
    let Some(at) = at.filter(|&at| at > 0) else {
        return passage.clone();
    };
    // Back to the start of the sentence the match is in, if one starts within 200 characters.
    let mut head_start = at.saturating_sub(200);
    while !text.is_char_boundary(head_start) {
        head_start += 1;
    }
    let head = &text[head_start..at];
    let start = head.rfind(". ").map_or(at, |i| head_start + i + 1);
    Passage {
        text: format!("…{}", text[start..].trim_start()),
        before: String::new(),
        after: passage.after.clone(),
    }
}

fn language(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path).to_lowercase();
    match name.rsplit_once('.') {
        Some((_, ext)) => ext.to_owned(),
        None => name,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token {
    Plain,
    Keyword,
    Number,
    String,
    Comment,
}

const KEYWORDS: &[&str] = &[
    "as",
    "async",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "crate",
    "def",
    "default",
    "defer",
    "del",
    "do",
    "dyn",
    "elif",
    "else",
    "enum",
    "except",
    "export",
    "extends",
    "false",
    "fi",
    "final",
    "finally",
    "fn",
    "for",
    "from",
    "func",
    "function",
    "go",
    "if",
    "impl",
    "import",
    "in",
    "interface",
    "let",
    "local",
    "loop",
    "match",
    "mod",
    "module",
    "mut",
    "new",
    "nil",
    "None",
    "not",
    "null",
    "or",
    "package",
    "pass",
    "pub",
    "raise",
    "return",
    "self",
    "Self",
    "static",
    "struct",
    "super",
    "switch",
    "then",
    "this",
    "throw",
    "trait",
    "true",
    "True",
    "False",
    "try",
    "type",
    "typeof",
    "undefined",
    "unsafe",
    "use",
    "var",
    "where",
    "while",
    "with",
    "yield",
];
const HASH_COMMENTS: &[&str] = &[
    "py",
    "sh",
    "bash",
    "zsh",
    "fish",
    "rb",
    "toml",
    "yaml",
    "yml",
    "pl",
    "r",
    "nix",
    "conf",
    "cfg",
    "ini",
    "dockerfile",
    "mk",
    "cmake",
];
const DASH_COMMENTS: &[&str] = &["lua", "sql", "hs", "elm"];

/// One line of code as coloured pieces: comments, strings, numbers, keywords, the rest plain. Line by line, so a block
/// comment or string that spans lines colours only where it starts; good enough for a preview.
fn tokens(line: &str, lang: &str) -> Vec<(Token, String)> {
    let comment = if HASH_COMMENTS.contains(&lang) {
        "#"
    } else if DASH_COMMENTS.contains(&lang) {
        "--"
    } else {
        "//"
    };
    let single_quoted = lang != "rs";
    let mut out: Vec<(Token, String)> = Vec::new();
    let mut push = |kind: Token, text: &str| match out.last_mut() {
        Some((last, piece)) if *last == kind => piece.push_str(text),
        _ => out.push((kind, text.to_owned())),
    };
    let mut i = 0;
    while i < line.len() {
        let rest = &line[i..];
        if rest.starts_with(comment) || rest.starts_with("/*") {
            push(Token::Comment, rest);
            break;
        }
        let ch = rest.chars().next().unwrap();
        if ch == '"' || ch == '`' || (ch == '\'' && single_quoted) {
            let mut j = i + ch.len_utf8();
            while j < line.len() {
                let c = line[j..].chars().next().unwrap();
                if c == ch {
                    break;
                }
                j += if c == '\\' {
                    c.len_utf8() + line[j + 1..].chars().next().map_or(0, char::len_utf8)
                } else {
                    c.len_utf8()
                };
            }
            let end = (j + ch.len_utf8()).min(line.len());
            push(Token::String, &line[i..end]);
            i = end;
            continue;
        }
        if ch.is_ascii_alphabetic() || ch == '_' {
            let len = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(rest.len());
            let word = &rest[..len];
            push(
                if KEYWORDS.contains(&word) {
                    Token::Keyword
                } else {
                    Token::Plain
                },
                word,
            );
            i += len;
            continue;
        }
        if ch.is_ascii_digit() {
            let mut len = rest
                .find(|c: char| !(c.is_ascii_digit() || c == '_'))
                .unwrap_or(rest.len());
            if rest[len..].starts_with('.')
                && rest[len + 1..].starts_with(|c: char| c.is_ascii_digit())
            {
                len += 1 + rest[len + 1..]
                    .find(|c: char| !c.is_ascii_digit())
                    .unwrap_or(rest.len() - len - 1);
            }
            push(Token::Number, &rest[..len]);
            i += len;
            continue;
        }
        push(Token::Plain, &rest[..ch.len_utf8()]);
        i += ch.len_utf8();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_is_split_into_coloured_pieces() {
        let t = tokens("let x = \"a\\\"b\"; // hi", "rs");
        assert_eq!(t[0], (Token::Keyword, "let".into()));
        assert!(t.contains(&(Token::String, "\"a\\\"b\"".into())));
        assert_eq!(t.last().unwrap(), &(Token::Comment, "// hi".into()));
        assert_eq!(
            tokens("x = 1.5 # n", "py")[1],
            (Token::Number, "1.5".into())
        );
        // Rust's quotes are lifetimes and chars, not strings.
        assert_eq!(tokens("'c'", "rs")[0], (Token::Plain, "'c'".into()));
    }

    #[test]
    fn the_block_is_the_paragraph_around_the_best_line() {
        let lines: Vec<(usize, String)> = ["a", "b", "", "c", "d", "", "e"]
            .iter()
            .enumerate()
            .map(|(i, t)| (i + 1, t.to_string()))
            .collect();
        let data = Preview {
            lines: Some(lines.clone()),
            first: 1,
            last: 7,
            ..Default::default()
        };
        assert_eq!(block(&data, &lines, 4), (4, 5));
        assert_eq!(best_line(&data, &["d".into()]), 5);
    }

    #[test]
    fn the_pdf_box_starts_at_the_matched_sentence() {
        let p = Passage {
            text: "Rent is due monthly. Tenant may keep one pet with consent.".into(),
            before: String::new(),
            after: String::new(),
        };
        assert_eq!(
            from_match(&p, "…keep one pet with").text,
            "…Tenant may keep one pet with consent."
        );
    }
}

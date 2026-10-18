//! What each kind of file looks like in the window, and the small text helpers rows, the preview and setup share:
//! counts, sizes, dates, the tag on a row, the query's words picked out of a snippet.

use omafind_core::{FileKind, Hit, Locator, Reason};

/// The filters Tab cycles through: a label, the kinds it keeps (none = everything), and how results are laid out.
pub struct Filter {
    pub label: &'static str,
    pub kinds: &'static [FileKind],
    pub grid: bool,
}

pub const FILTERS: [Filter; 5] = [
    Filter {
        label: "All",
        kinds: &[],
        grid: false,
    },
    Filter {
        label: "Photos",
        kinds: &[FileKind::Image, FileKind::Screenshot],
        grid: true,
    },
    Filter {
        label: "Documents",
        kinds: &[FileKind::Pdf, FileKind::Doc],
        grid: false,
    },
    Filter {
        label: "Media",
        kinds: &[FileKind::Audio, FileKind::Video],
        grid: false,
    },
    Filter {
        label: "Code",
        kinds: &[FileKind::Code],
        grid: false,
    },
];

/// Nerd Font glyphs, as the rest of Omarchy draws them.
pub mod glyphs {
    pub const SEARCH: &str = "\u{f002}";
    pub const SIMILAR: &str = "\u{f0c5}";
    pub const FOLDER: &str = "\u{f07b}";
    pub const GEAR: &str = "\u{f013}";
    pub const CHECK: &str = "\u{f00c}";
    pub const CHECK_ON: &str = "\u{f14a}";
    pub const CHECK_OFF: &str = "\u{f096}";
    pub const DOWNLOAD: &str = "\u{f019}";
    pub const SPINNER: &str = "\u{f110}";
    pub const LOCK: &str = "\u{f023}";
    pub const CLOCK: &str = "\u{f017}";
    pub const PHOTO: &str = "\u{f03e}";
    pub const PDF: &str = "\u{f1c1}";
    pub const MICROPHONE: &str = "\u{f130}";
}

pub fn glyph(kind: FileKind) -> &'static str {
    match kind {
        FileKind::Screenshot | FileKind::Image => "\u{f03e}",
        FileKind::Pdf => "\u{f1c1}",
        FileKind::Doc => "\u{f15c}",
        FileKind::Audio => "\u{f001}",
        FileKind::Video => "\u{f03d}",
        FileKind::Code => "\u{f121}",
    }
}

/// `1,204`
pub fn count(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    if n < 0 { format!("-{out}") } else { out }
}

/// How many results a filter chip stands for, from the per-kind counts of the unfiltered list.
pub fn filter_count(filter: &Filter, counts: &[(FileKind, usize)]) -> usize {
    counts
        .iter()
        .filter(|(kind, _)| filter.kinds.is_empty() || filter.kinds.contains(kind))
        .map(|(_, n)| n)
        .sum()
}

/// The locator as a number: a page, seconds, a line, a character offset.
pub fn locator_value(hit: &Hit) -> Option<f64> {
    hit.locator.map(Locator::value)
}

/// The tag on a row: why it matched and where.
pub fn tag(hit: &Hit) -> String {
    let at = hit.locator_label.as_str();
    // Code matches a stretch of lines; which list found it matters less than where it is (the design's "lines 88–121").
    if hit.kind == FileKind::Code
        && hit.reason != Reason::Name
        && let Some(Locator::Line(start)) = hit.locator
        && start > 0
    {
        let end = hit.locator_end.map(|e| e as u32).unwrap_or(start);
        return if end > start {
            format!("lines {start}–{end}")
        } else {
            format!("line {start}")
        };
    }
    match hit.reason {
        Reason::Picture => match hit.kind {
            FileKind::Video => format!("seen at {at}"),
            FileKind::Pdf => format!("page {} looks like it", at.replace("p. ", "")),
            _ => "by picture".into(),
        },
        Reason::Sound if !at.is_empty() => format!("said at {at}"),
        Reason::Sound => "by sound".into(),
        Reason::Name => "by name".into(),
        Reason::Words if !at.is_empty() && hit.kind == FileKind::Pdf => format!("words · {at}"),
        Reason::Words => "words".into(),
        Reason::Text if !at.is_empty() => format!("text · {at}"),
        Reason::Text => "text".into(),
    }
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// `2 min ago`, `today 14:05`, `yesterday`, `Mon`, `Mar 3`, `Mar 3, 2024`.
pub fn when(seconds: i64) -> String {
    when_at(seconds, jiff::Zoned::now())
}

fn when_at(seconds: i64, now: jiff::Zoned) -> String {
    if seconds <= 0 {
        return String::new();
    }
    let Ok(then) = jiff::Timestamp::from_second(seconds) else {
        return String::new();
    };
    let then = then.to_zoned(now.time_zone().clone());
    let ago = now.timestamp().as_second() - seconds;
    if ago < 60 {
        return "just now".into();
    }
    if ago < 3600 {
        return format!("{} min ago", ago / 60);
    }
    let days = (now.date() - then.date()).get_days();
    let hm = format!("{:02}:{:02}", then.hour(), then.minute());
    match days {
        0 => format!("today {hm}"),
        1 => "yesterday".into(),
        2..=6 => WEEKDAYS[then.weekday().to_monday_zero_offset() as usize].into(),
        _ => {
            let md = format!("{} {}", MONTHS[then.month() as usize - 1], then.day());
            if then.year() == now.year() {
                md
            } else {
                format!("{md}, {}", then.year())
            }
        }
    }
}

/// `412 KB`
pub fn bytes(n: u64) -> String {
    if n == 0 {
        return String::new();
    }
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if value >= 100.0 || unit == 0 {
        format!("{} {}", value.round(), UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// `about 1 h 50 min`
pub fn duration(seconds: f64) -> String {
    if seconds < 90.0 {
        return "under 2 min".into();
    }
    let minutes = (seconds / 60.0).round() as i64;
    if minutes < 60 {
        format!("about {minutes} min")
    } else {
        format!("about {} h {} min", minutes / 60, minutes % 60)
    }
}

/// A piece of text, and whether it is one of the query's words.
#[derive(Debug, Clone, PartialEq)]
pub struct Mark<'a> {
    pub text: &'a str,
    pub hit: bool,
}

/// `text` cut into pieces, each saying whether it is one of `words` (case-insensitive, as typed, half words too): what
/// a row or the preview paints in the accent.
pub fn marks<'a>(text: &'a str, words: &[String]) -> Vec<Mark<'a>> {
    let wanted: Vec<String> = words
        .iter()
        .filter(|w| w.chars().count() >= 2)
        .map(|w| w.to_lowercase())
        .collect();
    if text.is_empty() || wanted.is_empty() {
        return vec![Mark { text, hit: false }];
    }
    // Lowercasing can change byte lengths (rare letters); match on a lowered copy only when it keeps the offsets.
    let lower = text.to_lowercase();
    if lower.len() != text.len() {
        return vec![Mark { text, hit: false }];
    }
    let mut out = Vec::new();
    let mut at = 0;
    while at < text.len() {
        let mut next: Option<(usize, usize)> = None;
        for w in &wanted {
            if let Some(i) = lower[at..].find(w.as_str()).map(|i| i + at)
                && next.is_none_or(|(n, len)| i < n || (i == n && w.len() > len))
            {
                next = Some((i, w.len()));
            }
        }
        let Some((start, len)) = next else {
            out.push(Mark {
                text: &text[at..],
                hit: false,
            });
            break;
        };
        if start > at {
            out.push(Mark {
                text: &text[at..start],
                hit: false,
            });
        }
        out.push(Mark {
            text: &text[start..start + len],
            hit: true,
        });
        at = start + len;
    }
    out
}

/// `text` split after each space, the spaces kept: the pieces a wrapping line of marked words is built from.
pub fn words_of(text: &str) -> impl Iterator<Item = &str> {
    text.split_inclusive(' ')
}

/// One paragraph of plain text: line breaks and Markdown heading and list marks out.
pub fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.split('\n').enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if i > 0 && !out.is_empty() {
            out.push(' ');
        }
        out.push_str(strip_marks(line));
    }
    // Marks after a space inside the paragraph too ("… - item"), as the window's snippets carry them.
    let mut result = String::with_capacity(out.len());
    let mut words = out.split(' ').peekable();
    let mut first = true;
    while let Some(word) = words.next() {
        let is_mark = is_block_mark(word) && words.peek().is_some_and(|next| !next.is_empty());
        if is_mark {
            continue;
        }
        if !first {
            result.push(' ');
        }
        result.push_str(word);
        first = false;
    }
    result
}

fn is_block_mark(word: &str) -> bool {
    (!word.is_empty() && word.len() <= 6 && word.chars().all(|c| c == '#'))
        || matches!(word, "-" | "*" | "+")
        || (word.len() >= 2
            && word.ends_with('.')
            && word[..word.len() - 1].chars().all(|c| c.is_ascii_digit()))
}

fn strip_marks(line: &str) -> &str {
    match line.split_once(' ') {
        Some((first, rest)) if is_block_mark(first) && !rest.trim().is_empty() => rest.trim_start(),
        _ => line,
    }
}

/// Markdown-safe text, for TextView.
pub fn escape_markdown(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if "\\`*_{}[]()#+-.!|>~<".contains(ch) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// Whether the kind has a picture: a photo, a screenshot, a video frame, a PDF page.
pub fn has_thumb(kind: FileKind) -> bool {
    matches!(
        kind,
        FileKind::Image | FileKind::Screenshot | FileKind::Video | FileKind::Pdf
    )
}

/// Pictures and previews are made per file and place in it.
pub fn thumb_key(hit: &Hit) -> String {
    match locator_value(hit) {
        Some(v) => format!("{}#{v}", hit.path),
        None => format!("{}#", hit.path),
    }
}

/// The bar's main button, what Enter does. Pages and lines go unsaid: opening at the match is simply what Open does.
/// A recording or video says where it starts playing, since a moment isn't visible otherwise.
pub fn primary_label(hit: &Hit) -> String {
    if matches!(hit.kind, FileKind::Video | FileKind::Audio) && hit.locator.is_some() {
        return format!("Play from {}", hit.locator_label);
    }
    "Open".into()
}

/// The folder a hit is in, home shortened.
pub fn folder(hit: &Hit) -> String {
    std::path::Path::new(&hit.path)
        .parent()
        .map(omafind_core::paths::tilde)
        .unwrap_or_default()
}

/// A typed filter as people read it: `\u{f07b} ~/Documents`, `Documents`, `\u{f017} since 2025`, `\u{f017} last week`.
pub fn chip_label(text: &str) -> String {
    let Some((key, value)) = text.split_once(':') else {
        return format!("{} {text}", glyphs::CLOCK);
    };
    let key = key.to_lowercase();
    match key.as_str() {
        "in" => {
            let home = std::env::var("HOME").unwrap_or_default();
            match value.strip_prefix(home.as_str()) {
                Some(rest) if !home.is_empty() => format!("{} ~{rest}", glyphs::FOLDER),
                _ => format!("{} {value}", glyphs::FOLDER),
            }
        }
        "type" | "kind" => kind_name(value).unwrap_or(value).to_owned(),
        _ => {
            let word = match key.as_str() {
                "after" | "since" => "since",
                other => other,
            };
            format!("{} {word} {value}", glyphs::CLOCK)
        }
    }
}

fn kind_name(value: &str) -> Option<&'static str> {
    Some(match value.to_lowercase().as_str() {
        "photo" | "photos" | "image" | "images" => "Photos",
        "screenshot" | "screenshots" => "Screenshots",
        "pdf" => "PDFs",
        "doc" | "docs" | "document" | "documents" => "Documents",
        "audio" => "Audio",
        "recording" | "recordings" => "Recordings",
        "video" | "videos" => "Videos",
        "media" => "Media",
        "code" => "Code",
        _ => return None,
    })
}

/// The kind's plural for the indexing screen.
pub fn kind_title(kind: FileKind) -> &'static str {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_group_thousands() {
        assert_eq!(count(0), "0");
        assert_eq!(count(999), "999");
        assert_eq!(count(1204), "1,204");
        assert_eq!(count(41203), "41,203");
        assert_eq!(count(1_000_000), "1,000,000");
    }

    #[test]
    fn sizes_read_like_the_scripts() {
        assert_eq!(bytes(0), "");
        assert_eq!(bytes(391), "391 B");
        assert_eq!(bytes(51_300), "51.3 KB");
        assert_eq!(bytes(412_000), "412 KB");
        assert_eq!(bytes(5_200_000), "5.2 MB");
    }

    #[test]
    fn durations_round_to_minutes() {
        assert_eq!(duration(30.0), "under 2 min");
        assert_eq!(duration(600.0), "about 10 min");
        assert_eq!(duration(6600.0), "about 1 h 50 min");
    }

    #[test]
    fn dates_are_relative_then_absolute() {
        let now: jiff::Zoned = "2026-10-16T12:00:00[UTC]".parse().unwrap();
        let at = |s: &str| s.parse::<jiff::Timestamp>().unwrap().as_second();
        assert_eq!(when_at(at("2026-10-16T11:59:30Z"), now.clone()), "just now");
        assert_eq!(
            when_at(at("2026-10-16T11:30:00Z"), now.clone()),
            "30 min ago"
        );
        assert_eq!(
            when_at(at("2026-10-16T09:05:00Z"), now.clone()),
            "today 09:05"
        );
        assert_eq!(
            when_at(at("2026-10-15T09:05:00Z"), now.clone()),
            "yesterday"
        );
        assert_eq!(when_at(at("2026-10-12T09:05:00Z"), now.clone()), "Mon");
        assert_eq!(when_at(at("2026-03-03T09:05:00Z"), now.clone()), "Mar 3");
        assert_eq!(when_at(at("2024-03-03T09:05:00Z"), now), "Mar 3, 2024");
    }

    #[test]
    fn marks_pick_out_words_longest_first() {
        let words = vec!["lease".to_string(), "pet".to_string(), "pets".to_string()];
        let m = marks("Pets in the Lease.", &words);
        assert_eq!(
            m,
            vec![
                Mark {
                    text: "Pets",
                    hit: true
                },
                Mark {
                    text: " in the ",
                    hit: false
                },
                Mark {
                    text: "Lease",
                    hit: true
                },
                Mark {
                    text: ".",
                    hit: false
                },
            ]
        );
        assert_eq!(
            marks("x", &["a".into()]),
            vec![Mark {
                text: "x",
                hit: false
            }]
        );
    }

    #[test]
    fn plain_drops_markdown_marks_and_breaks() {
        assert_eq!(plain("# October\n\n- one\n- two"), "October one two");
        assert_eq!(plain("1. first\n2. second"), "first second");
        assert_eq!(plain("a - b"), "a b");
    }

    #[test]
    fn chips_read_as_people_say_them() {
        assert_eq!(chip_label("type:pdf"), "PDFs");
        assert_eq!(chip_label("after:2025"), "\u{f017} since 2025");
        assert_eq!(chip_label("last week"), "\u{f017} last week");
    }

    #[test]
    fn markdown_is_escaped() {
        assert_eq!(escape_markdown("a*b_c."), "a\\*b\\_c\\.");
    }
}

//! Completing filters as they are typed, as the "Typing filters" board draws it: the word at the caret offers the
//! filters it could become (`af` → `after:2026`, `after:last-month`…; `in:Doc` → the indexed folders that match;
//! `ty` → the kinds), Tab takes the selected one, and the field turns it into a chip. Only what the query parser reads
//! is offered.

use std::ops::Range;

use jiff::civil::Date;

/// One way to complete the word.
#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    /// What replaces the word: `after:last-month`.
    pub text: String,
    /// What it means, at the row's end: `since last month`.
    pub note: String,
}

/// The open list: the text it would replace, its suggestions, the highlighted one.
#[derive(Debug, Clone, PartialEq)]
pub struct Completion {
    pub range: Range<usize>,
    pub items: Vec<Suggestion>,
    pub selected: usize,
}

const KEYS: [&str; 6] = ["in", "type", "after", "since", "before", "on"];

const KINDS: [(&str, &str); 8] = [
    ("photos", "Photos"),
    ("screenshots", "Screenshots"),
    ("pdf", "PDF"),
    ("documents", "Documents"),
    ("audio", "Audio"),
    ("video", "Videos"),
    ("media", "Audio and video"),
    ("code", "Code"),
];

const PHRASES: [&str; 8] = [
    "today",
    "yesterday",
    "this week",
    "last week",
    "this month",
    "last month",
    "this year",
    "last year",
];

/// Most suggestions a list shows.
const MAX: usize = 6;

/// The list for the word that ends at `cursor` in `text`, or `None`. `taken` are the chips' ranges: a word inside or
/// against one is not completed. `folders` are the indexed folders and their subfolders, as `~/…`.
pub fn complete(
    text: &str,
    cursor: usize,
    taken: &[Range<usize>],
    folders: &[String],
    today: Date,
) -> Option<Completion> {
    if cursor > text.len() || !text.is_char_boundary(cursor) {
        return None;
    }
    // Only at the end of a word: completing in the middle of one would split it.
    if text[cursor..].starts_with(|c: char| !c.is_whitespace()) {
        return None;
    }
    let start = text[..cursor]
        .rfind(char::is_whitespace)
        .map_or(0, |i| i + 1);
    let word = &text[start..cursor];
    if word.is_empty() || taken.iter().any(|r| r.start < cursor && start < r.end) {
        return None;
    }
    // "last w" completes both words to "last week".
    let before = text[..start].trim_end();
    let previous = before.rsplit(char::is_whitespace).next().unwrap_or("");
    if matches!(previous.to_lowercase().as_str(), "this" | "last") && !word.contains(':') {
        let phrase_start = start - (text[..start].len() - before.len()) - previous.len();
        let typed = format!("{} {}", previous.to_lowercase(), word.to_lowercase());
        let items: Vec<Suggestion> = PHRASES
            .iter()
            .filter(|p| p.starts_with(&typed) && **p != typed)
            .map(|p| phrase(p))
            .collect();
        if !items.is_empty() {
            return Some(Completion {
                range: phrase_start..cursor,
                items,
                selected: 0,
            });
        }
    }
    let items = suggestions(word, folders, today);
    (!items.is_empty()).then_some(Completion {
        range: start..cursor,
        items,
        selected: 0,
    })
}

/// What `word` could become.
fn suggestions(word: &str, folders: &[String], today: Date) -> Vec<Suggestion> {
    let lower = word.to_lowercase();
    let mut out: Vec<Suggestion> = match lower.split_once(':') {
        Some((key, value)) => values(
            key,
            value,
            word.split_once(':').map_or("", |(_, v)| v),
            folders,
            today,
        ),
        // A key being typed, two letters at least: its values straight away, as the board shows for `af`. "in" and
        // "on" are everyday words, so those two wait for their colon.
        None if lower.len() >= 2 => {
            let mut out = Vec::new();
            for key in KEYS {
                if key.starts_with(&lower)
                    && !(matches!(key, "in" | "on") && lower.len() <= key.len())
                {
                    out.extend(values(key, "", "", folders, today));
                }
            }
            if lower.len() >= 3 {
                out.extend(
                    PHRASES
                        .iter()
                        .filter(|p| p.starts_with(&lower) && **p != lower)
                        .map(|p| phrase(p)),
                );
            }
            out
        }
        None => Vec::new(),
    };
    out.dedup_by(|a, b| a.text == b.text);
    out.truncate(MAX);
    out
}

/// The values a key takes that start with what was typed after its colon.
fn values(key: &str, value: &str, typed: &str, folders: &[String], today: Date) -> Vec<Suggestion> {
    match key {
        "in" => {
            let wanted = typed.trim_start_matches("~/").to_lowercase();
            folders
                .iter()
                .filter(|f| f.trim_start_matches("~/").to_lowercase().contains(&wanted))
                .map(|f| Suggestion {
                    text: format!("in:{f}"),
                    note: "folder".into(),
                })
                .collect()
        }
        "type" | "kind" => KINDS
            .iter()
            .filter(|(name, _)| name.starts_with(value))
            .map(|(name, label)| Suggestion {
                text: format!("type:{name}"),
                note: (*label).into(),
            })
            .collect(),
        "after" | "since" | "before" | "on" => {
            let key = if key == "since" { "after" } else { key };
            dates(today)
                .into_iter()
                .filter(|(v, _)| v.starts_with(value))
                .map(|(v, label)| Suggestion {
                    text: format!("{key}:{v}"),
                    note: match key {
                        "after" => format!("since {label}"),
                        "before" => format!("before {label}"),
                        _ => label,
                    },
                })
                .collect()
        }
        _ => Vec::new(),
    }
}

/// The periods a date filter offers, as the parser reads them, with what each is.
fn dates(today: Date) -> Vec<(String, String)> {
    let year = today.year();
    vec![
        (year.to_string(), "this year".into()),
        ("last-month".into(), "last month".into()),
        ("last-week".into(), "last week".into()),
        (format!("{year}-{:02}", today.month()), "this month".into()),
        ((year - 1).to_string(), "last year".into()),
        ("yesterday".into(), "yesterday".into()),
    ]
}

/// The roots and the folders under them, `depth` levels down, hidden ones left out (the crawler skips them too), as
/// `~/…`, sorted. Bounded: a home with thousands of folders offers the first ones.
pub fn folders_under(roots: &[std::path::PathBuf], depth: usize) -> Vec<String> {
    const MAX_FOLDERS: usize = 2000;
    let mut out = Vec::new();
    let mut level: Vec<std::path::PathBuf> = roots.to_vec();
    for _ in 0..=depth {
        let mut next = Vec::new();
        for dir in &level {
            if out.len() >= MAX_FOLDERS {
                break;
            }
            out.push(omafind_core::paths::tilde(dir));
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let hidden = entry.file_name().to_string_lossy().starts_with('.');
                if !hidden && entry.file_type().is_ok_and(|t| t.is_dir()) {
                    next.push(entry.path());
                }
            }
        }
        level = next;
    }
    out.sort();
    out.dedup();
    out
}

fn phrase(p: &str) -> Suggestion {
    Suggestion {
        text: p.to_owned(),
        note: "date".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn today() -> Date {
        Date::new(2026, 10, 17).unwrap()
    }

    fn at_end(text: &str) -> Option<Completion> {
        complete(
            text,
            text.len(),
            &[],
            &[
                "~/Documents".into(),
                "~/Documents/Health".into(),
                "~/Pictures".into(),
            ],
            today(),
        )
    }

    fn texts(c: &Completion) -> Vec<&str> {
        c.items.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn a_key_being_typed_offers_its_values() {
        let c = at_end("invoice af").unwrap();
        assert_eq!(c.range, 8..10);
        assert_eq!(
            &texts(&c)[..3],
            ["after:2026", "after:last-month", "after:last-week"]
        );
        assert_eq!(c.items[0].note, "since this year");
        let c = at_end("ty").unwrap();
        assert_eq!(texts(&c)[0], "type:photos");
    }

    #[test]
    fn values_narrow_as_they_are_typed() {
        assert_eq!(
            texts(&at_end("type:p").unwrap()),
            ["type:photos", "type:pdf"]
        );
        assert_eq!(
            texts(&at_end("in:health").unwrap()),
            ["in:~/Documents/Health"]
        );
        assert_eq!(
            texts(&at_end("before:last-m").unwrap()),
            ["before:last-month"]
        );
        assert!(at_end("type:zz").is_none());
    }

    #[test]
    fn everyday_words_are_left_alone() {
        assert!(at_end("photos in").is_none());
        assert!(at_end("cat on").is_none());
        assert!(at_end("a").is_none());
        assert!(at_end("beach ").is_none());
    }

    #[test]
    fn date_phrases_complete_across_two_words() {
        let c = at_end("standup last w").unwrap();
        assert_eq!(c.range, 8..14);
        assert_eq!(texts(&c), ["last week"]);
        assert_eq!(texts(&at_end("yest").unwrap()), ["yesterday"]);
    }

    #[test]
    fn chips_and_mid_word_carets_are_not_completed() {
        let text = "type:pdf af";
        let chip = |r: std::ops::Range<usize>| vec![r];
        assert!(complete(text, text.len(), &chip(0..9), &[], today()).is_some());
        assert!(complete("type:pdf", 8, &chip(0..8), &[], today()).is_none());
        assert!(complete("after", 2, &[], &[], today()).is_none());
    }
}

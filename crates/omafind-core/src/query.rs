//! What someone types, split into what to look for and what to look in: `invoice in:~/Documents type:pdf after:2025`
//! → text `invoice`, folder `~/Documents`, kind PDF, modified since 1 Jan 2025. Date words ("last week",
//! "yesterday") become date filters too, so they don't go to the model, which knows nothing about today.

use std::path::{Path, PathBuf};

use jiff::civil::Date;
use jiff::{ToSpan, Zoned};
use serde::{Deserialize, Serialize};

use crate::FileKind;

/// One filter as the window shows it: a chip with a label, removable as a whole.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chip {
    /// `in`, `type`, `after`, `before`, `when`, `code`.
    pub key: String,
    /// As the person reads it: `~/Documents`, `PDF`, `since Jan 2025`, `last week`.
    pub label: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Parsed {
    /// What is left to search for.
    pub text: String,
    pub kinds: Vec<FileKind>,
    pub folders: Vec<PathBuf>,
    /// Unix seconds, inclusive.
    pub after: Option<i64>,
    /// Unix seconds, exclusive.
    pub before: Option<i64>,
    /// `code:` or `type:code`: the code index.
    pub code: bool,
    pub chips: Vec<Chip>,
}

/// `type:` names and the kinds they keep.
pub fn kinds_named(name: &str) -> Option<Vec<FileKind>> {
    use FileKind::*;
    Some(match name.to_lowercase().as_str() {
        "photo" | "photos" | "image" | "images" | "picture" | "pictures" | "jpg" | "png" => {
            vec![Image, Screenshot]
        }
        "screenshot" | "screenshots" => vec![Screenshot],
        "pdf" | "pdfs" => vec![Pdf],
        "doc" | "docs" | "document" | "documents" | "text" => vec![Pdf, Doc],
        "audio" | "recording" | "recordings" | "sound" | "music" => vec![Audio],
        "video" | "videos" | "movie" | "movies" => vec![Video],
        "media" => vec![Audio, Video],
        "code" => vec![Code],
        _ => return None,
    })
}

fn kinds_label(kinds: &[FileKind]) -> String {
    use FileKind::*;
    match kinds {
        [Image, Screenshot] => "Photos".into(),
        [Screenshot] => "Screenshots".into(),
        [Pdf] => "PDF".into(),
        [Pdf, Doc] => "Documents".into(),
        [Audio] => "Audio".into(),
        [Video] => "Video".into(),
        [Audio, Video] => "Media".into(),
        [Code] => "Code".into(),
        other => other
            .iter()
            .map(|k| k.as_str())
            .collect::<Vec<_>>()
            .join(", "),
    }
}

fn midnight(date: Date, tz: &Zoned) -> Option<i64> {
    date.to_zoned(tz.time_zone().clone())
        .ok()
        .map(|z| z.timestamp().as_second())
}

/// `2025`, `2025-03`, `2025-03-14` → the first second of that period and of the next one.
fn period(value: &str, now: &Zoned) -> Option<(i64, i64, String)> {
    let parts: Vec<&str> = value.split('-').collect();
    let year: i16 = parts
        .first()?
        .parse()
        .ok()
        .filter(|y| (1970..=2200).contains(y))?;
    let (start, end, label) = match parts.len() {
        1 => (
            Date::new(year, 1, 1).ok()?,
            Date::new(year + 1, 1, 1).ok()?,
            format!("{year}"),
        ),
        2 => {
            let month: i8 = parts[1].parse().ok()?;
            let start = Date::new(year, month, 1).ok()?;
            (
                start,
                start.checked_add(1.month()).ok()?,
                start.strftime("%b %Y").to_string(),
            )
        }
        3 => {
            let start = Date::new(year, parts[1].parse().ok()?, parts[2].parse().ok()?).ok()?;
            (
                start,
                start.tomorrow().ok()?,
                start.strftime("%b %-d, %Y").to_string(),
            )
        }
        _ => return None,
    };
    Some((midnight(start, now)?, midnight(end, now)?, label))
}

/// Relative date words: (start, end, label).
fn relative(words: &[&str], now: &Zoned) -> Option<(i64, i64, String, usize)> {
    let today = now.date();
    let lower: Vec<String> = words.iter().take(2).map(|w| w.to_lowercase()).collect();
    let one = lower.first().map(String::as_str);
    let two = lower.get(1).map(String::as_str);
    let week_start = today
        .checked_sub((today.weekday().to_monday_zero_offset() as i64).days())
        .ok()?;
    let month_start = today.first_of_month();
    let year_start = Date::new(today.year(), 1, 1).ok()?;
    let (start, end, label, used) = match (one, two) {
        (Some("today"), _) => (today, today.tomorrow().ok()?, "today", 1),
        (Some("yesterday"), _) => (today.yesterday().ok()?, today, "yesterday", 1),
        (Some("this"), Some("week")) => (week_start, today.tomorrow().ok()?, "this week", 2),
        (Some("last"), Some("week")) => (
            week_start.checked_sub(7.days()).ok()?,
            week_start,
            "last week",
            2,
        ),
        (Some("this"), Some("month")) => (month_start, today.tomorrow().ok()?, "this month", 2),
        (Some("last"), Some("month")) => (
            month_start.checked_sub(1.month()).ok()?,
            month_start,
            "last month",
            2,
        ),
        (Some("this"), Some("year")) => (year_start, today.tomorrow().ok()?, "this year", 2),
        (Some("last"), Some("year")) => (
            Date::new(today.year() - 1, 1, 1).ok()?,
            year_start,
            "last year",
            2,
        ),
        _ => return None,
    };
    Some((
        midnight(start, now)?,
        midnight(end, now)?,
        label.to_owned(),
        used,
    ))
}

fn expand(folder: &str, home: &Path) -> PathBuf {
    if folder == "~" {
        return home.to_path_buf();
    }
    if let Some(rest) = folder.strip_prefix("~/") {
        return home.join(rest);
    }
    let path = PathBuf::from(folder);
    if path.is_absolute() {
        return path;
    }
    // `in:Pictures` means the folder of that name in the home directory.
    home.join(folder)
}

pub fn parse(input: &str) -> Parsed {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    parse_at(input, &Zoned::now(), &home)
}

pub fn parse_at(input: &str, now: &Zoned, home: &Path) -> Parsed {
    let mut out = Parsed::default();
    let mut rest = input.trim();
    if let Some(tail) = rest
        .get(..5)
        .filter(|p| p.eq_ignore_ascii_case("code:"))
        .map(|_| &rest[5..])
    {
        out.code = true;
        out.chips.push(Chip {
            key: "code".into(),
            label: "Code".into(),
        });
        rest = tail;
    }
    let words: Vec<&str> = rest.split_whitespace().collect();
    let mut text: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let word = words[i];
        if let Some((key, value)) = word
            .split_once(':')
            .filter(|(k, v)| !v.is_empty() && k.chars().all(|c| c.is_ascii_alphabetic()))
        {
            match key.to_lowercase().as_str() {
                "in" => {
                    let folder = expand(value, home);
                    let label = match folder.strip_prefix(home) {
                        Ok(rel) if rel.as_os_str().is_empty() => "~".to_owned(),
                        Ok(rel) => format!("~/{}", rel.display()),
                        Err(_) => folder.display().to_string(),
                    };
                    out.chips.push(Chip {
                        key: "in".into(),
                        label,
                    });
                    out.folders.push(folder);
                    i += 1;
                    continue;
                }
                "type" | "kind" => {
                    if let Some(kinds) = kinds_named(value) {
                        if kinds == [FileKind::Code] {
                            out.code = true;
                            out.chips.push(Chip {
                                key: "code".into(),
                                label: "Code".into(),
                            });
                        } else {
                            out.chips.push(Chip {
                                key: "type".into(),
                                label: kinds_label(&kinds),
                            });
                            for k in kinds {
                                if !out.kinds.contains(&k) {
                                    out.kinds.push(k);
                                }
                            }
                        }
                        i += 1;
                        continue;
                    }
                }
                "after" | "since" => {
                    if let Some((start, _, label)) = period(value, now) {
                        out.after = Some(start);
                        out.chips.push(Chip {
                            key: "after".into(),
                            label: format!("since {label}"),
                        });
                        i += 1;
                        continue;
                    }
                }
                "before" => {
                    if let Some((start, _, label)) = period(value, now) {
                        out.before = Some(start);
                        out.chips.push(Chip {
                            key: "before".into(),
                            label: format!("before {label}"),
                        });
                        i += 1;
                        continue;
                    }
                }
                "on" | "during" => {
                    if let Some((start, end, label)) = period(value, now) {
                        out.after = Some(start);
                        out.before = Some(end);
                        out.chips.push(Chip {
                            key: "when".into(),
                            label,
                        });
                        i += 1;
                        continue;
                    }
                }
                _ => {}
            }
        }
        if let Some((start, end, label, used)) = relative(&words[i..], now) {
            out.after = Some(start);
            out.before = Some(end);
            out.chips.push(Chip {
                key: "when".into(),
                label,
            });
            i += used;
            continue;
        }
        text.push(word);
        i += 1;
    }
    out.text = text.join(" ");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(input: &str) -> Parsed {
        let now: Zoned = "2026-10-15T10:00:00[UTC]".parse().unwrap();
        parse_at(input, &now, Path::new("/home/u"))
    }

    #[test]
    fn filters_come_out_of_the_text() {
        let p = at("invoice dentist in:~/Documents type:pdf after:2025");
        assert_eq!(p.text, "invoice dentist");
        assert_eq!(p.folders, vec![PathBuf::from("/home/u/Documents")]);
        assert_eq!(p.kinds, vec![FileKind::Pdf]);
        assert_eq!(p.after, Some(1735689600));
        assert_eq!(
            p.chips.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(),
            vec!["~/Documents", "PDF", "since 2025"]
        );
    }

    #[test]
    fn date_words_become_a_range() {
        let p = at("standup notes last week");
        assert_eq!(p.text, "standup notes");
        // Thursday 15 Oct 2026: last week is Mon 5 – Mon 12 Oct.
        assert_eq!(p.after, Some(1791158400));
        assert_eq!(p.before, Some(1791763200));
        let p = at("photos from yesterday");
        assert_eq!(p.text, "photos from");
    }

    #[test]
    fn code_and_unknown_keys() {
        let p = at("code: retry with backoff");
        assert!(p.code);
        assert_eq!(p.text, "retry with backoff");
        let p = at("error: DO_NOT_HONOR type:code");
        assert!(p.code);
        assert_eq!(p.text, "error: DO_NOT_HONOR");
        // A word with a colon that isn't a filter stays text.
        assert_eq!(at("10:30 meeting").text, "10:30 meeting");
    }
}

//! When a photo was taken, from its EXIF `DateTimeOriginal`: what "photos from last summer" means, where the file's
//! modification time only says when it was copied. The offset tag is honoured when the camera wrote one; otherwise
//! the time is read as local time, as a camera clock is.

use std::path::Path;

use exif::{In, Tag, Value};

/// Unix seconds, or None when the file has no readable capture time.
pub fn taken_at(path: &Path) -> Option<i64> {
    let file = std::fs::File::open(path).ok()?;
    let data = exif::Reader::new()
        .read_from_container(&mut std::io::BufReader::new(file))
        .ok()?;
    let text = |tag: Tag| -> Option<String> {
        let field = data.get_field(tag, In::PRIMARY)?;
        match &field.value {
            Value::Ascii(parts) => parts
                .first()
                .map(|b| String::from_utf8_lossy(b).trim().to_owned()),
            _ => None,
        }
    };
    let when = text(Tag::DateTimeOriginal).or_else(|| text(Tag::DateTime))?;
    // `2024:08:14 18:52:07`
    let iso = when.replacen(':', "-", 2).replacen(' ', "T", 1);
    let civil: jiff::civil::DateTime = iso.parse().ok()?;
    if civil.year() < 1990 {
        return None;
    }
    let zoned = match text(Tag::OffsetTimeOriginal).and_then(|o| parse_offset(&o)) {
        Some(offset) => civil.to_zoned(jiff::tz::TimeZone::fixed(offset)).ok()?,
        None => civil.to_zoned(jiff::tz::TimeZone::system()).ok()?,
    };
    Some(zoned.timestamp().as_second())
}

/// `+02:00`, `-0530`.
fn parse_offset(text: &str) -> Option<jiff::tz::Offset> {
    let sign = match text.chars().next()? {
        '+' => 1,
        '-' => -1,
        _ => return None,
    };
    let digits: String = text[1..].chars().filter(|c| c.is_ascii_digit()).collect();
    let (h, m) = (
        digits.get(0..2)?.parse::<i32>().ok()?,
        digits
            .get(2..4)
            .and_then(|m| m.parse::<i32>().ok())
            .unwrap_or(0),
    );
    jiff::tz::Offset::from_seconds(sign * (h * 3600 + m * 60)).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets() {
        assert_eq!(parse_offset("+01:00").map(|o| o.seconds()), Some(3600));
        assert_eq!(parse_offset("-0530").map(|o| o.seconds()), Some(-19800));
        assert_eq!(parse_offset("Z"), None);
    }

    /// `DIG_EXIF_SAMPLE=photo.jpg EXPECT=unix-seconds cargo test -p dig-extract exif` checks a real file.
    #[test]
    fn sample() {
        if let (Ok(path), Ok(expect)) = (std::env::var("DIG_EXIF_SAMPLE"), std::env::var("EXPECT"))
        {
            assert_eq!(taken_at(Path::new(&path)), expect.parse().ok());
        }
    }
}

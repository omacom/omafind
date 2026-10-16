//! The desktop portal's folder chooser (xdg-desktop-portal, the GTK backend on Omarchy): no toolkit of our own, and
//! the dialog follows the desktop's theme.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

const DESKTOP: &str = "org.freedesktop.portal.Desktop";

/// Asks for a folder; `None` when the dialog is cancelled. Blocks until it closes.
pub fn pick_folder(title: &str) -> Result<Option<PathBuf>> {
    let conn = zbus::blocking::Connection::session().context("no session bus")?;
    // The reply comes as a signal on a request object whose path we can predict from a token: subscribe before
    // calling, or a fast answer is missed.
    let token = format!(
        "omafind{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_micros())
            .unwrap_or_default()
    );
    let sender = conn
        .unique_name()
        .context("no bus name")?
        .trim_start_matches(':')
        .replace('.', "_");
    let handle = format!("/org/freedesktop/portal/desktop/request/{sender}/{token}");
    let request = zbus::blocking::Proxy::new(
        &conn,
        DESKTOP,
        handle.as_str(),
        "org.freedesktop.portal.Request",
    )?;
    let mut responses = request.receive_signal("Response")?;

    let mut home = std::env::var("HOME").unwrap_or_default().into_bytes();
    home.push(0);
    let mut options: HashMap<&str, Value> = HashMap::new();
    options.insert("handle_token", Value::from(token.as_str()));
    options.insert("directory", Value::from(true));
    options.insert("modal", Value::from(true));
    options.insert("current_folder", Value::from(home));
    let chooser = zbus::blocking::Proxy::new(
        &conn,
        DESKTOP,
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.FileChooser",
    )?;
    let _: OwnedObjectPath = chooser
        .call("OpenFile", &("", title, options))
        .context("the desktop portal has no file chooser")?;

    let message = responses.next().context("the portal went away")?;
    let (code, results): (u32, HashMap<String, OwnedValue>) = message.body().deserialize()?;
    if code != 0 {
        return Ok(None);
    }
    let uris: Vec<String> = match results.get("uris") {
        Some(value) => value.try_clone()?.try_into()?,
        None => Vec::new(),
    };
    Ok(uris.first().and_then(|uri| file_path(uri)))
}

/// `file:///home/me/My%20Notes` → `/home/me/My Notes`.
fn file_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Some(b) = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            out.push(b);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    use std::os::unix::ffi::OsStringExt;
    Some(PathBuf::from(std::ffi::OsString::from_vec(out)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_uris_decode_to_paths() {
        assert_eq!(
            file_path("file:///home/me/My%20Notes"),
            Some(PathBuf::from("/home/me/My Notes"))
        );
        assert_eq!(
            file_path("file:///home/me/caf%C3%A9"),
            Some(PathBuf::from("/home/me/café"))
        );
        assert_eq!(file_path("https://example.com"), None);
    }
}

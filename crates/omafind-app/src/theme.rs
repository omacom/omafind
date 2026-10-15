//! Omarchy's palette onto gpui-base's semantic tokens, live: `colors.toml` is read at start and whenever the theme
//! changes (the `current/theme` link is replaced by `omarchy-theme-set`, so the parent is watched), and the font is
//! whatever `fc-match monospace` says, as the rest of Omarchy does.

use std::path::PathBuf;

use anyhow::{Context, Result};
use gpui_kit::base::{Theme, ThemeAppearance};
use gpui_kit::{App, Hsla, Rgba, px};

#[derive(Debug, Clone, PartialEq)]
pub struct Palette {
    pub light: bool,
    pub background: Hsla,
    pub darker_background: Hsla,
    pub lighter_background: Hsla,
    pub foreground: Hsla,
    pub muted_foreground: Hsla,
    pub accent: Hsla,
    pub selection: Hsla,
    pub muted: Hsla,
    pub red: Hsla,
    pub yellow: Hsla,
    pub green: Hsla,
}

/// Colors the scripts need beyond gpui-base's roles: numbers and strings in code previews, as `#rrggbb`.
pub static SYNTAX: std::sync::Mutex<Option<(String, String)>> = std::sync::Mutex::new(None);

pub fn state_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/state/omarchy/current")
}

pub fn colors_path() -> PathBuf {
    state_dir().join("theme/colors.toml")
}

fn hex(value: &str) -> Option<Hsla> {
    let v = value.trim().trim_start_matches('#');
    let (r, g, b) = match v.len() {
        6 => (
            u8::from_str_radix(&v[0..2], 16).ok()?,
            u8::from_str_radix(&v[2..4], 16).ok()?,
            u8::from_str_radix(&v[4..6], 16).ok()?,
        ),
        3 => {
            let d = |i: usize| u8::from_str_radix(&v[i..i + 1].repeat(2), 16).ok();
            (d(0)?, d(1)?, d(2)?)
        }
        _ => return None,
    };
    Some(
        Rgba {
            r: r as f32 / 255.0,
            g: g as f32 / 255.0,
            b: b as f32 / 255.0,
            a: 1.0,
        }
        .into(),
    )
}

impl Palette {
    /// Reads `colors.toml` (canonical and legacy key names). Missing keys fall back along the ramp.
    pub fn parse(text: &str, light_marker: bool) -> Result<Self> {
        let table: toml::Table = text.parse().context("colors.toml")?;
        let get = |keys: &[&str]| {
            keys.iter()
                .find_map(|k| table.get(*k).and_then(|v| v.as_str()).and_then(hex))
        };
        let background = get(&["background", "bg"]).context("no background")?;
        let foreground = get(&["foreground", "fg"]).context("no foreground")?;
        let light = light_marker || table.get("mode").and_then(|v| v.as_str()) == Some("light");
        let accent = get(&["accent", "blue", "color4"]).unwrap_or(foreground);
        Ok(Self {
            light,
            background,
            darker_background: get(&[
                "darker_background",
                "darker_bg",
                "dark_background",
                "dark_bg",
            ])
            .unwrap_or(background),
            lighter_background: get(&["lighter_background", "lighter_bg"]).unwrap_or(background),
            foreground,
            muted_foreground: get(&["light_foreground", "light_fg", "dark_foreground", "dark_fg"])
                .unwrap_or(foreground),
            accent,
            selection: get(&["selection"]).unwrap_or(mix(background, foreground, 0.08)),
            muted: get(&["muted", "color8"]).unwrap_or(mix(background, foreground, 0.2)),
            red: get(&["red", "color1"]).unwrap_or(accent),
            yellow: get(&["yellow", "color3"]).unwrap_or(accent),
            green: get(&["green", "color2"]).unwrap_or(foreground),
        })
    }

    pub fn load() -> Result<Self> {
        let text = std::fs::read_to_string(colors_path())
            .with_context(|| format!("reading {}", colors_path().display()))?;
        Self::parse(&text, state_dir().join("theme/light.mode").exists())
    }

    /// The gpui-base tokens this palette maps onto. Same mapping as Omarchy's own menu: rows on `background`,
    /// text in `foreground`, the selected row `foreground` at 8 % with the title in `accent`, dividers in `muted`.
    pub fn apply(&self, font: &str, cx: &mut App) {
        let theme = Theme::global_mut(cx);
        theme.appearance = if self.light {
            ThemeAppearance::Light
        } else {
            ThemeAppearance::Dark
        };
        let c = &mut theme.tokens.colors;
        c.background = self.background;
        c.foreground = self.foreground;
        c.surface = self.lighter_background;
        c.surface_foreground = self.foreground;
        c.primary = self.accent;
        c.primary_foreground = self.background;
        c.secondary = self.lighter_background;
        c.secondary_foreground = self.foreground;
        c.muted = self.selection;
        c.muted_foreground = self.muted_foreground;
        c.accent = self.selected_row();
        c.accent_foreground = self.selected_title();
        c.destructive = self.red;
        c.destructive_foreground = self.background;
        c.border = self.muted;
        c.input = self.muted;
        c.ring = self.accent;
        c.selection = self.selection;
        *SYNTAX.lock().unwrap() = Some((to_hex(self.yellow), to_hex(self.green)));
        let t = &mut theme.tokens.typography;
        t.sans = font.to_owned().into();
        t.mono = font.to_owned().into();
        for (style, size) in [
            (&mut t.xs, 11.0),
            (&mut t.sm, 12.0),
            (&mut t.md, 13.0),
            (&mut t.lg, 15.0),
            (&mut t.xl, 18.0),
            (&mut t.mono_md, 13.0),
        ] {
            style.size = px(size);
            style.line_height = px(size * 1.4);
        }
        let r = &mut theme.tokens.radius;
        let rounding = hyprland_rounding() as f32;
        r.sm = px(rounding.min(4.0));
        r.md = px(rounding.min(8.0));
        r.lg = px(rounding);
    }
}

impl Palette {
    /// The selected row: the foreground at 8 % over the background, as Omarchy's menu draws it.
    pub fn selected_row(&self) -> Hsla {
        mix(self.background, self.foreground, 0.08)
    }

    /// The selected row's title: the accent, as in Omarchy's menu, unless it doesn't read on the row (a few
    /// themes pick a dim accent), then the foreground.
    pub fn selected_title(&self) -> Hsla {
        if contrast(self.accent, self.selected_row()) >= 3.0 {
            self.accent
        } else {
            self.foreground
        }
    }
}

fn to_hex(color: Hsla) -> String {
    let c: Rgba = color.into();
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", byte(c.r), byte(c.g), byte(c.b))
}

/// `a` towards `b` by `t`.
pub fn mix(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let (a, b): (Rgba, Rgba) = (a.into(), b.into());
    Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: 1.0,
    }
    .into()
}

/// Hyprland's `decoration:rounding`, which the Omarchy shell mirrors for its cards. 0 when unknown.
pub fn hyprland_rounding() -> u32 {
    crate::hypr::command("getoption decoration:rounding")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("int: "))
                .and_then(|v| v.trim().parse().ok())
        })
        .unwrap_or(0)
}

/// The system monospace family, as `omarchy-font-current` reads it.
pub fn font_family() -> String {
    std::process::Command::new("fc-match")
        .args(["monospace", "-f", "%{family}"])
        .output()
        .ok()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .split(',')
                .next()
                .unwrap_or("")
                .trim()
                .to_owned()
        })
        .filter(|f| !f.is_empty())
        .unwrap_or_else(|| "JetBrainsMono Nerd Font".into())
}

/// WCAG relative luminance.
fn luminance(color: Hsla) -> f32 {
    let c: Rgba = color.into();
    let lin = |v: f32| {
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
}

/// WCAG contrast ratio between two colors.
pub fn contrast(a: Hsla, b: Hsla) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// Watches the Omarchy state directory and calls `on_change` whenever the theme link or its colors change.
pub fn watch(on_change: impl Fn() + Send + 'static) -> Result<notify::RecommendedWatcher> {
    use notify::Watcher;
    let dir = state_dir();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if let Ok(event) = event {
            // Only writes: reading the file reports access events, and reacting to those would loop forever.
            if !matches!(
                event.kind,
                notify::EventKind::Modify(_)
                    | notify::EventKind::Create(_)
                    | notify::EventKind::Remove(_)
            ) {
                return;
            }
            let relevant = event.paths.iter().any(|p: &PathBuf| {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                name == "theme"
                    || name == "theme.name"
                    || name == "colors.toml"
                    || name == "light.mode"
            });
            if relevant {
                on_change();
            }
        }
    })?;
    watcher.watch(&dir, notify::RecursiveMode::NonRecursive)?;
    let theme_dir = dir.join("theme");
    if theme_dir.exists() {
        let _ = watcher.watch(&theme_dir, notify::RecursiveMode::NonRecursive);
    }
    Ok(watcher)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every theme Omarchy ships must stay readable: text on background and the selected title (accent) on the
    /// selected row. The Omarchy checkout is a sibling of this repo; without it the test is skipped.
    #[test]
    fn omarchy_themes_keep_contrast() {
        let themes = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../omarchy-quattro/themes");
        let Ok(entries) = std::fs::read_dir(&themes) else {
            eprintln!("no omarchy checkout at {}: skipped", themes.display());
            return;
        };
        let mut checked = 0;
        for entry in entries.flatten() {
            let file = entry.path().join("colors.toml");
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            let palette = Palette::parse(&text, entry.path().join("light.mode").exists()).unwrap();
            let text_contrast = contrast(palette.foreground, palette.background);
            let title_contrast = contrast(palette.selected_title(), palette.selected_row());
            let muted_contrast = contrast(palette.muted_foreground, palette.background);
            assert!(
                text_contrast >= 4.5,
                "{}: text {text_contrast:.2}",
                entry.file_name().to_string_lossy()
            );
            assert!(
                title_contrast >= 3.0,
                "{}: selected title {title_contrast:.2}",
                entry.file_name().to_string_lossy()
            );
            assert!(
                muted_contrast >= 2.5,
                "{}: muted text {muted_contrast:.2}",
                entry.file_name().to_string_lossy()
            );
            checked += 1;
        }
        assert!(checked > 5, "only {checked} themes checked");
    }

    #[test]
    fn hex_parsing_and_mixing() {
        let white = hex("#ffffff").unwrap();
        let black = hex("#000").unwrap();
        assert!(contrast(white, black) > 20.0);
        let grey: Rgba = mix(black, white, 0.5).into();
        assert!((grey.r - 0.5).abs() < 0.01);
    }
}

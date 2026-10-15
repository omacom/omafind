//! The terminal side of `omafind`'s interactive commands, on cliclack. Prompts only run on a terminal: with `--yes`,
//! or when stdin or stderr isn't one (a script, a pipe), each takes its default and says so.

use std::fmt::Display;
use std::io::IsTerminal;

use anyhow::Result;

/// Whether prompts can be shown.
pub fn interactive(yes: bool) -> bool {
    !yes && std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}

/// Esc or Ctrl+C in a prompt: end the frame and stop, leaving whatever was done so far.
fn stopped(error: std::io::Error, what: &str) -> anyhow::Error {
    if error.kind() == std::io::ErrorKind::Interrupted {
        let _ = cliclack::outro_cancel(format!("{what} stopped; nothing more was changed"));
        std::process::exit(130);
    }
    error.into()
}

/// Yes/no, defaulting to yes.
pub fn confirm(question: impl Display, yes: bool, what: &str) -> Result<bool> {
    if !interactive(yes) {
        cliclack::log::step(format!("{question} yes"))?;
        return Ok(true);
    }
    cliclack::confirm(question)
        .initial_value(true)
        .interact()
        .map_err(|e| stopped(e, what))
}

/// Several of `items` (value, label, hint), the `on` ones preselected. Without prompts: the preselected ones.
pub fn choose<T: Clone + Eq>(
    question: impl Display,
    items: &[(T, String, String)],
    on: Vec<T>,
    yes: bool,
    what: &str,
) -> Result<Vec<T>> {
    if !interactive(yes) {
        let picked: Vec<&str> = items
            .iter()
            .filter(|(v, _, _)| on.contains(v))
            .map(|(_, label, _)| label.as_str())
            .collect();
        let picked = if picked.is_empty() {
            "none".to_owned()
        } else {
            picked.join(", ")
        };
        cliclack::log::step(format!("{question} {picked}"))?;
        return Ok(on);
    }
    cliclack::multiselect(question)
        .items(items)
        .initial_values(on)
        .required(false)
        .max_rows(12)
        .interact()
        .map_err(|e| stopped(e, what))
}

// --- the look ----------------------------------------------------------------------------------------------------

use console::{Style, style};

/// cliclack's theme with the active bar in ANSI blue, which Omarchy's terminal themes map to the theme's accent, so
/// the frame follows the desktop's colors.
struct Omafind;

impl cliclack::Theme for Omafind {
    fn bar_color(&self, state: &cliclack::ThemeState) -> Style {
        match state {
            cliclack::ThemeState::Active => Style::new().blue(),
            cliclack::ThemeState::Cancel => Style::new().red(),
            cliclack::ThemeState::Submit => Style::new().bright().black(),
            cliclack::ThemeState::Error(_) => Style::new().yellow(),
        }
    }
}

pub fn init() {
    cliclack::set_theme(Omafind);
}

/// Whether a command's output can be framed: both streams on a terminal. Piped or scripted output stays plain.
pub fn framed() -> bool {
    std::io::stdout().is_terminal() && std::io::stderr().is_terminal()
}

/// `┌   omafind  doctor   checks the model, engine, tools and setup`
pub fn intro(command: &str, subtitle: impl Display) -> Result<()> {
    let subtitle = subtitle.to_string();
    let badge = style(" omafind ").black().on_blue().bold();
    let title = if subtitle.is_empty() {
        format!("{badge} {}", style(command).bold())
    } else {
        format!(
            "{badge} {}  {}",
            style(command).bold(),
            style(subtitle).dim()
        )
    };
    cliclack::intro(title)?;
    Ok(())
}

/// A line inside the frame, without the blank line a log adds after itself.
pub fn line(text: impl Display) {
    eprintln!("{}  {text}", style("│").bright().black());
}

/// `✓` green or `✗` red.
pub fn mark(ok: bool) -> String {
    if ok {
        style("✓").green().to_string()
    } else {
        style("✗").red().to_string()
    }
}

pub fn dim(text: impl Display) -> String {
    style(text).dim().to_string()
}

pub fn bold(text: impl Display) -> String {
    style(text).bold().to_string()
}

/// A short pause between lines that appear one after another, so a report reads as it arrives rather than landing
/// in one block. Nothing when the output isn't a terminal.
pub fn beat(ms: u64) {
    if std::io::stderr().is_terminal() {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
}

/// Runs `work` under a spinner saying `doing`, shown at least `min_ms` on a terminal so a fast step still registers,
/// and ends the spinner with what `done` says about the result.
pub fn step<T>(
    doing: &str,
    min_ms: u64,
    work: impl FnOnce() -> T,
    done: impl FnOnce(&T) -> String,
) -> T {
    let spinner = cliclack::spinner();
    spinner.start(doing);
    let started = std::time::Instant::now();
    let result = work();
    if std::io::stderr().is_terminal() {
        let left = std::time::Duration::from_millis(min_ms).saturating_sub(started.elapsed());
        std::thread::sleep(left);
    }
    spinner.stop(done(&result));
    result
}

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

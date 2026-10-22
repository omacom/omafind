//! What the search screen shows without results — recent searches and files, the first indexing pass, a message — and
//! the footer that says what the indexer is doing.

use gpui_kit::base::{StyledExt as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, div, prelude::FluentBuilder as _, px,
};
use omafind_core::Hit;

use crate::format::{self, FILTERS, glyphs};
use crate::search::{Mode, SearchView};
use crate::state::AppState;
use crate::widgets::{clickable, colors};

/// What to try on an empty history.
const EXAMPLES: [&str; 4] = [
    "a dog on the beach",
    "the clause about pets in the lease",
    "where they talk about sleep",
    "invoice type:pdf after:2025",
];

impl SearchView {
    pub(crate) fn render_empty(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = colors(cx);
        let s = cx.global::<AppState>();
        if !s.daemon {
            return message(
                "Omafind isn't running",
                "systemctl --user start omafindd",
                cx,
            );
        }
        if s.roots().is_empty() {
            return message("Nothing indexed yet", "Ctrl+, picks the folders", cx);
        }
        if let Some(error) = &self.error {
            return message("Search failed", error, cx);
        }
        if !self.searched.is_empty() {
            let hint = if FILTERS[self.filter].kinds.is_empty() {
                "Try other words: what it shows, what it says, when it was"
            } else {
                "Tab widens the filter"
            };
            return message("Nothing matched", hint, cx);
        }
        // The first pass: while the folders are still being looked through, or most files are still unread.
        let first_look = s.indexed() == 0
            && s.status
                .as_ref()
                .is_some_and(|st| matches!(st.state.as_str(), "crawling" | "starting"));
        if first_look || (s.pending() > 0 && (s.indexed() as f64) < s.files() as f64 * 0.6) {
            return indexing(s, cx);
        }
        let label = |t: &str| {
            div()
                .text_size(px(11.))
                .text_color(c.muted_foreground)
                .pb(px(6.))
                .child(t.to_owned())
        };
        let searches: Vec<(String, String)> = match &self.recents {
            Some(r) if !r.searches.is_empty() => r
                .searches
                .iter()
                .take(7)
                .map(|(q, at)| (q.clone(), format::when(*at)))
                .collect(),
            _ => EXAMPLES
                .iter()
                .map(|q| (q.to_string(), String::new()))
                .collect(),
        };
        let has_history = self
            .recents
            .as_ref()
            .is_some_and(|r| !r.searches.is_empty());
        let added: Vec<Hit> = self
            .recents
            .iter()
            .flat_map(|r| r.added.iter().take(5).cloned())
            .collect();
        let opened: Vec<Hit> = self
            .recents
            .iter()
            .flat_map(|r| r.opened.iter().take(5).cloned())
            .collect();
        h_flex()
            .flex_1()
            .min_h_0()
            .items_start()
            .child(
                v_flex()
                    .w(px(380.))
                    .flex_none()
                    .h_full()
                    .px(px(12.))
                    .py(px(16.))
                    .gap(px(2.))
                    .border_r_1()
                    .border_color(c.border)
                    .child(label(if has_history {
                        "RECENT SEARCHES · ↑ recalls"
                    } else {
                        "TRY"
                    }))
                    .children(searches.into_iter().enumerate().map(|(i, (query, at))| {
                        let current = self.history_at == Some(i);
                        let recalled = query.clone();
                        clickable(format!("recent-{i}"))
                            .flex()
                            .justify_between()
                            .px(px(8.))
                            .py(px(8.))
                            .rounded(px(5.))
                            .bg(if current { c.accent } else { c.background })
                            .hover(move |st| st.bg(c.accent))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.recall(recalled.clone(), window, cx)
                            }))
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .text_color(if current {
                                        c.accent_foreground
                                    } else {
                                        c.foreground
                                    })
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(query),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(c.muted_foreground)
                                    .child(at),
                            )
                    })),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .px(px(18.))
                    .py(px(16.))
                    .gap(px(18.))
                    .child(
                        v_flex().gap(px(4.)).child(label("JUST ADDED")).child(
                            h_flex()
                                .gap(px(10.))
                                .children(added.into_iter().enumerate().map(|(i, hit)| {
                                    let picture = self.picture(&hit, 104., 104., cx);
                                    let (name, at) = (hit.name.clone(), format::when(hit.mtime));
                                    v_flex()
                                        .id(format!("added-{i}"))
                                        .w(px(104.))
                                        .gap(px(5.))
                                        .on_click(cx.listener(move |this, _, _, _| {
                                            this.open_hit(&hit, hit.locator);
                                        }))
                                        .child(picture)
                                        .child(
                                            div()
                                                .text_size(px(11.))
                                                .text_color(c.foreground)
                                                .overflow_hidden()
                                                .whitespace_nowrap()
                                                .text_ellipsis()
                                                .child(name),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(10.))
                                                .text_color(c.muted_foreground)
                                                .child(at),
                                        )
                                })),
                        ),
                    )
                    .when(!opened.is_empty(), |d| {
                        d.child(v_flex().gap(px(2.)).child(label("OPENED OFTEN")).children(
                            opened.into_iter().enumerate().map(|(i, hit)| {
                                let (name, folder) = (hit.name.clone(), format::folder(&hit));
                                clickable(format!("opened-{i}"))
                                    .flex()
                                    .justify_between()
                                    .py(px(7.))
                                    .border_b_1()
                                    .border_color(c.muted)
                                    .on_click(cx.listener(move |this, _, _, _| {
                                        this.open_hit(&hit, hit.locator);
                                    }))
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .text_color(c.foreground)
                                            .child(name),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(c.muted_foreground)
                                            .child(folder),
                                    )
                            }),
                        ))
                    })
                    .child(div().flex_1())
                    .child(
                        h_flex()
                            .gap(px(6.))
                            .text_size(px(11.))
                            .text_color(c.muted_foreground)
                            .child(div().child("Narrow it while you type:"))
                            .children(
                                [
                                    "in:~/Pictures",
                                    "type:pdf",
                                    "after:2024",
                                    "last week",
                                    "code:",
                                ]
                                .map(|t| div().text_color(c.foreground).child(t)),
                            ),
                    ),
            )
            .into_any_element()
    }

    pub(crate) fn render_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = colors(cx);
        let s = cx.global::<AppState>();
        let status = s.status.clone().unwrap_or_default();
        let indexed = format::count(status.indexed);
        let mut left = if !s.daemon {
            "omafindd isn't running".to_owned()
        } else {
            match status.state.as_str() {
                "paused" => format!("Paused · {indexed} files"),
                "paused_power_saver" => format!("Paused in power-saver · {indexed} files"),
                "waiting_for_ac" => format!("Waiting for a charger · {indexed} files"),
                "stuck" => format!(
                    "Not indexing: {}",
                    status.problem.as_deref().unwrap_or("see omafind doctor")
                ),
                _ if self.like.is_some() => format!(
                    "{} like {}",
                    format::count(self.results.len() as i64),
                    self.like.as_ref().map_or("", |h| h.name.as_str())
                ),
                _ if !self.searched.is_empty() && self.took_ms.is_some() => {
                    let n = self.results.len();
                    let pending = if status.pending > 0 {
                        format!(" · indexing, {} left", format::count(status.pending))
                    } else {
                        String::new()
                    };
                    format!(
                        "{} {}{pending}",
                        format::count(n as i64),
                        if n == 1 { "result" } else { "results" }
                    )
                }
                "indexing" if status.pending > 0 => {
                    let current = status
                        .current
                        .as_deref()
                        .map(|c| format!(" · {}", c.rsplit('/').next().unwrap_or(c)))
                        .unwrap_or_default();
                    let eta = status
                        .eta_seconds
                        .map(|e| format!(" · {}", format::duration(e)))
                        .unwrap_or_default();
                    format!(
                        "Indexing {} left{current}{eta}",
                        format::count(status.pending)
                    )
                }
                "crawling" if status.files > 0 => format!(
                    "Looking for new files · {} found",
                    format::count(status.files)
                ),
                "crawling" => "Looking for new files".into(),
                "starting" => "Starting: watching your folders".into(),
                _ => format!("{indexed} files · up to date"),
            }
        };
        let hints = if self.mode == Mode::Setup {
            let setup = self.setup.read(cx);
            if let Some(note) = setup.footer_note(self.first_run, cx) {
                left = note;
            }
            setup.hints(self.first_run)
        } else if self.completion.is_some() {
            left = "Filters become chips · ⌫ on an empty word removes the last chip".to_owned();
            "⇥ complete · esc close list".to_owned()
        } else if self.progress_open {
            "esc back".to_owned()
        } else if self.menu.is_some() {
            "↑↓ move · ⏎ run · esc back".to_owned()
        } else if let Some(hit) = self.results.get(self.selected) {
            format!(
                "{} move · ⏎ {} · ⌃S similar · ⌃K actions · esc close",
                if self.is_grid() {
                    "←↑↓→"
                } else {
                    "↑↓"
                },
                format::primary_label(hit).to_lowercase()
            )
        } else {
            "↑ last search · ⇥ filter · ⌃, settings · esc close".to_owned()
        };
        h_flex()
            .w_full()
            .h(px(32.))
            .flex_none()
            .px(px(16.))
            .border_t_1()
            .border_color(c.border)
            .justify_between()
            .items_center()
            .text_size(px(11.))
            .text_color(c.muted_foreground)
            .child({
                // While files are being found or read, the status opens their progress; Esc closes it.
                let busy = s.daemon
                    && (status.pending > 0
                        || matches!(status.state.as_str(), "crawling" | "starting"));
                div()
                    .id("progress")
                    .when(busy, |d| {
                        d.hover(move |st| st.text_color(c.foreground))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.progress_open = !this.progress_open;
                                cx.notify();
                            }))
                    })
                    .child(left)
            })
            .child(
                h_flex().gap(px(12.)).child(div().child(hints)).child(
                    div()
                        .id("settings")
                        .text_size(px(12.))
                        .hover(move |st| st.text_color(c.foreground))
                        .on_click(cx.listener(|this, _, window, cx| {
                            if this.mode == Mode::Setup {
                                this.leave_setup(cx);
                            } else {
                                this.open_setup(false, window, cx);
                            }
                        }))
                        .child(glyphs::GEAR),
                ),
            )
    }
}

fn message(title: &str, hint: &str, cx: &App) -> AnyElement {
    let c = colors(cx);
    v_flex()
        .flex_1()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .child(
            div()
                .text_size(px(14.))
                .text_color(c.foreground)
                .child(title.to_owned()),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(c.muted_foreground)
                .child(hint.to_owned()),
        )
        .into_any_element()
}

/// How far indexing is: files found while the folders are looked through, then how far along each kind is read.
pub(crate) fn indexing(s: &AppState, cx: &App) -> AnyElement {
    let c = colors(cx);
    let status = s.status.clone().unwrap_or_default();
    let eta = status
        .eta_seconds
        .map(|e| format!(" · {} left", format::duration(e)))
        .unwrap_or_default();
    let looking = matches!(status.state.as_str(), "crawling" | "starting");
    let (title, line) = if looking && status.pending == status.files {
        (
            "Looking through your folders",
            format!(
                "{} files found so far. Reading starts when the look-through is done, newest first.",
                format::count(status.files)
            ),
        )
    } else if status.pending > 0 {
        (
            "Reading your files",
            format!(
                "{} of {}{eta}. The newest are read first, so search works already.",
                format::count(status.files - status.pending),
                format::count(status.files)
            ),
        )
    } else {
        (
            "Everything is read",
            format!("{} files, up to date.", format::count(status.files)),
        )
    };
    v_flex()
        .flex_1()
        .items_center()
        .justify_center()
        .child(
            v_flex()
                .w(px(560.))
                .gap(px(20.))
                .child(
                    v_flex()
                        .gap(px(6.))
                        .child(
                            div()
                                .text_size(px(18.))
                                .font_medium()
                                .text_color(c.foreground)
                                .child(title),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(c.muted_foreground)
                                .child(line),
                        ),
                )
                .child(v_flex().gap(px(10.)).children(status.kinds.iter().map(
                    |(kind, done, total)| {
                        let fraction = if *total > 0 {
                            *done as f32 / *total as f32
                        } else {
                            0.
                        };
                        h_flex()
                            .gap(px(12.))
                            .items_center()
                            .text_size(px(12.))
                            .child(
                                div()
                                    .w(px(110.))
                                    .text_color(c.foreground)
                                    .child(format::kind_title(*kind)),
                            )
                            .child(div().flex_1().child(crate::widgets::bar(
                                format!("indexed-{}", kind.as_str()),
                                fraction,
                                6.,
                                cx,
                            )))
                            .child(div().w(px(120.)).text_color(c.muted_foreground).child(
                                if done == total {
                                    format!("{} done", format::count(*total))
                                } else {
                                    format!("{} / {}", format::count(*done), format::count(*total))
                                },
                            ))
                    },
                )))
                .when(status.failed > 0, |d| {
                    d.child(
                        div()
                            .text_size(px(11.))
                            .text_color(c.destructive)
                            .child(format!(
                                "{} files couldn't be read · omafind status --failed",
                                format::count(status.failed)
                            )),
                    )
                }),
        )
        .into_any_element()
}

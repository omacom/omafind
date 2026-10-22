//! The search screen's top and left: the bar with the field and the selected result's two likeliest actions, the
//! filter chips, the result rows (or the photo grid), and the Ctrl+K menu over them.

use std::path::Path;
use std::rc::Rc;

use gpui_kit::base::{StyledExt as _, h_flex, v_flex, v_virtual_list};
use gpui_kit::component::input::{InlineTokenContext, Input};
use gpui_kit::{
    AnyElement, App, Context, Div, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Window, div, img, prelude::FluentBuilder as _,
    px, size,
};
use omafind_core::{FileKind, Hit, Reason};

use crate::format::{self, FILTERS, glyphs};
use crate::search::{ROW, SearchView, THUMB, THUMB_H, TILE, TILE_GAP};
use crate::state::AppState;
use crate::widgets::{BarButton, ButtonKind, alpha, clickable, colors, pill, radius};

impl SearchView {
    pub(crate) fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = colors(cx);
        let s = cx.global::<AppState>();
        let indexing = s.daemon && s.pending() > 0 && s.files() > 0;
        let done = if s.files() > 0 {
            (s.files() - s.pending()) as f32 / s.files() as f32
        } else {
            0.
        };
        let field: AnyElement = match &self.like {
            Some(like) => h_flex()
                .flex_1()
                .gap(px(8.))
                .items_center()
                .text_size(px(14.))
                .child(div().text_color(c.muted_foreground).child("Like"))
                .child(
                    div()
                        .text_color(c.foreground)
                        .font_medium()
                        .child(like.name.clone()),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(c.muted_foreground)
                        .child("esc back"),
                )
                .into_any_element(),
            None => Input::new(&self.query)
                .flex_1()
                .h(px(36.))
                .px(px(4.))
                .border_0()
                .focus_bordered(false)
                .bg(c.background)
                .text_size(px(15.))
                .token(chip_token)
                .on_token_click({
                    // A chip clicked comes out, as its × says; the search follows.
                    let (query, view) = (self.query.clone(), cx.entity().downgrade());
                    move |event, window, cx| {
                        query.update(cx, |q, cx| {
                            q.set_selected_range(event.range(), cx);
                            q.replace("", window, cx);
                        });
                        let _ = view.update(cx, |view, cx| view.refresh_after_edit(window, cx));
                    }
                })
                .into_any_element(),
        };
        let took = match self.took_ms {
            Some(ms) if !self.quick_shown => format!("{} ms", ms.round()),
            _ => String::new(),
        };
        v_flex()
            .w_full()
            .child(
                h_flex()
                    .w_full()
                    .h(px(48.))
                    .pl(px(16.))
                    .pr(px(9.))
                    .gap(px(10.))
                    .items_center()
                    .child(
                        div()
                            .text_size(px(15.))
                            .text_color(c.muted_foreground)
                            .child(if self.like.is_some() {
                                glyphs::SIMILAR
                            } else {
                                glyphs::SEARCH
                            }),
                    )
                    .child(field)
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(c.muted_foreground)
                            .child(took),
                    )
                    .child(self.render_bar_actions(cx)),
            )
            .child(if indexing {
                // How far the first pass is, as a thin line under the bar.
                div()
                    .w_full()
                    .h(px(2.))
                    .bg(c.border)
                    .child(crate::widgets::bar("indexing-line", done, 2., cx))
            } else {
                div().h(px(1.)).w_full().bg(c.border)
            })
    }

    /// The selected result's two likeliest actions, at the bar's right end: what Enter does, and the next thing people
    /// do with that kind of file (more like a photo or a video frame, the folder of a document).
    fn render_bar_actions(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(hit) = self
            .menu
            .is_none()
            .then(|| self.results.get(self.selected))
            .flatten()
        else {
            return div().into_any_element();
        };
        let c = colors(cx);
        let visual = matches!(
            hit.kind,
            FileKind::Image | FileKind::Screenshot | FileKind::Video
        );
        let second = if visual {
            BarButton::new("bar-similar", "Similar", "⌃S")
                .icon(glyphs::SIMILAR)
                .render(cx.listener(|this, _, _, cx| this.more_like_this(cx)), cx)
                .into_any_element()
        } else {
            let hit = hit.clone();
            BarButton::new("bar-reveal", "Show in Files", "⌃⏎")
                .icon(glyphs::FOLDER)
                .render(cx.listener(move |this, _, _, _| this.reveal(&hit)), cx)
                .into_any_element()
        };
        h_flex()
            .gap(px(6.))
            .items_center()
            .child(div().w(px(1.)).h(px(20.)).mx(px(2.)).bg(c.border))
            .child(second)
            .child(
                BarButton::new("bar-open", format::primary_label(hit), "⏎")
                    .kind(ButtonKind::Primary)
                    .render(cx.listener(|this, _, _, cx| this.open_selected(cx)), cx),
            )
            .into_any_element()
    }

    pub(crate) fn render_filters(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = colors(cx);
        let hint = if self.is_grid() {
            "grid · ⌃G list"
        } else if FILTERS[self.filter].kinds.contains(&FileKind::Image) {
            "list · ⌃G grid"
        } else {
            "⇥ next filter"
        };
        h_flex()
            .w_full()
            .h(px(38.))
            .px(px(14.))
            .gap(px(6.))
            .items_center()
            .border_b_1()
            .border_color(c.border)
            .children(FILTERS.iter().enumerate().map(|(i, filter)| {
                let active = i == self.filter;
                let n = (!self.searched.is_empty())
                    .then(|| {
                        self.counts
                            .as_deref()
                            .map(|counts| format::filter_count(filter, counts))
                    })
                    .flatten();
                let label = match n {
                    Some(n) if filter.kinds != [FileKind::Code] => {
                        format!("{} {}", filter.label, format::count(n as i64))
                    }
                    _ => filter.label.to_owned(),
                };
                clickable(format!("filter-{}", filter.label))
                    .h(px(24.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .rounded(px(4.))
                    .border_1()
                    .border_color(if active { c.primary } else { c.border })
                    .bg(if active {
                        alpha(c.primary, 0.12)
                    } else {
                        c.background
                    })
                    .text_size(px(12.))
                    .text_color(if active {
                        c.primary
                    } else if n == Some(0) {
                        c.muted_foreground
                    } else {
                        c.foreground
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.set_filter(i, cx)))
                    .child(label)
            }))
            .child(div().flex_1())
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(c.muted_foreground)
                    .child(hint),
            )
    }

    pub(crate) fn render_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        if self.is_grid() {
            let cols = self.cols.max(1);
            let rows = self.results.len().div_ceil(cols);
            let sizes = Rc::new(vec![size(px(self.list_w - 20.), px(TILE + TILE_GAP)); rows]);
            return div()
                .w(px(self.list_w))
                .flex_none()
                .h_full()
                .px(px(10.))
                .py(px(10.))
                .child(
                    v_virtual_list(view, "grid", sizes, move |this, range, _, cx| {
                        range
                            .map(|r| {
                                let end = ((r + 1) * cols).min(this.results.len());
                                h_flex()
                                    .gap(px(TILE_GAP))
                                    .h(px(TILE + TILE_GAP))
                                    .children((r * cols..end).map(|k| this.render_tile(k, cx)))
                            })
                            .collect()
                    })
                    .size_full()
                    .track_scroll(&self.scroll),
                );
        }
        let sizes = Rc::new(vec![
            size(px(self.list_w - 12.), px(ROW));
            self.results.len()
        ]);
        div()
            .w(px(self.list_w))
            .flex_none()
            .h_full()
            .px(px(6.))
            .py(px(6.))
            .child(
                v_virtual_list(view, "results", sizes, |this, range, _, cx| {
                    range.map(|i| this.render_row(i, cx)).collect()
                })
                .size_full()
                .track_scroll(&self.scroll),
            )
    }

    fn render_row(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let c = colors(cx);
        let hit = &self.results[index];
        let selected = index == self.selected;
        let words = &hit.matched_words;
        let caption = if !hit.snippet.is_empty() {
            format::plain(&hit.snippet)
        } else if hit.reason == Reason::Sound {
            match hit.locator_label.as_str() {
                "" => "Matched by sound".into(),
                at => format!("Matched by sound at {at}"),
            }
        } else {
            String::new()
        };
        let mut meta = vec![format::folder(hit), format::when(hit.mtime)];
        if matches!(hit.kind, FileKind::Doc | FileKind::Pdf) {
            meta.push(format::bytes(hit.size.max(0) as u64));
        }
        let meta = meta
            .into_iter()
            .filter(|m| !m.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
        let row_hover = if selected {
            c.accent
        } else {
            alpha(c.accent, 0.6)
        };
        clickable(format!("row-{}", hit.chunk_id))
            .flex()
            .w_full()
            .h(px(ROW - 2.))
            .px(px(10.))
            .gap(px(12.))
            .items_center()
            .rounded(radius(cx))
            .bg(if selected { c.accent } else { c.background })
            .hover(move |style| style.bg(row_hover))
            .on_click(cx.listener(move |this, _, _, cx| this.click_result(index, cx)))
            .child(self.picture(hit, THUMB, THUMB_H, cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(4.))
                    .child(
                        h_flex()
                            .gap(px(8.))
                            .items_center()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .when(selected, |d| d.font_semibold())
                                    .when(!selected, |d| d.font_medium())
                                    .text_color(if selected {
                                        c.accent_foreground
                                    } else {
                                        c.foreground
                                    })
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(hit.name.clone()),
                            )
                            .child(pill(format::tag(hit), selected, cx))
                            .when(!hit.duplicates.is_empty(), |d| {
                                d.child(pill(
                                    format!("+{} similar", hit.duplicates.len()),
                                    false,
                                    cx,
                                ))
                            }),
                    )
                    .child(if !hit.snippet.is_empty() {
                        snippet(
                            &caption,
                            words,
                            if selected {
                                c.foreground
                            } else {
                                c.muted_foreground
                            },
                            cx,
                        )
                        .into_any_element()
                    } else if !caption.is_empty() {
                        div()
                            .text_size(px(12.))
                            .text_color(c.muted_foreground)
                            .child(caption)
                            .into_any_element()
                    } else {
                        div().into_any_element()
                    })
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(c.muted_foreground)
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(meta),
                    ),
            )
            .into_any_element()
    }

    /// A grid tile: square, so rows line up for the eye and the arrow keys; the picture whole inside it, at its own
    /// shape.
    fn render_tile(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let c = colors(cx);
        let hit = &self.results[index];
        let selected = index == self.selected;
        let picture = self.thumbs.get(&format::thumb_key(hit)).cloned().flatten();
        clickable(format!("tile-{}", hit.chunk_id))
            .w(px(TILE))
            .h(px(TILE))
            .flex_none()
            .rounded(radius(cx))
            .overflow_hidden()
            .bg(c.muted)
            .border_2()
            .border_color(if selected { c.primary } else { c.background })
            .flex()
            .items_center()
            .justify_center()
            .relative()
            .on_click(cx.listener(move |this, _, _, cx| this.click_result(index, cx)))
            .child(match picture {
                Some(path) => self
                    .fitted(&path, hit, TILE - 4., TILE - 4.)
                    .into_any_element(),
                None => div()
                    .text_size(px(28.))
                    .text_color(c.muted_foreground)
                    .child(format::glyph(hit.kind))
                    .into_any_element(),
            })
            .when(
                hit.kind == FileKind::Video && !hit.locator_label.is_empty(),
                |d| {
                    d.child(
                        div()
                            .absolute()
                            .left(px(6.))
                            .bottom(px(6.))
                            .px(px(5.))
                            .py(px(1.))
                            .rounded(px(3.))
                            .bg(alpha(c.background, 0.75))
                            .text_size(px(10.))
                            .text_color(c.foreground)
                            .child(format!("▶ {}", hit.locator_label)),
                    )
                },
            )
            .when(!hit.duplicates.is_empty(), |d| {
                d.child(
                    div()
                        .absolute()
                        .top(px(6.))
                        .right(px(6.))
                        .px(px(6.))
                        .py(px(1.))
                        .rounded(px(3.))
                        .bg(alpha(c.background, 0.8))
                        .text_size(px(10.))
                        .text_color(c.foreground)
                        .child(format!("+{}", hit.duplicates.len())),
                )
            })
            .into_any_element()
    }

    /// A result's small picture in a `w`×`h` cell: its thumbnail, else its type the way a file card shows it.
    pub(crate) fn picture(&self, hit: &Hit, w: f32, h: f32, cx: &App) -> Div {
        let c = colors(cx);
        let cell = div()
            .w(px(w))
            .h(px(h))
            .flex_none()
            .rounded(px(3.))
            .overflow_hidden()
            .bg(c.muted)
            .flex()
            .items_center()
            .justify_center()
            .relative();
        // A moment in a recording or video sits on the picture, as on a video thumbnail.
        let badge =
            matches!(hit.kind, FileKind::Video | FileKind::Audio) && !hit.locator_label.is_empty();
        let badge = badge.then(|| {
            div()
                .absolute()
                .left(px(3.))
                .bottom(px(3.))
                .px(px(3.))
                .rounded(px(2.))
                .bg(alpha(c.background, 0.75))
                .text_size(px(9.))
                .text_color(c.foreground)
                .child(hit.locator_label.clone())
        });
        if let Some(path) = self.thumbs.get(&format::thumb_key(hit)).cloned().flatten() {
            return cell
                .bg(c.background)
                .child(self.fitted(&path, hit, w, h))
                .children(badge);
        }
        // Documents and code without a picture show their type, the way a file card would.
        if matches!(hit.kind, FileKind::Doc | FileKind::Code) {
            let ext = match hit.name.rsplit_once('.') {
                Some((_, ext)) => ext.to_uppercase().chars().take(4).collect(),
                None => "TXT".to_owned(),
            };
            return cell
                .bg(c.surface)
                .border_1()
                .border_color(c.border)
                .text_size(px(10.))
                .text_color(c.muted_foreground)
                .child(ext);
        }
        cell.text_size(px(w.min(h) * 0.4))
            .text_color(c.muted_foreground)
            .child(format::glyph(hit.kind))
            .children(badge)
    }

    /// The picture at `path` whole inside a `w`×`h` box at its own shape (contain): a page stands tall, a video frame
    /// lies wide. The box centres it.
    fn fitted(&self, path: &Path, hit: &Hit, w: f32, h: f32) -> impl IntoElement {
        let aspect = self
            .aspects
            .get(&format::thumb_key(hit))
            .copied()
            .unwrap_or(w / h);
        let (fw, fh) = if aspect > w / h {
            (w, (w / aspect).round())
        } else {
            ((h * aspect).round(), h)
        };
        img(path.to_path_buf()).w(px(fw)).h(px(fh)).rounded(px(3.))
    }

    /// The filters offered for the word at the caret, in a list under it.
    pub(crate) fn render_completion(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(completion) = &self.completion else {
            return div().into_any_element();
        };
        let c = colors(cx);
        // Under the word being typed: the caret, less the word, in the field's monospace.
        let input = self.query.read(cx);
        let caret = input
            .cursor_layout()
            .map(|(bounds, _)| f32::from(bounds.origin.x));
        let typed = input.value()[completion.range.clone()].chars().count() as f32;
        let left = caret.map_or(60., |x| (x - typed * 9.).max(12.));
        v_flex()
            .id("completion")
            .absolute()
            .top(px(46.))
            .left(px(left))
            .w(px(340.))
            .p(px(4.))
            .rounded(px(6.))
            .border_1()
            .border_color(c.border)
            .bg(c.surface)
            .shadow_lg()
            .children(completion.items.iter().enumerate().map(|(i, item)| {
                let selected = i == completion.selected;
                clickable(format!("suggestion-{i}"))
                    .flex()
                    .justify_between()
                    .px(px(10.))
                    .py(px(8.))
                    .rounded(px(4.))
                    .text_size(px(12.))
                    .when(selected, |d| d.bg(c.accent))
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.pick_completion(i, window, cx)),
                    )
                    .child(
                        div()
                            .text_color(if selected { c.primary } else { c.foreground })
                            .child(item.text.clone()),
                    )
                    .child(
                        div()
                            .text_color(c.muted_foreground)
                            .child(item.note.clone()),
                    )
            }))
            .child(
                div()
                    .mt(px(2.))
                    .px(px(10.))
                    .pt(px(6.))
                    .pb(px(4.))
                    .border_t_1()
                    .border_color(c.border)
                    .text_size(px(10.))
                    .text_color(c.muted_foreground)
                    .child("⇥ complete · words like \"last week\" work too"),
            )
            .into_any_element()
    }

    pub(crate) fn render_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(menu) = &self.menu else {
            return div().into_any_element();
        };
        let c = colors(cx);
        div()
            .id("menu-backdrop")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .justify_center()
            .items_start()
            .pt(px(110.))
            .bg(alpha(c.background, 0.6))
            .on_click(cx.listener(|this, _, _, cx| this.close_menu(cx)))
            .child(
                v_flex()
                    .id("menu")
                    .w(px(440.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(c.border)
                    .bg(c.surface)
                    .shadow_lg()
                    .overflow_hidden()
                    // Clicks inside the menu stay in it; the backdrop closes it.
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(
                        h_flex()
                            .gap(px(10.))
                            .px(px(14.))
                            .py(px(12.))
                            .items_center()
                            .border_b_1()
                            .border_color(c.border)
                            .child(self.picture(&menu.hit, 40., 40., cx))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap(px(2.))
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .font_medium()
                                            .text_color(c.foreground)
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .child(menu.hit.name.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(c.muted_foreground)
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .child(format::folder(&menu.hit)),
                                    ),
                            ),
                    )
                    .child(
                        v_flex()
                            .p(px(6.))
                            .children(menu.items.iter().enumerate().map(|(i, item)| {
                                let selected = i == menu.selected;
                                let label = if item.danger && selected && menu.confirm {
                                    "Press ⏎ again to move to trash".to_owned()
                                } else {
                                    item.label.clone()
                                };
                                clickable(format!("act-{i}"))
                                    .flex()
                                    .justify_between()
                                    .px(px(10.))
                                    .py(px(9.))
                                    .rounded(px(4.))
                                    .bg(if selected { c.accent } else { c.surface })
                                    .text_size(px(12.))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.select_menu_item(i, cx);
                                    }))
                                    .child(
                                        div()
                                            .text_color(if item.danger {
                                                c.destructive
                                            } else if selected {
                                                c.accent_foreground
                                            } else {
                                                c.foreground
                                            })
                                            .child(label),
                                    )
                                    .child(div().text_color(c.muted_foreground).child(item.keys))
                            })),
                    ),
            )
            .into_any_element()
    }
}

/// A filter typed into the field, shown as a chip inside it.
fn chip_token(token: &InlineTokenContext, _: &mut Window, cx: &mut App) -> AnyElement {
    let c = colors(cx);
    let label = token.token().label();
    let label = if label.is_empty() {
        token.token().text().clone()
    } else {
        label.clone()
    };
    h_flex()
        .h(px(24.))
        .px(px(8.))
        .gap(px(6.))
        .items_center()
        .rounded(px(4.))
        .bg(if token.is_selected() {
            c.primary
        } else {
            c.muted
        })
        .text_size(px(12.))
        .text_color(if token.is_selected() {
            c.primary_foreground
        } else {
            c.foreground
        })
        .child(label)
        .child(
            div()
                .text_color(if token.is_selected() {
                    c.primary_foreground
                } else {
                    c.muted_foreground
                })
                .child("×"),
        )
        .into_any_element()
}

/// The matched passage under a result's name: up to two lines, the query's words tinted like a highlighter.
fn snippet(text: &str, words: &[String], color: gpui_kit::Hsla, cx: &App) -> impl IntoElement {
    let c = colors(cx);
    let pieces: Vec<AnyElement> = format::marks(text, words)
        .into_iter()
        .flat_map(|m| {
            format::words_of(m.text)
                .filter(|p| !p.is_empty())
                .map(move |part| {
                    if m.hit {
                        div()
                            .flex_none()
                            .rounded(px(2.))
                            .bg(alpha(c.primary, 0.2))
                            .text_color(c.primary)
                            .child(part.to_owned())
                            .into_any_element()
                    } else {
                        div()
                            .flex_none()
                            .text_color(color)
                            .child(part.to_owned())
                            .into_any_element()
                    }
                })
                .collect::<Vec<_>>()
        })
        .collect();
    h_flex()
        .w_full()
        .h(px(38.))
        .overflow_hidden()
        .flex_wrap()
        .content_start()
        .text_size(px(12.))
        .children(pieces)
}

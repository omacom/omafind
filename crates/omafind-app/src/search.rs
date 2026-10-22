//! Omafind's search window: a field on top with the filters as chips, results on the left (rows, or a grid for
//! photos), a preview of the selected result on the right, a footer that says what the indexer is doing. Opened with an
//! empty field it shows recent searches and files instead; on a first run, setup.
//!
//! The view owns the search state and the keyboard; rows and tiles (`results.rs`), the preview (`preview.rs`) and the
//! empty screens (`home.rs`) are its render helpers. Everything that touches files, the index or processes goes
//! through `service` on the background executor.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::base::VirtualListScrollHandle;
use gpui_kit::component::input::{InlineToken, InputEvent, InputState};
use gpui_kit::{
    AppContext as _, Context, Entity, FocusHandle, InteractiveElement as _, IntoElement,
    KeyDownEvent, ParentElement as _, Render, ScrollStrategy, Styled as _, Subscription, Task,
    Window, div, prelude::FluentBuilder as _,
};
use omafind_client::{Passage, Recents, Reply, Request};
use omafind_core::{FileKind, Hit, Locator};

use crate::format::{self, FILTERS};
use crate::service;
use crate::settings::{Setup, SetupEvent};
use crate::state::AppState;
use crate::widgets::colors;

// Names and keywords answer in a few ms: shown almost at once. The model's answer follows when typing pauses.
const QUICK_MS: u64 = 30;
const FULL_MS: u64 = 110;
pub(crate) const ROW: f32 = 90.;
// Result thumbnails are page-shaped, like the design's: a PDF page, a document card, a frame.
pub(crate) const THUMB: f32 = 44.;
pub(crate) const THUMB_H: f32 = 56.;
pub(crate) const TILE: f32 = 140.;
pub(crate) const TILE_GAP: f32 = 8.;
/// Previews kept, most recent first: enough for a session of moving through results.
const PREVIEWS_KEPT: usize = 200;

gpui_kit::actions!(omafind, [NextFilter, PreviousFilter]);

/// The screen the window shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Search,
    /// The first run's onboarding, or the settings.
    Setup,
}

/// What the preview of one result needs, loaded once per file and place in it.
#[derive(Default)]
pub(crate) struct Preview {
    /// The large picture: a photo, a PDF page, a video frame.
    pub large: Option<PathBuf>,
    /// The matched passage in full, for documents and PDFs.
    pub passage: Option<Passage>,
    /// Numbered lines, for code and plain text files.
    pub lines: Option<Vec<(usize, String)>>,
    /// The matched stretch, as line numbers.
    pub first: usize,
    pub last: usize,
    /// The line of the stretch holding the most query words: where the preview tints hardest and Open goes.
    pub best: Option<usize>,
}

/// The Ctrl+K menu for one result.
pub(crate) struct Menu {
    pub hit: Hit,
    pub items: Vec<MenuItem>,
    pub selected: usize,
    /// "Move to trash" asks once more before it acts.
    pub confirm: bool,
}

pub(crate) struct MenuItem {
    pub label: String,
    pub keys: &'static str,
    pub danger: bool,
    pub action: MenuAction,
}

#[derive(Clone)]
pub(crate) enum MenuAction {
    Open,
    MoreLikeThis,
    Reveal,
    CopyFile,
    CopyPath,
    CopyText(String),
    Trash,
}

pub struct SearchView {
    pub(crate) query: Entity<InputState>,
    pub(crate) results: Vec<Hit>,
    pub(crate) selected: usize,
    /// Whether the selection was moved by hand since this query's first answer.
    moved: bool,
    pub(crate) filter: usize,
    pub(crate) counts: Option<Vec<(FileKind, usize)>>,
    pub(crate) took_ms: Option<f64>,
    /// The query the results on screen answer.
    pub(crate) searched: String,
    pub(crate) quick_shown: bool,
    /// Searches are numbered as they start; an answer older than the one on screen is dropped. The daemon supersedes
    /// a session's older searches too, but two filters' searches can still finish out of order.
    generation: u64,
    shown_generation: u64,
    pub(crate) error: Option<String>,
    /// The scheduled quick and full searches; replacing them cancels the old ones.
    timers: Vec<Task<()>>,
    pub(crate) thumbs: HashMap<String, Option<PathBuf>>,
    thumb_task: Option<Task<()>>,
    /// Thumbnail shape (width / height) by thumb key, read once from each picture's header.
    pub(crate) aspects: HashMap<String, f32>,
    pub(crate) previews: HashMap<String, Preview>,
    preview_order: VecDeque<String>,
    pub(crate) scroll: VirtualListScrollHandle,
    focus: FocusHandle,
    /// The `AppState::shown` this view last reset for.
    shown: u64,
    pub(crate) mode: Mode,
    pub(crate) first_run: bool,
    pub(crate) setup: Entity<Setup>,
    pub(crate) recents: Option<Recents>,
    pub(crate) history_at: Option<usize>,
    pub(crate) menu: Option<Menu>,
    /// The result "more like this" started from.
    pub(crate) like: Option<Hit>,
    /// The indexing progress, opened from the footer while files are being found or read.
    pub(crate) progress_open: bool,
    /// Filters offered for the word at the caret (the "Typing filters" board).
    pub(crate) completion: Option<crate::completion::Completion>,
    /// The field's text when Esc closed the list: it stays closed until the text changes.
    completion_closed_for: Option<String>,
    /// The indexed folders and their subfolders, as `~/…`, for `in:`.
    folder_choices: Vec<String>,
    /// The roots `folder_choices` was made from.
    folder_roots: Vec<std::path::PathBuf>,
    /// `None` follows the filter (photos are a grid); Ctrl+G overrides it.
    grid: Option<bool>,
    pub(crate) cols: usize,
    pub(crate) list_w: f32,
    /// Whether the first-run decision (show setup or not) was made.
    decided: bool,
    _first_run_check: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl SearchView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Describe it: what it shows, says, or when it was")
        });
        let setup = cx.new(Setup::new);
        let subscriptions = vec![
            cx.subscribe_in(&query, window, |this, _, event, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.changed(window, cx);
                }
            }),
            cx.subscribe_in(&setup, window, Self::on_setup_event),
            cx.observe_global_in::<AppState>(window, Self::on_state),
        ];
        let shown = cx.global::<AppState>().shown;
        let mut view = Self {
            query,
            results: Vec::new(),
            selected: 0,
            moved: false,
            filter: 0,
            counts: None,
            took_ms: None,
            searched: String::new(),
            quick_shown: false,
            generation: 0,
            shown_generation: 0,
            error: None,
            timers: Vec::new(),
            thumbs: HashMap::new(),
            thumb_task: None,
            aspects: HashMap::new(),
            previews: HashMap::new(),
            preview_order: VecDeque::new(),
            scroll: VirtualListScrollHandle::new(),
            focus: cx.focus_handle(),
            shown,
            mode: Mode::Search,
            first_run: false,
            setup,
            recents: None,
            history_at: None,
            menu: None,
            like: None,
            progress_open: false,
            completion: None,
            completion_closed_for: None,
            folder_choices: Vec::new(),
            folder_roots: Vec::new(),
            grid: None,
            cols: 4,
            list_w: 500.,
            decided: false,
            _first_run_check: None,
            _subscriptions: subscriptions,
        };
        view.focus_field(window, cx);
        view.load_recents(cx);
        // Without a daemon to ask, a missing config after a few seconds means a first run.
        view._first_run_check = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(3)).await;
            let _ = this.update_in(cx, |this, window, cx| {
                let s = cx.global::<AppState>();
                if !this.decided && !s.daemon && !s.configured {
                    this.decided = true;
                    this.open_setup(true, window, cx);
                }
            });
        }));
        view
    }

    // ---- modes -------------------------------------------------------------------------------------------------

    /// The daemon or the window changed: decide on a first run once the daemon answers, and start fresh each time the
    /// window is shown.
    fn on_state(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let s = cx.global::<AppState>();
        let (daemon, roots, configured, shown) = (s.daemon, s.roots().len(), s.configured, s.shown);
        if !self.decided && daemon {
            self.decided = true;
            if roots == 0 && !configured {
                self.open_setup(true, window, cx);
            }
        }
        self.load_folders(cx);
        if shown != self.shown {
            // Shown again: fresh field, keyboard on it, recents up to date.
            self.shown = shown;
            self.completion = None;
            self.progress_open = false;
            self.query.update(cx, |q, cx| q.set_value("", window, cx));
            self.results.clear();
            self.took_ms = None;
            self.searched.clear();
            self.counts = None;
            self.menu = None;
            self.like = None;
            self.history_at = None;
            self.timers.clear();
            self.focus_field(window, cx);
            self.load_recents(cx);
        }
        cx.notify();
    }

    pub(crate) fn open_setup(
        &mut self,
        first_run: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.mode = Mode::Setup;
        self.first_run = first_run;
        self.setup.update(cx, |setup, cx| setup.open(first_run, cx));
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// Esc, Back, the gear: out of settings, asking first when there are unsaved changes. A first run has nowhere to
    /// go back to: it hides the window, or closes it when it was opened before any setup (no window rules yet).
    pub(crate) fn leave_setup(&mut self, cx: &mut Context<Self>) {
        if self.first_run {
            crate::hide_window();
        } else {
            self.setup.update(cx, |setup, cx| setup.leave(cx));
        }
    }

    fn on_setup_event(
        &mut self,
        _: &Entity<Setup>,
        event: &SetupEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SetupEvent::Close => self.close_setup(window, cx),
            SetupEvent::Finished => {
                self.first_run = false;
                self.close_setup(window, cx);
            }
            SetupEvent::Later => crate::hide_window(),
        }
    }

    fn close_setup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.mode = Mode::Search;
        self.focus_field(window, cx);
        cx.notify();
    }

    /// The keyboard on the search field; setup screens have no field, so there the window's root keeps it.
    fn focus_field(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == Mode::Search {
            self.query.update(cx, |q, cx| q.focus(window, cx));
        } else {
            self.focus.focus(window, cx);
        }
    }

    /// The list pane follows the window, which Omafind sizes to the monitor: wider for the grid, never past half.
    fn layout(&mut self, window: &Window) {
        let width = f32::from(window.viewport_size().width);
        if self.is_grid() {
            self.cols = (((width * 0.55 - 20.) / (TILE + TILE_GAP)).floor() as usize).clamp(3, 6);
            self.list_w = self.cols as f32 * (TILE + TILE_GAP) + 20.;
        } else {
            self.cols = 1;
            self.list_w = (width * 0.44).clamp(420., 620.).round();
        }
    }

    pub(crate) fn is_grid(&self) -> bool {
        self.grid.unwrap_or(FILTERS[self.filter].grid)
    }

    // ---- searching ---------------------------------------------------------------------------------------------

    pub(crate) fn text(&self, cx: &gpui_kit::App) -> String {
        self.query.read(cx).value().trim().to_owned()
    }

    fn changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tokenize(window, cx);
        self.history_at = None;
        self.like = None;
        self.update_completion(cx);
        self.schedule(cx);
    }

    /// Offers the filters the word at the caret could become.
    fn update_completion(&mut self, cx: &mut Context<Self>) {
        let input = self.query.read(cx);
        let content = input.content();
        let text = content.text().to_string();
        if self.completion_closed_for.as_deref() == Some(text.as_str()) {
            self.completion = None;
            return;
        }
        self.completion_closed_for = None;
        let taken: Vec<std::ops::Range<usize>> =
            content.tokens().iter().map(|t| t.range()).collect();
        let today = jiff::Zoned::now().date();
        self.completion =
            crate::completion::complete(&text, input.cursor(), &taken, &self.folder_choices, today);
    }

    /// Takes the highlighted filter: it replaces the word, a space follows, and the field turns it into a chip.
    pub(crate) fn accept_completion(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(completion) = self.completion.take() else {
            return;
        };
        let text = format!("{} ", completion.items[completion.selected].text);
        self.query.update(cx, |q, cx| {
            q.set_selected_range(completion.range.clone(), cx);
            q.replace(text, window, cx);
        });
        self.changed(window, cx);
        cx.notify();
    }

    /// After the field was edited from code (a chip removed): what typing would have done.
    pub(crate) fn refresh_after_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.changed(window, cx);
        cx.notify();
    }

    pub(crate) fn pick_completion(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(completion) = self.completion.as_mut() {
            completion.selected = index;
        }
        self.accept_completion(window, cx);
    }

    /// The indexed folders and the folders in them, two levels down, for `in:`; made again when the roots change.
    fn load_folders(&mut self, cx: &mut Context<Self>) {
        let roots: Vec<std::path::PathBuf> = cx
            .global::<AppState>()
            .roots()
            .iter()
            .filter(|r| !r.code)
            .map(|r| r.path.clone())
            .collect();
        if roots == self.folder_roots {
            return;
        }
        self.folder_roots = roots.clone();
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_spawn(async move { crate::completion::folders_under(&roots, 2) })
                .await;
            let _ = this.update(cx, |this, _| this.folder_choices = found);
        })
        .detach();
    }

    /// The first completed filter in the field that isn't a chip yet becomes one. The chip takes the space after it
    /// too, so the caret lands past it and the words typed next stay separate. Replacing fires another change, which
    /// picks up the next filter.
    fn tokenize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let content = self.query.read(cx).content();
        let text = content.text().to_string();
        let taken: Vec<std::ops::Range<usize>> =
            content.tokens().iter().map(|t| t.range()).collect();
        for (start, end, filter) in completed_filters(&text) {
            if taken.iter().any(|r| r.start < end && start < r.end) {
                continue;
            }
            let token = InlineToken::new(format!("{start}-{filter}"), text[start..end].to_owned())
                .with_label(format::chip_label(&filter));
            self.query.update(cx, |q, cx| {
                let _ = q.replace_range_with_token(start..end, token, window, cx);
            });
            return;
        }
    }

    pub(crate) fn schedule(&mut self, cx: &mut Context<Self>) {
        self.timers = [(QUICK_MS, true), (FULL_MS, false)]
            .into_iter()
            .map(|(ms, quick)| {
                cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(ms))
                        .await;
                    let _ = this.update(cx, |this, cx| this.run(quick, cx));
                })
            })
            .collect();
    }

    fn run(&mut self, quick: bool, cx: &mut Context<Self>) {
        let text = self.text(cx);
        if text.is_empty() {
            self.results.clear();
            self.took_ms = None;
            self.searched.clear();
            self.counts = None;
            self.load_recents(cx);
            cx.notify();
            return;
        }
        let request = service::SearchRequest {
            query: text.clone(),
            limit: 60,
            kinds: FILTERS[self.filter].kinds.to_vec(),
            quick,
        };
        self.generation += 1;
        let (generation, filter) = (self.generation, self.filter);
        cx.spawn(async move |this, cx| {
            let started = std::time::Instant::now();
            let answer = cx
                .background_spawn(async move { service::search(request) })
                .await;
            let _ = this.update(cx, |this, cx| match answer {
                Err(error) => {
                    if !quick {
                        this.error = Some(format!("{error:#}"));
                        cx.notify();
                    }
                }
                Ok(reply) => {
                    // A slower earlier search must not overwrite a newer one, nor a quick answer the full one.
                    if reply.superseded
                        || this.text(cx) != text
                        || this.filter != filter
                        || generation < this.shown_generation
                    {
                        return;
                    }
                    if quick && this.searched == text && !this.quick_shown {
                        return;
                    }
                    if !quick {
                        tracing::info!(
                            "search {text:?}: {} hits in {:.0} ms",
                            reply.hits.len(),
                            started.elapsed().as_secs_f64() * 1000.
                        );
                    }
                    let took = started.elapsed().as_secs_f64() * 1000.;
                    this.shown_generation = generation;
                    this.show(reply, text, quick, took, cx);
                }
            });
        })
        .detach();
    }

    /// Puts an answer on screen. The selection stays on the same file only when someone moved it there, for the same
    /// query (the full answer replacing the quick one, a reindex); otherwise it starts at the top, where the best
    /// result is.
    fn show(
        &mut self,
        reply: Reply,
        query: String,
        quick: bool,
        took_ms: f64,
        cx: &mut Context<Self>,
    ) {
        if query != self.searched {
            self.moved = false;
        }
        let keep = self
            .moved
            .then(|| self.results.get(self.selected).map(|h| h.path.clone()))
            .flatten();
        self.menu = None;
        self.error = None;
        self.results = reply.hits;
        self.took_ms = Some(took_ms);
        self.searched = query;
        self.quick_shown = quick;
        self.counts = Some(reply.counts);
        self.selected = keep
            .and_then(|path| self.results.iter().position(|h| h.path == path))
            .unwrap_or(0);
        self.scroll
            .scroll_to_item(self.row_of(self.selected), ScrollStrategy::Top);
        cx.notify();
        self.load_thumbs(cx);
        self.load_preview(cx);
    }

    pub(crate) fn load_recents(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let recents = cx
                .background_spawn(async move { service::recents() })
                .await
                .ok();
            let _ = this.update(cx, |this, cx| {
                this.recents = recents;
                cx.notify();
                let rows: Vec<Hit> = this
                    .recents
                    .iter()
                    .flat_map(|r| r.added.iter().chain(r.opened.iter()).cloned())
                    .collect();
                this.load_thumbs_for(rows, cx);
            });
        })
        .detach();
    }

    pub(crate) fn more_like_this(&mut self, cx: &mut Context<Self>) {
        let Some(hit) = self.results.get(self.selected).cloned() else {
            return;
        };
        let (id, code) = (hit.chunk_id, hit.kind == FileKind::Code);
        self.generation += 1;
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let started = std::time::Instant::now();
            let answer = cx
                .background_spawn(async move { service::similar(id, code) })
                .await;
            let _ = this.update(cx, |this, cx| match answer {
                Err(error) => {
                    this.error = Some(format!("{error:#}"));
                    cx.notify();
                }
                Ok(reply) => {
                    this.shown_generation = generation;
                    this.like = Some(hit);
                    let text = this.text(cx);
                    this.show(
                        reply,
                        text,
                        false,
                        started.elapsed().as_secs_f64() * 1000.,
                        cx,
                    );
                }
            });
        })
        .detach();
    }

    // ---- pictures and previews ---------------------------------------------------------------------------------

    fn load_thumbs(&mut self, cx: &mut Context<Self>) {
        self.load_thumbs_for(self.results.clone(), cx);
    }

    /// Small pictures, top of the list first, two at a time; each arrival repaints. Most were made while indexing.
    fn load_thumbs_for(&mut self, hits: Vec<Hit>, cx: &mut Context<Self>) {
        let wanted: VecDeque<Hit> = hits
            .into_iter()
            .filter(|h| {
                format::has_thumb(h.kind) && !self.thumbs.contains_key(&format::thumb_key(h))
            })
            .collect();
        if wanted.is_empty() {
            return;
        }
        let wanted = std::sync::Arc::new(std::sync::Mutex::new(wanted));
        self.thumb_task = Some(cx.spawn(async move |this, cx| {
            let lane = |wanted: std::sync::Arc<std::sync::Mutex<VecDeque<Hit>>>,
                        this: gpui_kit::WeakEntity<Self>,
                        cx: gpui_kit::AsyncApp| async move {
                let mut cx = cx;
                loop {
                    let Some(hit) = wanted.lock().unwrap().pop_front() else {
                        break;
                    };
                    let key = format::thumb_key(&hit);
                    let known = this
                        .read_with(&cx, |this, _| this.thumbs.contains_key(&key))
                        .unwrap_or(true);
                    if known {
                        continue;
                    }
                    let found = cx
                        .background_spawn(async move {
                            let path = omafind_extract::thumbs::get(
                                std::path::Path::new(&hit.path),
                                hit.kind,
                                format::locator_value(&hit),
                                omafind_extract::thumbs::Size::Small,
                            );
                            let size = path
                                .as_deref()
                                .and_then(omafind_extract::thumbs::dimensions);
                            (path, size)
                        })
                        .await;
                    if this
                        .update(&mut cx, |this, cx| {
                            if let Some((w, h)) = found.1
                                && h > 0
                            {
                                this.aspects.insert(key.clone(), w as f32 / h as f32);
                            }
                            this.thumbs.insert(key, found.0);
                            cx.notify();
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            };
            let first = lane(wanted.clone(), this.clone(), cx.clone());
            let second = lane(wanted, this, cx.clone());
            futures::join!(first, second);
        }));
    }

    /// What the preview of the selected result needs: the large picture, the passage, the lines of code.
    pub(crate) fn load_preview(&mut self, cx: &mut Context<Self>) {
        let Some(hit) = self.results.get(self.selected).cloned() else {
            return;
        };
        let key = format::thumb_key(&hit);
        if self.previews.contains_key(&key) {
            return;
        }
        self.previews.insert(key.clone(), Preview::default());
        self.preview_order.push_back(key.clone());
        while self.preview_order.len() > PREVIEWS_KEPT {
            if let Some(old) = self.preview_order.pop_front() {
                self.previews.remove(&old);
            }
        }
        cx.spawn(async move |this, cx| {
            let preview = cx
                .background_spawn(async move { crate::preview::load(&hit) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(entry) = this.previews.get_mut(&key) {
                    *entry = preview;
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ---- moving and acting -------------------------------------------------------------------------------------

    pub(crate) fn row_of(&self, index: usize) -> usize {
        if self.is_grid() {
            index / self.cols.max(1)
        } else {
            index
        }
    }

    fn move_by(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.results.is_empty() {
            return;
        }
        let last = self.results.len() as isize - 1;
        self.selected = (self.selected as isize + delta).clamp(0, last) as usize;
        self.moved = true;
        self.scroll
            .scroll_to_item(self.row_of(self.selected), ScrollStrategy::Top);
        self.load_preview(cx);
        cx.notify();
    }

    /// A click on a row or tile: the first selects it, a second on the selected one opens it.
    pub(crate) fn click_result(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.selected == index {
            self.open_selected(cx);
        } else {
            self.selected = index;
            self.moved = true;
            self.load_preview(cx);
            cx.notify();
        }
    }

    pub(crate) fn set_filter(&mut self, index: usize, cx: &mut Context<Self>) {
        self.filter = index;
        self.grid = None;
        cx.notify();
        if !self.text(cx).is_empty() {
            self.run(true, cx);
            self.run(false, cx);
        }
    }

    fn cycle_filter(&mut self, delta: isize, cx: &mut Context<Self>) {
        let n = FILTERS.len() as isize;
        self.set_filter(((self.filter as isize + delta + n) % n) as usize, cx);
    }

    /// Where to open: the line the preview found for code and plain text (a document's own locator is a character
    /// offset, which no app takes), else the page or moment.
    fn open_locator(&self, hit: &Hit) -> Option<Locator> {
        if let Some(best) = self
            .previews
            .get(&format::thumb_key(hit))
            .and_then(|p| p.best)
        {
            return Some(Locator::Line(best as u32));
        }
        match (hit.kind, hit.locator) {
            (FileKind::Doc, _) | (_, Some(Locator::Offset(_))) => None,
            (_, locator) => locator,
        }
    }

    pub(crate) fn open_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(hit) = self.results.get(self.selected).cloned() {
            let locator = self.open_locator(&hit);
            self.open_hit(&hit, locator);
        }
        cx.notify();
    }

    /// Opens a file where the match is, tells the daemon (frecency), and hides the window.
    pub(crate) fn open_hit(&self, hit: &Hit, locator: Option<Locator>) {
        let words =
            omafind_core::ranking::query_words(&omafind_core::query::parse(&self.searched).text);
        let path = PathBuf::from(&hit.path);
        if let Err(error) = crate::actions::open(&path, hit.kind, locator, &words) {
            tracing::warn!("open {}: {error:#}", hit.path);
            return;
        }
        service::tell_daemon(Request::Opened { path, query: None });
        crate::hide_window();
    }

    pub(crate) fn reveal(&self, hit: &Hit) {
        if let Err(error) = crate::actions::reveal(std::path::Path::new(&hit.path)) {
            tracing::warn!("reveal {}: {error:#}", hit.path);
            return;
        }
        crate::hide_window();
    }

    /// Actions for the selected result, in the order the menu shows them.
    fn actions_for(&self, hit: &Hit) -> Vec<MenuItem> {
        let item = |label: String, keys, action| MenuItem {
            label,
            keys,
            danger: false,
            action,
        };
        let mut items = vec![
            item(format::primary_label(hit), "⏎", MenuAction::Open),
            item("More like this".into(), "⌃S", MenuAction::MoreLikeThis),
            item("Show in Files".into(), "⌃⏎", MenuAction::Reveal),
            item("Copy file".into(), "⌃C", MenuAction::CopyFile),
            item("Copy path".into(), "⌃⇧C", MenuAction::CopyPath),
        ];
        let passage = self
            .previews
            .get(&format::thumb_key(hit))
            .and_then(|p| p.passage.as_ref())
            .map(|p| p.text.clone())
            .filter(|t| !t.is_empty());
        if let Some(text) =
            passage.or_else(|| (!hit.snippet.is_empty()).then(|| hit.snippet.clone()))
        {
            let label = if hit.kind == FileKind::Screenshot {
                "Copy text in image"
            } else {
                "Copy passage"
            };
            items.push(item(label.into(), "", MenuAction::CopyText(text)));
        }
        items.push(MenuItem {
            label: "Move to trash".into(),
            keys: "",
            danger: true,
            action: MenuAction::Trash,
        });
        items
    }

    fn open_menu(&mut self, cx: &mut Context<Self>) {
        let Some(hit) = self.results.get(self.selected).cloned() else {
            return;
        };
        self.menu = Some(Menu {
            items: self.actions_for(&hit),
            hit,
            selected: 0,
            confirm: false,
        });
        cx.notify();
    }

    pub(crate) fn run_menu(&mut self, cx: &mut Context<Self>) {
        let Some(menu) = self.menu.as_mut() else {
            return;
        };
        let item = &menu.items[menu.selected];
        if matches!(item.action, MenuAction::Trash) && !menu.confirm {
            menu.confirm = true;
            cx.notify();
            return;
        }
        let (action, hit) = (item.action.clone(), menu.hit.clone());
        self.menu = None;
        match action {
            MenuAction::Open => {
                let locator = self.open_locator(&hit);
                self.open_hit(&hit, locator);
            }
            MenuAction::MoreLikeThis => self.more_like_this(cx),
            MenuAction::Reveal => self.reveal(&hit),
            MenuAction::CopyFile => report(crate::actions::copy(std::path::Path::new(&hit.path))),
            MenuAction::CopyPath => report(crate::actions::copy_text(&hit.path)),
            MenuAction::CopyText(text) => report(crate::actions::copy_text(&text)),
            MenuAction::Trash => self.trash_selected(cx),
        }
        cx.notify();
    }

    pub(crate) fn select_menu_item(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(menu) = self.menu.as_mut() {
            menu.selected = index;
        }
        self.run_menu(cx);
    }

    pub(crate) fn close_menu(&mut self, cx: &mut Context<Self>) {
        self.menu = None;
        cx.notify();
    }

    fn trash_selected(&mut self, cx: &mut Context<Self>) {
        let Some(hit) = self.results.get(self.selected).cloned() else {
            return;
        };
        match crate::actions::trash(std::path::Path::new(&hit.path)) {
            Ok(()) => {
                self.results.retain(|h| h.path != hit.path);
                self.selected = self.selected.min(self.results.len().saturating_sub(1));
            }
            Err(error) => self.error = Some(format!("{error:#}")),
        }
        cx.notify();
    }

    /// A recent search clicked, or recalled with ↑.
    pub(crate) fn recall(&mut self, query: String, window: &mut Window, cx: &mut Context<Self>) {
        self.query
            .update(cx, |q, cx| q.set_value(query, window, cx));
        self.schedule(cx);
    }

    // ---- keys --------------------------------------------------------------------------------------------------

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.unparse();
        if self.key(&key, window, cx) {
            cx.stop_propagation();
        }
    }

    /// Handles a key; true when it was this view's.
    fn key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.mode == Mode::Setup {
            return self
                .setup
                .update(cx, |setup, cx| setup.key(key, window, cx));
        }
        if let Some(completion) = self.completion.as_mut() {
            let n = completion.items.len();
            match key {
                "down" | "ctrl-n" | "ctrl-j" => completion.selected = (completion.selected + 1) % n,
                "up" | "ctrl-p" => completion.selected = (completion.selected + n - 1) % n,
                "tab" => {
                    self.accept_completion(window, cx);
                    return true;
                }
                "escape" => {
                    self.completion = None;
                    self.completion_closed_for = Some(self.query.read(cx).value().to_string());
                }
                _ => return false,
            }
            cx.notify();
            return true;
        }
        if let Some(menu) = self.menu.as_mut() {
            let n = menu.items.len();
            match key {
                "escape" => self.close_menu(cx),
                "down" | "ctrl-n" | "ctrl-j" => {
                    menu.selected = (menu.selected + 1) % n;
                    menu.confirm = false;
                    cx.notify();
                }
                "up" | "ctrl-p" | "ctrl-k" => {
                    menu.selected = (menu.selected + n - 1) % n;
                    menu.confirm = false;
                    cx.notify();
                }
                "enter" => self.run_menu(cx),
                _ => return false,
            }
            return true;
        }
        let has_row = self.results.get(self.selected).is_some();
        let grid = self.is_grid() && !self.results.is_empty();
        let cols = self.cols as isize;
        match key {
            "escape" if self.progress_open => {
                self.progress_open = false;
                cx.notify();
            }
            "escape" => {
                if self.like.is_some() {
                    self.like = None;
                    self.run(false, cx);
                } else {
                    crate::hide_window();
                }
            }
            "ctrl-," => self.open_setup(false, window, cx),
            "down" | "ctrl-n" | "ctrl-j" => self.move_by(if grid { cols } else { 1 }, cx),
            "up" => {
                let searches = self.recents.as_ref().map(|r| r.searches.len()).unwrap_or(0);
                if self.text(cx).is_empty() && searches > 0 {
                    let at = self.history_at.map_or(0, |at| (at + 1).min(searches - 1));
                    self.history_at = Some(at);
                    let query = self.recents.as_ref().unwrap().searches[at].0.clone();
                    self.recall(query, window, cx);
                    return true;
                }
                self.move_by(if grid { -cols } else { -1 }, cx);
            }
            "ctrl-k" if has_row => self.open_menu(cx),
            "ctrl-p" | "ctrl-k" => self.move_by(if grid { -cols } else { -1 }, cx),
            "right" if grid => self.move_by(1, cx),
            "left" if grid => self.move_by(-1, cx),
            "pagedown" => self.move_by(if grid { cols * 3 } else { 8 }, cx),
            "pageup" => self.move_by(if grid { -cols * 3 } else { -8 }, cx),
            "enter" => self.open_selected(cx),
            "ctrl-enter" if has_row => {
                let hit = self.results[self.selected].clone();
                self.reveal(&hit);
            }
            "ctrl-c" if has_row => report(crate::actions::copy(std::path::Path::new(
                &self.results[self.selected].path,
            ))),
            "ctrl-shift-c" if has_row => {
                report(crate::actions::copy_text(&self.results[self.selected].path))
            }
            "ctrl-s" => self.more_like_this(cx),
            "ctrl-g" => {
                self.grid = Some(!self.is_grid());
                self.scroll
                    .scroll_to_item(self.row_of(self.selected), ScrollStrategy::Top);
                cx.notify();
            }
            "tab" => self.cycle_filter(1, cx),
            "shift-tab" => self.cycle_filter(-1, cx),
            _ => return false,
        }
        true
    }
}

/// Logs a clipboard or file action that failed; the window has no place to say so that wouldn't get in the way.
fn report(result: anyhow::Result<()>) {
    if let Err(error) = result {
        tracing::warn!("{error:#}");
    }
}

/// Filters typed into the field turn into chips once they are complete, that is followed by a space: each one's byte
/// range (the space included) and its text.
fn completed_filters(text: &str) -> Vec<(usize, usize, String)> {
    static FILTER: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r"(?i)(?:^|\s)((?:in|type|kind|after|since|before|on|during):\S+|today|yesterday|(?:this|last) (?:week|month|year))",
        )
        .unwrap()
    });
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(found) = FILTER.captures_at(text, at) {
        let m = found.get(1).unwrap();
        // Complete only when a space follows: "type:pd" is still being typed.
        if text[m.end()..].starts_with(char::is_whitespace) {
            let space = text[m.end()..].chars().next().map_or(1, char::len_utf8);
            out.push((m.start(), m.end() + space, m.as_str().to_owned()));
        }
        at = m.end();
    }
    out
}

impl Render for SearchView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = colors(cx);
        self.layout(window);
        let font = gpui_kit::component::Theme::global(cx)
            .mono_font_family
            .clone();
        let root = div()
            .flex()
            .flex_col()
            .size_full()
            .relative()
            .bg(c.background)
            .text_color(c.foreground)
            .font_family(font)
            .key_context("Omafind")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_action(cx.listener(|this, _: &NextFilter, window, cx| {
                this.key("tab", window, cx);
            }))
            .on_action(cx.listener(|this, _: &PreviousFilter, window, cx| {
                this.key("shift-tab", window, cx);
            }));
        if self.mode == Mode::Setup {
            let dialog = Setup::dialog(&self.setup, self.first_run, cx);
            return root
                .child(self.setup.clone())
                .child(self.render_footer(cx))
                .children(dialog)
                .into_any_element();
        }
        let body = if self.progress_open {
            crate::home::indexing(cx.global::<AppState>(), cx)
        } else if self.results.is_empty() {
            self.render_empty(cx).into_any_element()
        } else {
            div()
                .flex()
                .flex_1()
                .min_h_0()
                .child(self.render_list(cx))
                .child(self.render_preview(cx))
                .into_any_element()
        };
        // While filters are offered, the results stand back, as on the board.
        let body = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .when(self.completion.is_some(), |d| d.opacity(0.55))
            .child(body);
        root.child(self.render_header(cx))
            .child(self.render_filters(cx))
            .child(body)
            .child(self.render_footer(cx))
            .children(
                self.completion
                    .is_some()
                    .then(|| self.render_completion(cx)),
            )
            .children(self.menu.is_some().then(|| self.render_menu(cx)))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_become_chips_once_a_space_follows() {
        assert_eq!(completed_filters("type:pd"), vec![]);
        assert_eq!(
            completed_filters("type:pdf "),
            vec![(0, 9, "type:pdf".into())]
        );
        assert_eq!(
            completed_filters("invoice type:pdf after:2025 x"),
            vec![(8, 17, "type:pdf".into()), (17, 28, "after:2025".into())]
        );
        assert_eq!(
            completed_filters("photos last week "),
            vec![(7, 17, "last week".into())]
        );
        // Inside a word is not a filter.
        assert_eq!(completed_filters("within:x "), vec![]);
    }
}

/// UI integration tests: the real view in a headless window, driven by keys and typing as a person would. Nothing here
/// presses Escape on the search screen or Enter on a result: those hide the real Omafind workspace and open files.
#[cfg(test)]
mod ui_tests {
    use super::*;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{
        AnyWindowHandle, Bounds, KeyBinding, TestAppContext, WindowBounds, WindowOptions, px, size,
    };
    use omafind_core::{ChunkKind, Reason};

    fn hit(id: i64, name: &str) -> Hit {
        Hit {
            file_id: id,
            chunk_id: id,
            path: format!("/nonexistent/omafind-ui-test/{name}"),
            name: name.into(),
            kind: FileKind::Doc,
            chunk_kind: ChunkKind::Passage,
            locator: None,
            locator_end: None,
            locator_label: String::new(),
            snippet: format!("about {name}"),
            score: 1.,
            similarity: None,
            matched_words: Vec::new(),
            reason: Reason::Text,
            mtime: 0,
            size: 0,
            duplicates: Vec::new(),
        }
    }

    /// A window with the search view, as `main` opens it, talking to no daemon.
    fn open(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<SearchView>) {
        // A test search must never reach the real daemon (it would land in the search history).
        unsafe { std::env::set_var("OMAFIND_SOCKET", "/nonexistent/omafind-ui-test.sock") };
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(AppState {
                daemon: true,
                configured: true,
                ..Default::default()
            });
            cx.bind_keys([
                KeyBinding::new("tab", NextFilter, Some("Omafind")),
                KeyBinding::new("shift-tab", PreviousFilter, Some("Omafind")),
            ]);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    Default::default(),
                    size(px(1400.), px(900.)),
                ))),
                ..Default::default()
            };
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| SearchView::new(window, cx))
            })
            .unwrap()
        })
    }

    fn show(view: &Entity<SearchView>, names: &[&str], cx: &mut TestAppContext) {
        let hits = names
            .iter()
            .enumerate()
            .map(|(i, n)| hit(i as i64 + 1, n))
            .collect();
        cx.update(|cx| {
            view.update(cx, |v, cx| {
                v.show(
                    Reply {
                        hits,
                        ..Default::default()
                    },
                    "lease".into(),
                    false,
                    1.,
                    cx,
                )
            })
        });
    }

    fn press(window: AnyWindowHandle, keys: &[&str], cx: &mut TestAppContext) {
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            for key in keys {
                window.press(key, cx);
            }
        })
        .unwrap();
    }

    fn read<R>(
        view: &Entity<SearchView>,
        cx: &mut TestAppContext,
        f: impl FnOnce(&SearchView, &gpui_kit::App) -> R,
    ) -> R {
        cx.update(|cx| f(view.read(cx), cx))
    }

    #[gpui_kit::test]
    fn arrow_keys_move_the_selection_and_stop_at_the_ends(cx: &mut TestAppContext) {
        let (window, view) = open(cx);
        show(&view, &["lease.pdf", "rules.pdf", "checklist.md"], cx);
        press(window, &["down"], cx);
        assert_eq!(read(&view, cx, |v, _| v.selected), 1);
        press(window, &["down", "down", "down"], cx);
        assert_eq!(read(&view, cx, |v, _| v.selected), 2);
        press(window, &["up", "ctrl-p"], cx);
        assert_eq!(read(&view, cx, |v, _| v.selected), 0);
        press(window, &["ctrl-n", "pageup"], cx);
        assert_eq!(read(&view, cx, |v, _| v.selected), 0);
        press(window, &["pagedown"], cx);
        assert_eq!(read(&view, cx, |v, _| v.selected), 2);
    }

    #[gpui_kit::test]
    fn tab_cycles_the_filters_and_ctrl_g_flips_the_layout(cx: &mut TestAppContext) {
        let (window, view) = open(cx);
        show(&view, &["lease.pdf"], cx);
        press(window, &["tab"], cx);
        assert_eq!(read(&view, cx, |v, _| (v.filter, v.is_grid())), (1, true));
        press(window, &["tab", "shift-tab", "shift-tab"], cx);
        assert_eq!(read(&view, cx, |v, _| v.filter), 0);
        press(window, &["shift-tab"], cx);
        assert_eq!(read(&view, cx, |v, _| v.filter), FILTERS.len() - 1);
        press(window, &["tab", "ctrl-g"], cx);
        assert_eq!(read(&view, cx, |v, _| (v.filter, v.is_grid())), (0, true));
        press(window, &["ctrl-g"], cx);
        assert!(!read(&view, cx, |v, _| v.is_grid()));
    }

    #[gpui_kit::test]
    fn the_actions_menu_moves_asks_before_trashing_and_closes(cx: &mut TestAppContext) {
        let (window, view) = open(cx);
        show(&view, &["lease.pdf", "rules.pdf"], cx);
        press(window, &["down", "ctrl-k"], cx);
        let (items, name) = read(&view, cx, |v, _| {
            let menu = v.menu.as_ref().expect("Ctrl+K opens the menu");
            (menu.items.len(), menu.hit.name.clone())
        });
        assert_eq!(name, "rules.pdf");
        // Up from the first item wraps to the last: Move to trash.
        press(window, &["up"], cx);
        assert_eq!(
            read(&view, cx, |v, _| v.menu.as_ref().unwrap().selected),
            items - 1
        );
        press(window, &["enter"], cx);
        assert!(
            read(&view, cx, |v, _| v.menu.as_ref().unwrap().confirm),
            "trash asks again first"
        );
        press(window, &["down"], cx);
        assert!(
            !read(&view, cx, |v, _| v.menu.as_ref().unwrap().confirm),
            "moving away cancels the question"
        );
        press(window, &["escape"], cx);
        assert!(read(&view, cx, |v, _| v.menu.is_none()));
        assert_eq!(
            read(&view, cx, |v, _| v.results.len()),
            2,
            "nothing was trashed"
        );
    }

    #[gpui_kit::test]
    fn typing_a_filter_offers_completions_and_tab_makes_a_chip(cx: &mut TestAppContext) {
        let (window, view) = open(cx);
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.input("invoice af", cx);
        })
        .unwrap();
        let first = read(&view, cx, |v, _| {
            v.completion.as_ref().map(|c| c.items[0].text.clone())
        });
        assert_eq!(
            first.as_deref(),
            Some(format!("after:{}", jiff::Zoned::now().year()).as_str())
        );
        press(window, &["down", "tab"], cx);
        let (text, chips, open) = read(&view, cx, |v, cx| {
            let content = v.query.read(cx).content();
            (
                content.text().to_string(),
                content.tokens().len(),
                v.completion.is_some(),
            )
        });
        assert_eq!(text, "invoice after:last-month ");
        assert_eq!(chips, 1, "the completed filter is a chip");
        assert!(!open);
    }

    #[gpui_kit::test]
    fn escape_closes_the_list_until_the_text_changes(cx: &mut TestAppContext) {
        let (window, view) = open(cx);
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.input("ty", cx);
        })
        .unwrap();
        assert!(read(&view, cx, |v, _| v.completion.is_some()));
        press(window, &["escape"], cx);
        assert!(read(&view, cx, |v, _| v.completion.is_none()));
        cx.update_window(window, |_, window, cx| window.input("pe:p", cx))
            .unwrap();
        let items = read(&view, cx, |v, _| {
            v.completion.as_ref().map(|c| c.items.len())
        });
        assert_eq!(
            items,
            Some(2),
            "typing again offers type:photos and type:pdf"
        );
    }
}

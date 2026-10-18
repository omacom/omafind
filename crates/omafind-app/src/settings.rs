//! Setup, in the window: the first run's onboarding (welcome, the model, folders, the shortcut, then indexing) and the
//! settings screen (Ctrl+, or the footer gear). Both edit the same choices — folders, code folders, power, the
//! shortcut, the model files — and nothing is written until Save; leaving settings with changes asks first.
//!
//! The entity owns the choices, the folder survey, the download and the save. The search view owns the window around
//! it: it routes keys here, shows the footer this says, draws the leave dialog over everything, and handles
//! [`SetupEvent`].

use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::base::{StyledExt as _, VirtualListScrollHandle, h_flex, v_flex, v_virtual_list};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, ScrollStrategy, StatefulInteractiveElement as _,
    Styled as _, Window, div, prelude::FluentBuilder as _, px, relative, size,
};

use crate::format::{self, glyphs};
use crate::service::{self, Candidates, Folder, Plan, Power, Progress};
use crate::state::AppState;
use crate::widgets::{BarButton, ButtonKind, alpha, clickable, colors};

/// What setup asks of the window around it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupEvent {
    /// Back to searching (settings closed, saved or not).
    Close,
    /// The first run is done: indexing has started.
    Finished,
    /// "Later" on the welcome: hide the window, keep the screen for next time.
    Later,
}

impl EventEmitter<SetupEvent> for Setup {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Folders,
    Code,
    Power,
    Desktop,
    Model,
}

const SECTIONS: [(Section, &str, &str); 5] = [
    (Section::Folders, "Folders", "Folders to index"),
    (Section::Code, "Code", "Code · searched with a code: prefix"),
    (Section::Power, "Power", "Power"),
    (Section::Desktop, "Desktop", "Desktop"),
    (Section::Model, "Model", "Model"),
];

/// One row the cursor walks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Folder(usize),
    Code(usize),
    Add { code: bool },
    PowerSaver,
    Charger,
    Shortcut,
    Models,
}

impl Row {
    fn section(self) -> Section {
        match self {
            Row::Folder(_) | Row::Add { code: false } => Section::Folders,
            Row::Code(_) | Row::Add { code: true } => Section::Code,
            Row::PowerSaver | Row::Charger => Section::Power,
            Row::Shortcut => Section::Desktop,
            Row::Models => Section::Model,
        }
    }
}

/// The list, flat: section headings and rows, each with its height.
#[derive(Debug, Clone, Copy)]
enum Item {
    Heading { title: &'static str, height: f32 },
    Row { row: Row, index: usize },
}

impl Item {
    fn height(self) -> f32 {
        match self {
            Item::Heading { height, .. } => height,
            Item::Row {
                row: Row::Add { .. },
                ..
            } => 38.,
            Item::Row { .. } => 46.,
        }
    }
}

/// The onboarding's steps, and which sections of the list each shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Welcome,
    Model,
    Folders,
    Shortcut,
}

const STEPS: [(Step, &str); 4] = [
    (Step::Welcome, "Welcome"),
    (Step::Model, "Model"),
    (Step::Folders, "Folders"),
    (Step::Shortcut, "Shortcut"),
];

// EmbeddingGemma 2 comes as two files; the page shows them as the one model they are.
const TEXT_MB: f64 = 310.;
const MEDIA_MB: f64 = 555.;

const EXAMPLES: [(&str, &str, &str, &str); 3] = [
    (
        "dog running on the beach",
        "IMG_4102.jpg · ~/Pictures/2024",
        "photo",
        glyphs::PHOTO,
    ),
    (
        "the clause about pets in the lease",
        "lease-2024.pdf · page 7",
        "pdf",
        glyphs::PDF,
    ),
    (
        "where they talk about sleep",
        "podcast-ep12.m4a · at 23:40",
        "said",
        glyphs::MICROPHONE,
    ),
];

/// What a running operation blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Busy {
    /// The folder chooser is open, for code folders or not.
    Pick {
        code: bool,
    },
    Apply,
}

/// What happens once a save has worked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AfterSave {
    Stay,
    Close,
    Finish,
}

/// Everything Save writes, to tell whether anything changed.
#[derive(Debug, Clone, PartialEq, Default)]
struct Snapshot {
    folders: Vec<PathBuf>,
    code: Vec<PathBuf>,
    power: Option<Power>,
    shortcut: bool,
}

pub struct Setup {
    loading: bool,
    folders: Vec<Folder>,
    code: Vec<Folder>,
    power: Power,
    shortcut: bool,
    text_model: bool,
    media_model: bool,
    configured: bool,
    index_bytes: u64,
    /// The shortcut is already in ~/.config/hypr/omafind.lua.
    binding: bool,
    cursor: usize,
    busy: Option<Busy>,
    downloading: bool,
    progress: Option<Progress>,
    error: Option<String>,
    message: Option<String>,
    /// The leave-with-changes dialog is up.
    confirm: bool,
    saved: Snapshot,
    first_run: bool,
    step: usize,
    scroll: VirtualListScrollHandle,
}

impl Setup {
    pub fn new(_: &mut Context<Self>) -> Self {
        Self {
            loading: true,
            folders: Vec::new(),
            code: Vec::new(),
            power: Power::default(),
            shortcut: true,
            text_model: true,
            media_model: true,
            configured: false,
            index_bytes: 0,
            binding: false,
            cursor: 0,
            busy: None,
            downloading: false,
            progress: None,
            error: None,
            message: None,
            confirm: false,
            saved: Snapshot::default(),
            first_run: false,
            step: 0,
            scroll: VirtualListScrollHandle::new(),
        }
    }

    /// Opens the onboarding (a first run) or the settings.
    pub fn open(&mut self, first_run: bool, cx: &mut Context<Self>) {
        self.first_run = first_run;
        self.step = 0;
        self.cursor = 0;
        self.error = None;
        self.load(cx);
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        self.confirm = false;
        self.message = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let found: Candidates = cx
                .background_spawn(async move { service::candidates() })
                .await;
            let _ = this.update(cx, |this, cx| {
                // Offered ticked on a first run; in settings, only added when ticked. (The window rules and autostart
                // always go in; the shortcut is the only choice.)
                this.shortcut = !found.configured && !found.binding;
                this.binding = found.binding;
                this.folders = found.folders;
                this.code = found.code;
                this.power = found.power;
                this.configured = found.configured;
                this.index_bytes = found.index_bytes;
                this.text_model = found.text_model;
                this.media_model = found.media_model;
                this.error = None;
                this.saved = this.snapshot();
                this.loading = false;
                this.cursor = 0;
                cx.notify();
            });
        })
        .detach();
    }

    fn snapshot(&self) -> Snapshot {
        let on = |list: &[Folder]| {
            let mut paths: Vec<PathBuf> = list
                .iter()
                .filter(|f| f.selected)
                .map(|f| f.path.clone())
                .collect();
            paths.sort();
            paths
        };
        Snapshot {
            folders: on(&self.folders),
            code: on(&self.code),
            power: Some(self.power),
            shortcut: self.shortcut,
        }
    }

    fn is_dirty(&self) -> bool {
        !self.loading && self.snapshot() != self.saved
    }

    /// Save is offered when it would do something: a change, or on a first run, a start.
    fn can_save(&self) -> bool {
        if self.loading || self.busy.is_some() || self.downloading {
            return false;
        }
        self.first_run || self.is_dirty()
    }

    fn step_kind(&self) -> Step {
        STEPS[self.step].0
    }

    /// The sections the list shows: all of them in settings, a step's own in the onboarding.
    fn shows(&self, section: Section) -> bool {
        if !self.first_run {
            return true;
        }
        match self.step_kind() {
            Step::Welcome | Step::Model => false,
            Step::Folders => matches!(section, Section::Folders | Section::Code),
            Step::Shortcut => section == Section::Desktop,
        }
    }

    /// The rows the cursor walks, in order.
    fn rows(&self) -> Vec<Row> {
        let mut rows: Vec<Row> = (0..self.folders.len()).map(Row::Folder).collect();
        rows.push(Row::Add { code: false });
        rows.extend((0..self.code.len()).map(Row::Code));
        rows.extend([
            Row::Add { code: true },
            Row::PowerSaver,
            Row::Charger,
            Row::Shortcut,
            Row::Models,
        ]);
        rows.retain(|r| self.shows(r.section()));
        rows
    }

    fn items(&self) -> Vec<Item> {
        // One section on the step's own page needs no heading of its own.
        let headings = !(self.first_run && self.step_kind() == Step::Shortcut);
        let mut out = Vec::new();
        let mut section = None;
        for (index, row) in self.rows().into_iter().enumerate() {
            if headings && section != Some(row.section()) {
                section = Some(row.section());
                let title = SECTIONS
                    .iter()
                    .find(|(s, ..)| *s == row.section())
                    .unwrap()
                    .2;
                out.push(Item::Heading {
                    title,
                    height: if out.is_empty() { 30. } else { 40. },
                });
            }
            out.push(Item::Row { row, index });
        }
        out
    }

    fn move_by(&mut self, delta: isize, cx: &mut Context<Self>) {
        let n = self.rows().len() as isize;
        if n == 0 {
            return;
        }
        self.move_to((self.cursor as isize + delta).clamp(0, n - 1) as usize, cx);
    }

    fn move_to(&mut self, index: usize, cx: &mut Context<Self>) {
        self.cursor = index;
        let items = self.items();
        if let Some(at) = items
            .iter()
            .position(|it| matches!(it, Item::Row { index: i, .. } if *i == index))
        {
            // The section heading too, when the row is a section's first.
            let target = if at > 0 && matches!(items[at - 1], Item::Heading { .. }) {
                at - 1
            } else {
                at
            };
            self.scroll.scroll_to_item(target, ScrollStrategy::Top);
        }
        cx.notify();
    }

    fn toggle(&mut self, cx: &mut Context<Self>) {
        let Some(row) = self.rows().get(self.cursor).copied() else {
            return;
        };
        match row {
            Row::Folder(i) => self.folders[i].selected = !self.folders[i].selected,
            Row::Code(i) => self.code[i].selected = !self.code[i].selected,
            Row::PowerSaver => self.power.pause_in_power_saver = !self.power.pause_in_power_saver,
            Row::Charger => self.power.wait_for_ac = !self.power.wait_for_ac,
            Row::Shortcut => {
                if !self.binding {
                    self.shortcut = !self.shortcut;
                }
            }
            Row::Add { code } => return self.add(code, cx),
            Row::Models => {
                if self.is_model_missing() {
                    self.download(cx);
                }
                return;
            }
        }
        cx.notify();
    }

    fn click_row(&mut self, index: usize, cx: &mut Context<Self>) {
        self.cursor = index;
        self.toggle(cx);
    }

    fn is_model_missing(&self) -> bool {
        !self.text_model || !self.media_model
    }

    /// Asks the desktop for a folder and adds it, ticked, with what it holds.
    fn add(&mut self, code: bool, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(Busy::Pick { code });
        cx.notify();
        cx.spawn(async move |this, cx| {
            let picked = cx
                .background_spawn(async move {
                    let path = service::pick_folder()?;
                    Ok::<_, anyhow::Error>(path.map(|p| {
                        let survey = service::survey_folder(&p, code);
                        (p, survey)
                    }))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                match picked {
                    Ok(Some((path, survey))) => {
                        let list = if code {
                            &mut this.code
                        } else {
                            &mut this.folders
                        };
                        match list.iter().position(|f| f.path == path) {
                            Some(known) => list[known].selected = true,
                            None => match survey {
                                Some(folder) => list.push(folder),
                                None => this.error = Some("Can't read that folder".into()),
                            },
                        }
                        let rows = this.rows();
                        let list = if code { &this.code } else { &this.folders };
                        if let Some(at) = rows.iter().position(|r| match r {
                            Row::Code(i) if code => list[*i].path == path,
                            Row::Folder(i) if !code => list[*i].path == path,
                            _ => false,
                        }) {
                            this.cursor = at;
                        }
                    }
                    Ok(None) => {}
                    Err(error) => this.error = Some(format!("{error:#}")),
                }
                this.busy = None;
                cx.notify();
            });
        })
        .detach();
    }

    /// Its own flag rather than `busy`: folders can be picked while it runs.
    fn download(&mut self, cx: &mut Context<Self>) {
        if self.downloading {
            return;
        }
        self.downloading = true;
        cx.notify();
        let (tx, rx) = async_channel::unbounded::<Option<Progress>>();
        let download = cx.background_spawn(async move {
            service::download_models(|step| {
                let _ = tx.send_blocking(step);
            })
        });
        cx.spawn(async move |this, cx| {
            // The channel closes when the download returns and drops its sender.
            while let Ok(step) = rx.recv().await {
                if this
                    .update(cx, |this, cx| {
                        this.progress = step;
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
            let result = download.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.text_model = true;
                        this.media_model = true;
                        this.error = None;
                    }
                    Err(error) => this.error = Some(format!("{error:#}")),
                }
                this.progress = None;
                this.downloading = false;
                cx.notify();
            });
        })
        .detach();
    }

    /// Saves and starts.
    fn apply(&mut self, after: AfterSave, cx: &mut Context<Self>) {
        if !self.can_save() {
            return;
        }
        let folders: Vec<PathBuf> = self
            .folders
            .iter()
            .filter(|f| f.selected)
            .map(|f| f.path.clone())
            .collect();
        let code: Vec<PathBuf> = self
            .code
            .iter()
            .filter(|f| f.selected)
            .map(|f| f.path.clone())
            .collect();
        if folders.is_empty() && code.is_empty() {
            self.error = Some("Pick at least one folder".into());
            cx.notify();
            return;
        }
        if !self.text_model {
            self.error = Some("Download the model files first".into());
            cx.notify();
            return;
        }
        self.busy = Some(Busy::Apply);
        cx.notify();
        let plan = Plan {
            folders,
            code,
            power: self.power,
            shortcut: self.shortcut,
        };
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { service::apply_setup(plan) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.busy = None;
                match result {
                    Ok(message) => {
                        this.error = None;
                        this.message = Some(message);
                        for f in this.folders.iter_mut().chain(this.code.iter_mut()) {
                            f.chosen = f.selected;
                        }
                        this.configured = true;
                        this.saved = this.snapshot();
                        match after {
                            AfterSave::Stay => {}
                            AfterSave::Close => {
                                this.confirm = false;
                                cx.emit(SetupEvent::Close);
                            }
                            AfterSave::Finish => cx.emit(SetupEvent::Finished),
                        }
                    }
                    Err(error) => {
                        this.error = Some(format!("{error:#}"));
                        this.confirm = false;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Save from the bar or Ctrl+S: a first run moves on to searching; settings stay up, Save greyed again.
    fn save(&mut self, cx: &mut Context<Self>) {
        let after = if self.first_run {
            AfterSave::Finish
        } else {
            AfterSave::Stay
        };
        self.apply(after, cx);
    }

    /// Back to searching now, or once the user says what to do with the changes.
    pub fn leave(&mut self, cx: &mut Context<Self>) {
        if self.is_dirty() {
            self.confirm = true;
            cx.notify();
        } else {
            cx.emit(SetupEvent::Close);
        }
    }

    fn keep_editing(&mut self, cx: &mut Context<Self>) {
        self.confirm = false;
        cx.notify();
    }

    fn discard(&mut self, cx: &mut Context<Self>) {
        self.confirm = false;
        self.load(cx);
        cx.emit(SetupEvent::Close);
    }

    fn save_and_leave(&mut self, cx: &mut Context<Self>) {
        self.apply(AfterSave::Close, cx);
    }

    // ---- the onboarding's steps ----------------------------------------------------------------------------------

    fn go(&mut self, step: usize, cx: &mut Context<Self>) {
        self.step = step;
        self.cursor = 0;
        self.error = None;
        cx.notify();
    }

    /// Enter and the primary button: one step forward; on the model step a first press starts the download.
    fn next(&mut self, cx: &mut Context<Self>) {
        match self.step_kind() {
            Step::Model if self.is_model_missing() && !self.downloading => {
                return self.download(cx);
            }
            Step::Folders => {
                if self.loading {
                    return;
                }
                if !self.folders.iter().chain(&self.code).any(|f| f.selected) {
                    self.error = Some("Pick at least one folder".into());
                    cx.notify();
                    return;
                }
            }
            Step::Shortcut => return self.apply(AfterSave::Finish, cx),
            _ => {}
        }
        self.go(self.step + 1, cx);
    }

    /// Esc and Back: a step back; from the welcome, later.
    fn back(&mut self, cx: &mut Context<Self>) {
        if self.step > 0 {
            self.go(self.step - 1, cx);
        } else {
            cx.emit(SetupEvent::Later);
        }
    }

    /// The primary button's label, and whether it can be pressed.
    fn primary(&self) -> (&'static str, bool) {
        match self.step_kind() {
            Step::Welcome => ("Get started", true),
            Step::Model if self.is_model_missing() && !self.downloading => {
                ("Download", !self.loading)
            }
            Step::Model | Step::Folders => ("Continue", !self.loading),
            Step::Shortcut if self.downloading => ("Waiting for the model…", false),
            Step::Shortcut if self.busy == Some(Busy::Apply) => ("Starting…", false),
            Step::Shortcut => ("Start indexing", self.can_save()),
        }
    }

    /// Megabytes on disk of the model's two files, counting the one downloading now by its progress.
    fn downloaded_mb(&self) -> f64 {
        let current_is_media = self
            .progress
            .as_ref()
            .map(|p| p.message.contains("encoders"));
        let fraction = self.progress.as_ref().map_or(0., |p| p.fraction);
        let text = if self.text_model || current_is_media == Some(true) {
            TEXT_MB
        } else if current_is_media == Some(false) {
            TEXT_MB * fraction
        } else {
            0.
        };
        let media = if self.media_model {
            MEDIA_MB
        } else if current_is_media == Some(true) {
            MEDIA_MB * fraction
        } else {
            0.
        };
        text + media
    }

    // ---- keys and the footer -------------------------------------------------------------------------------------

    /// Handles a key; true when it was setup's.
    pub fn key(&mut self, key: &str, _window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.first_run {
            match key {
                "escape" => self.back(cx),
                "enter" => self.next(cx),
                "down" | "ctrl-n" | "ctrl-j" | "tab" => self.move_by(1, cx),
                "up" | "ctrl-p" | "ctrl-k" | "shift-tab" => self.move_by(-1, cx),
                "space" => {
                    if !self.rows().is_empty() {
                        self.toggle(cx);
                    }
                }
                _ => return false,
            }
            return true;
        }
        if self.confirm {
            match key {
                "escape" => self.keep_editing(cx),
                "enter" => self.save_and_leave(cx),
                "d" => self.discard(cx),
                _ => return false,
            }
            return true;
        }
        match key {
            "escape" => self.leave(cx),
            "down" | "ctrl-n" | "ctrl-j" | "tab" => self.move_by(1, cx),
            "up" | "ctrl-p" | "ctrl-k" | "shift-tab" => self.move_by(-1, cx),
            // In settings Enter acts on the row like space; Save is Ctrl+S.
            "space" | "enter" => self.toggle(cx),
            "ctrl-s" => self.save(cx),
            _ => return false,
        }
        true
    }

    /// The footer's left side while setup is up; `None` keeps the usual status.
    pub fn footer_note(&self, first_run: bool, cx: &App) -> Option<String> {
        if let Some(error) = &self.error {
            return Some(error.clone());
        }
        if first_run {
            return Some(self.onboarding_note());
        }
        match self.busy {
            Some(Busy::Apply) => return Some("Saving…".into()),
            Some(Busy::Pick { .. }) => return Some("Choosing a folder…".into()),
            None => {}
        }
        if self.is_dirty() {
            return Some(self.pending());
        }
        let _ = cx;
        self.message.as_ref().map(|m| format!("Saved · {m}"))
    }

    fn onboarding_note(&self) -> String {
        match self.step_kind() {
            Step::Welcome => "About two minutes to set up".into(),
            Step::Model => {
                if self
                    .progress
                    .as_ref()
                    .is_some_and(|p| p.message.starts_with("Checking"))
                {
                    "Checking the download…".into()
                } else if self.downloading {
                    format!(
                        "Downloading · {} %",
                        ((self.downloaded_mb() / (TEXT_MB + MEDIA_MB)) * 100.).round()
                    )
                } else if self.is_model_missing() {
                    "865 MB, downloaded once".into()
                } else {
                    "Already on this computer".into()
                }
            }
            Step::Folders => {
                if self.loading {
                    return "Looking at your folders…".into();
                }
                let on: Vec<&Folder> = self
                    .folders
                    .iter()
                    .chain(&self.code)
                    .filter(|f| f.selected)
                    .collect();
                if on.is_empty() {
                    return "Pick at least one folder".into();
                }
                let seconds: f64 = on.iter().map(|f| estimate(&f.summary)).sum();
                format!(
                    "{} {} · ~{} of indexing, newest first",
                    on.len(),
                    if on.len() == 1 { "folder" } else { "folders" },
                    span(seconds)
                )
            }
            Step::Shortcut => {
                if self.busy == Some(Busy::Apply) {
                    "Starting the indexer…".into()
                } else if self.downloading {
                    "Indexing starts when the model has downloaded".into()
                } else {
                    "Then Omafind starts reading your files".into()
                }
            }
        }
    }

    /// What saving would change, for the footer.
    fn pending(&self) -> String {
        let all = || self.folders.iter().chain(&self.code);
        let added: Vec<&Folder> = all().filter(|f| f.selected && !f.chosen).collect();
        let removed = all().filter(|f| !f.selected && f.chosen).count();
        if !self.configured {
            return match added.len() {
                0 => "Pick what to index".into(),
                1 => "1 folder to index".into(),
                n => format!("{n} folders to index"),
            };
        }
        if added.len() == 1 && removed == 0 {
            let time = added[0].summary.rsplit(" · ").next().unwrap_or("");
            return if time.starts_with('~') {
                format!("{} adds {time} of indexing", added[0].label)
            } else {
                format!("{} will be indexed", added[0].label)
            };
        }
        let mut parts = Vec::new();
        if !added.is_empty() {
            parts.push(format!(
                "adds {} {}",
                added.len(),
                if added.len() == 1 {
                    "folder"
                } else {
                    "folders"
                }
            ));
        }
        if removed > 0 {
            parts.push(format!("removes {removed} from the index"));
        }
        if parts.is_empty() {
            "Unsaved changes".into()
        } else {
            format!("Saving {}", parts.join(", "))
        }
    }

    pub fn hints(&self, first_run: bool) -> String {
        if first_run {
            return match self.step_kind() {
                Step::Welcome => "⏎ get started · esc later",
                Step::Model => "⏎ continue · esc back",
                _ => "↑↓ move · space toggle · ⏎ continue · esc back",
            }
            .into();
        }
        if self.confirm {
            return "⏎ save · d discard · esc keep editing".into();
        }
        "↑↓ move · space toggle · ⌃S save · esc back".into()
    }

    // ---- the leave dialog, drawn by the window over everything -------------------------------------------------------

    pub fn dialog(setup: &Entity<Setup>, first_run: bool, cx: &App) -> Option<AnyElement> {
        let this = setup.read(cx);
        if !this.confirm {
            return None;
        }
        let c = colors(cx);
        let all = || this.folders.iter().chain(&this.code);
        let added: Vec<String> = all()
            .filter(|f| f.selected && !f.chosen)
            .map(|f| f.label.clone())
            .collect();
        let removed: Vec<String> = all()
            .filter(|f| !f.selected && f.chosen)
            .map(|f| f.label.clone())
            .collect();
        let list = |xs: &[String]| {
            if xs.len() <= 2 {
                xs.join(" and ")
            } else {
                format!("{} folders", xs.len())
            }
        };
        let detail = if !this.configured {
            "Nothing is indexed until you start.".to_owned()
        } else if !added.is_empty() && removed.is_empty() {
            let one = added.len() == 1;
            format!(
                "{} {} added to the folders to index. Leaving now drops {}.",
                list(&added),
                if one { "was" } else { "were" },
                if one { "it" } else { "them" }
            )
        } else if !removed.is_empty() && added.is_empty() {
            let one = removed.len() == 1;
            format!(
                "{} would leave the index. Leaving now keeps {}.",
                list(&removed),
                if one { "it" } else { "them" }
            )
        } else {
            "Your changes aren't saved yet.".to_owned()
        };
        let can_save = this.can_save();
        let on = |f: fn(&mut Setup, &mut Context<Setup>)| {
            let setup = setup.clone();
            move |_: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut App| setup.update(cx, f)
        };
        Some(
            div()
                .id("leave-backdrop")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .justify_center()
                .items_center()
                .bg(alpha(c.background, 0.6))
                .child(
                    v_flex()
                        .id("leave")
                        .w(px(440.))
                        .px(px(22.))
                        .pt(px(20.))
                        .pb(px(18.))
                        .gap(px(10.))
                        .rounded(px(8.))
                        .border_1()
                        .border_color(c.border)
                        .bg(c.surface)
                        .shadow_lg()
                        .child(
                            div()
                                .text_size(px(15.))
                                .font_semibold()
                                .text_color(c.foreground)
                                .child(if first_run {
                                    "Start indexing?"
                                } else {
                                    "Save your changes?"
                                }),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(c.muted_foreground)
                                .whitespace_normal()
                                .child(detail),
                        )
                        .child(
                            h_flex()
                                .justify_end()
                                .gap(px(6.))
                                .mt(px(8.))
                                .child(
                                    BarButton::new("leave-keep", "Keep editing", "esc")
                                        .render(on(Setup::keep_editing), cx),
                                )
                                .child(
                                    BarButton::new("leave-discard", "Discard", "D")
                                        .kind(ButtonKind::Danger)
                                        .render(on(Setup::discard), cx),
                                )
                                .child(
                                    BarButton::new(
                                        "leave-save",
                                        if first_run { "Start" } else { "Save" },
                                        "⏎",
                                    )
                                    .kind(ButtonKind::Primary)
                                    .enabled(can_save)
                                    .render(on(Setup::save_and_leave), cx),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    // ---- the list both screens share -------------------------------------------------------------------------------

    fn render_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = colors(cx);
        if self.loading {
            return v_flex()
                .flex_1()
                .px(px(26.))
                .py(px(18.))
                .text_size(px(12.))
                .text_color(c.muted_foreground)
                .child("Looking at your folders…")
                .into_any_element();
        }
        let items = self.items();
        let sizes = Rc::new(
            items
                .iter()
                .map(|it| size(px(400.), px(it.height())))
                .collect::<Vec<_>>(),
        );
        let items = Rc::new(items);
        v_virtual_list(
            cx.entity(),
            "setup-list",
            sizes,
            move |this, range, _, cx| range.map(|i| this.render_item(items[i], cx)).collect(),
        )
        .flex_1()
        .min_w_0()
        .h_full()
        .px(px(14.))
        .py(px(6.))
        .track_scroll(&self.scroll)
        .into_any_element()
    }

    fn render_item(&self, item: Item, cx: &mut Context<Self>) -> AnyElement {
        let c = colors(cx);
        let (row, index) = match item {
            Item::Heading { title, height } => {
                return div()
                    .h(px(height))
                    .flex()
                    .items_end()
                    .pb(px(6.))
                    .px(px(12.))
                    .text_size(px(11.))
                    .text_color(c.muted_foreground)
                    .child(title.to_uppercase())
                    .into_any_element();
            }
            Item::Row { row, index } => (row, index),
        };
        let configured = self.configured && !self.first_run;
        let status = |f: &Folder| -> Option<(String, Hsla)> {
            if !configured {
                return None;
            }
            match (f.selected, f.chosen) {
                (true, true) => Some(("indexed".into(), c.muted_foreground)),
                (true, false) => Some(("new · will be read".into(), c.primary)),
                (false, true) => Some(("will be removed".into(), c.destructive)),
                (false, false) => None,
            }
        };
        let check = |on: bool| {
            if on {
                glyphs::CHECK_ON
            } else {
                glyphs::CHECK_OFF
            }
        };
        match row {
            Row::Folder(i) | Row::Code(i) => {
                let f = if matches!(row, Row::Folder(_)) {
                    &self.folders[i]
                } else {
                    &self.code[i]
                };
                self.row(
                    index,
                    &f.label,
                    &f.summary,
                    check(f.selected),
                    status(f),
                    !f.selected,
                    cx,
                )
            }
            Row::Add { code } => {
                let selected = index == self.cursor;
                let label = if self.busy == Some(Busy::Pick { code }) {
                    "Choosing…"
                } else if code {
                    "+ Add a code folder…"
                } else {
                    "+ Add a folder…"
                };
                h_flex()
                    .h(px(38.))
                    .px(px(12.))
                    .items_center()
                    .child(
                        clickable(if code { "add-code" } else { "add-folder" })
                            .h(px(28.))
                            .px(px(10.))
                            .flex()
                            .items_center()
                            .rounded(px(4.))
                            .border_1()
                            .border_dashed()
                            .border_color(if selected { c.primary } else { c.border })
                            .text_size(px(12.))
                            .text_color(if selected {
                                c.foreground
                            } else {
                                c.muted_foreground
                            })
                            .hover(move |st| st.text_color(c.foreground))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.cursor = index;
                                this.add(code, cx);
                            }))
                            .child(label),
                    )
                    .into_any_element()
            }
            Row::PowerSaver => {
                let on = self.power.pause_in_power_saver;
                self.row(
                    index,
                    "Pause in power-saver mode",
                    "Search keeps working",
                    check(on),
                    None,
                    !on,
                    cx,
                )
            }
            Row::Charger => {
                let on = self.power.wait_for_ac;
                self.row(
                    index,
                    "Only index on a charger",
                    "Waits while on battery",
                    check(on),
                    None,
                    !on,
                    cx,
                )
            }
            Row::Shortcut => {
                let have = self.binding;
                let label = if self.first_run {
                    "Add the shortcut"
                } else {
                    "SUPER + CTRL + G opens Omafind"
                };
                let caption = if have {
                    "In ~/.config/hypr/omafind.lua"
                } else if self.first_run {
                    "SUPER + CTRL + G, in ~/.config/hypr/omafind.lua"
                } else {
                    "Adds it to ~/.config/hypr/omafind.lua"
                };
                let mark = if have {
                    glyphs::CHECK
                } else {
                    check(self.shortcut)
                };
                let note = have.then(|| ("installed".to_owned(), c.muted_foreground));
                self.row(
                    index,
                    label,
                    caption,
                    mark,
                    note,
                    !have && !self.shortcut,
                    cx,
                )
            }
            Row::Models => {
                let missing = self.is_model_missing();
                let caption = match &self.progress {
                    Some(p) => p.message.clone(),
                    None if self.downloading => "Downloading…".into(),
                    None if missing => "865 MB, once · stays on this machine".into(),
                    None => "By Google DeepMind · Apache 2.0 · on this machine".into(),
                };
                let label = if !missing {
                    "EmbeddingGemma 2"
                } else if self.text_model {
                    "Image and audio encoders missing"
                } else {
                    "Download the model files"
                };
                let mark = if self.downloading {
                    glyphs::SPINNER
                } else if missing {
                    glyphs::DOWNLOAD
                } else {
                    glyphs::CHECK
                };
                let note = (!missing).then(|| ("installed".to_owned(), c.muted_foreground));
                v_flex()
                    .w_full()
                    .child(self.row(index, label, &caption, mark, note, false, cx))
                    .when_some(self.progress.as_ref(), |d, p| {
                        d.child(
                            div()
                                .mx(px(12.))
                                .h(px(3.))
                                .rounded(px(2.))
                                .bg(c.muted)
                                .child(
                                    div()
                                        .h(px(3.))
                                        .w(relative(p.fraction as f32))
                                        .rounded(px(2.))
                                        .bg(c.primary),
                                ),
                        )
                    })
                    .into_any_element()
            }
        }
    }

    /// A row of the list: its mark (a checkbox, a check, a download), a label and caption, a note at the end.
    #[allow(clippy::too_many_arguments)]
    fn row(
        &self,
        index: usize,
        label: &str,
        caption: &str,
        mark: &'static str,
        note: Option<(String, Hsla)>,
        dim: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = colors(cx);
        let selected = index == self.cursor;
        clickable(format!("setup-{index}"))
            .flex()
            .w_full()
            .h(px(46.))
            .px(px(12.))
            .gap(px(12.))
            .items_center()
            .rounded(px(6.))
            .bg(if selected { c.accent } else { c.background })
            .on_click(cx.listener(move |this, _, _, cx| this.click_row(index, cx)))
            .child(
                div()
                    .w(px(16.))
                    .text_size(px(15.))
                    .text_color(if mark == glyphs::CHECK_ON {
                        c.primary
                    } else {
                        c.muted_foreground
                    })
                    .child(mark),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(3.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(if dim {
                                c.muted_foreground
                            } else {
                                c.foreground
                            })
                            .when(selected, |d| d.font_medium())
                            .child(label.to_owned()),
                    )
                    .when(!caption.is_empty(), |d| {
                        d.child(
                            div()
                                .text_size(px(11.))
                                .text_color(c.muted_foreground)
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(caption.to_owned()),
                        )
                    }),
            )
            .when_some(note, |d, (text, color)| {
                d.child(
                    div()
                        .flex_none()
                        .text_size(px(11.))
                        .text_color(color)
                        .child(text),
                )
            })
            .into_any_element()
    }

    // ---- settings --------------------------------------------------------------------------------------------------

    fn render_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = colors(cx);
        let save_label = if self.busy == Some(Busy::Apply) {
            "Saving…"
        } else {
            "Save"
        };
        let bar = h_flex()
            .h(px(48.))
            .flex_none()
            .pl(px(16.))
            .pr(px(9.))
            .gap(px(10.))
            .items_center()
            .border_b_1()
            .border_color(c.border)
            .child(
                div()
                    .text_size(px(15.))
                    .text_color(c.muted_foreground)
                    .child(glyphs::GEAR),
            )
            .child(
                div()
                    .flex_1()
                    .text_size(px(15.))
                    .font_medium()
                    .text_color(c.foreground)
                    .child("Settings"),
            )
            .child(div().w(px(1.)).h(px(20.)).bg(c.border).mx(px(2.)))
            .child(
                h_flex()
                    .gap(px(6.))
                    .child(
                        BarButton::new("setup-back", "Back", "esc")
                            .render(cx.listener(|this, _, _, cx| this.leave(cx)), cx),
                    )
                    .child(
                        BarButton::new("setup-save", save_label, "⌃S")
                            .kind(ButtonKind::Primary)
                            .enabled(self.can_save())
                            .render(cx.listener(|this, _, _, cx| this.save(cx)), cx),
                    ),
            );
        v_flex()
            .flex_1()
            .min_h_0()
            .child(bar)
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_start()
                    .child(self.render_nav(cx))
                    .child(self.render_list(cx)),
            )
            .into_any_element()
    }

    fn render_nav(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = colors(cx);
        let rows = self.rows();
        let current = rows.get(self.cursor).map(|r| r.section());
        let indexed = cx.global::<AppState>().indexed();
        let mut stats = vec!["Files never leave this machine.".to_owned()];
        if self.configured {
            stats.insert(0, format!("{} files indexed", format::count(indexed)));
            stats.insert(1, format!("{} on disk", format::bytes(self.index_bytes)));
        }
        v_flex()
            .w(px(220.))
            .flex_none()
            .h_full()
            .px(px(6.))
            .py(px(10.))
            .gap(px(2.))
            .border_r_1()
            .border_color(c.border)
            .children(SECTIONS.iter().map(|&(section, label, _)| {
                let on = current == Some(section);
                let first = rows
                    .iter()
                    .position(|r| r.section() == section)
                    .unwrap_or(0);
                let picked = match section {
                    Section::Folders => Some(&self.folders),
                    Section::Code => Some(&self.code),
                    _ => None,
                }
                .filter(|list| !list.is_empty())
                .map(|list| {
                    format!(
                        "{} of {}",
                        list.iter().filter(|f| f.selected).count(),
                        list.len()
                    )
                });
                let hover = if on { c.accent } else { c.muted };
                clickable(format!("nav-{label}"))
                    .flex()
                    .h(px(34.))
                    .px(px(12.))
                    .justify_between()
                    .items_center()
                    .rounded(px(5.))
                    .text_size(px(13.))
                    .bg(if on { c.accent } else { c.background })
                    .text_color(if on { c.primary } else { c.foreground })
                    .when(on, |d| d.font_semibold())
                    .hover(move |st| st.bg(hover))
                    .on_click(cx.listener(move |this, _, _, cx| this.move_to(first, cx)))
                    .child(div().child(label))
                    .when_some(picked, |d, picked| {
                        d.child(
                            div()
                                .text_size(px(11.))
                                .font_normal()
                                .text_color(c.muted_foreground)
                                .child(picked),
                        )
                    })
            }))
            .child(div().flex_1())
            .child(
                v_flex()
                    .px(px(12.))
                    .py(px(10.))
                    .gap(px(3.))
                    .text_size(px(11.))
                    .text_color(c.muted_foreground)
                    .children(stats.into_iter().map(|line| div().child(line))),
            )
    }

    // ---- the onboarding ----------------------------------------------------------------------------------------------

    fn render_onboarding(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = colors(cx);
        let (label, enabled) = self.primary();
        let bar = h_flex()
            .h(px(48.))
            .flex_none()
            .pl(px(16.))
            .pr(px(9.))
            .gap(px(10.))
            .items_center()
            .border_b_1()
            .border_color(c.border)
            .child(
                div()
                    .text_size(px(15.))
                    .text_color(c.primary)
                    .child(glyphs::SEARCH),
            )
            .child(
                div()
                    .w(px(150.))
                    .text_size(px(15.))
                    .font_medium()
                    .text_color(c.foreground)
                    .child("Omafind"),
            )
            .child(
                h_flex()
                    .flex_1()
                    .justify_center()
                    .items_center()
                    .gap(px(8.))
                    .children(
                        STEPS
                            .iter()
                            .enumerate()
                            .map(|(i, (_, name))| self.step_mark(i, name, cx)),
                    ),
            )
            .child(
                h_flex()
                    .w(px(150.))
                    .justify_end()
                    .gap(px(6.))
                    .when(self.step > 0, |d| {
                        d.child(
                            BarButton::new("onboarding-back", "Back", "esc")
                                .render(cx.listener(|this, _, _, cx| this.back(cx)), cx),
                        )
                    })
                    .child(
                        BarButton::new("onboarding-next", label, "⏎")
                            .kind(ButtonKind::Primary)
                            .enabled(enabled)
                            .render(cx.listener(|this, _, _, cx| this.next(cx)), cx),
                    ),
            );
        let body = match self.step_kind() {
            Step::Welcome => self.render_welcome(cx).into_any_element(),
            Step::Model => self.render_model(cx).into_any_element(),
            Step::Folders => v_flex()
                .w(px(720.))
                .h_full()
                .pt(px(30.))
                .gap(px(14.))
                .child(div().px(px(12.)).child(heading(
                    "What should Omafind read?",
                    "The newest files go first, so search works within minutes. You can change this later in settings.",
                    cx,
                )))
                .child(v_flex().flex_1().min_h_0().child(self.render_list(cx)))
                .into_any_element(),
            Step::Shortcut => v_flex()
                .w(px(620.))
                .h_full()
                .justify_center()
                .gap(px(24.))
                .child(div().px(px(26.)).child(heading(
                    "Open it from anywhere",
                    "Omafind starts with your session and waits in the background, opening over whatever you're doing. \
                     Its window rules go in ~/.config/hypr/omafind.lua either way; the shortcut is up to you.",
                    cx,
                )))
                .child(
                    h_flex()
                        .px(px(26.))
                        .gap(px(8.))
                        .items_center()
                        .children(["SUPER", "+", "CTRL", "+", "G"].map(|k| keycap(k, cx))),
                )
                .child(v_flex().h(px(56.)).child(self.render_list(cx)))
                .into_any_element(),
        };
        v_flex()
            .flex_1()
            .min_h_0()
            .child(bar)
            .child(h_flex().flex_1().min_h_0().justify_center().child(body))
            .into_any_element()
    }

    fn step_mark(&self, i: usize, name: &'static str, cx: &App) -> impl IntoElement {
        let c = colors(cx);
        let at = self.step;
        let dot = div()
            .size(px(18.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(9.))
            .border_1()
            .border_color(if i <= at { c.primary } else { c.border })
            .bg(if i < at { c.primary } else { c.background })
            .text_size(px(10.))
            .text_color(if i < at {
                c.primary_foreground
            } else if i == at {
                c.primary
            } else {
                c.muted_foreground
            })
            .child(if i < at {
                glyphs::CHECK.to_owned()
            } else {
                (i + 1).to_string()
            });
        h_flex()
            .gap(px(8.))
            .items_center()
            .child(
                h_flex()
                    .gap(px(7.))
                    .items_center()
                    .text_size(px(12.))
                    .text_color(if i <= at {
                        c.foreground
                    } else {
                        c.muted_foreground
                    })
                    .when(i == at, |d| d.font_semibold())
                    .child(dot)
                    .child(div().child(name)),
            )
            .when(i < STEPS.len() - 1, |d| {
                d.child(div().w(px(28.)).h(px(1.)).bg(c.border))
            })
    }

    fn render_welcome(&self, cx: &App) -> impl IntoElement {
        let c = colors(cx);
        v_flex()
            .w(px(620.))
            .h_full()
            .justify_center()
            .gap(px(28.))
            .child(
                v_flex()
                    .gap(px(10.))
                    .child(
                        div()
                            .text_size(px(26.))
                            .font_semibold()
                            .text_color(c.foreground)
                            .child("Find files by describing them"),
                    )
                    .child(div().text_size(px(13.)).text_color(c.muted_foreground).whitespace_normal().child(
                        "Say what's in it, what it says or when it was. Omafind finds the photo, the page, the passage, \
                         or the moment in a recording.",
                    )),
            )
            .child(v_flex().gap(px(2.)).children(EXAMPLES.iter().enumerate().map(|(i, (query, at, tag, glyph))| {
                h_flex()
                    .gap(px(14.))
                    .px(px(12.))
                    .py(px(10.))
                    .items_center()
                    .rounded(px(6.))
                    .bg(if i == 0 { c.surface } else { c.background })
                    .child(
                        div()
                            .size(px(40.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.))
                            .border_1()
                            .border_color(c.border)
                            .text_size(px(16.))
                            .text_color(c.muted_foreground)
                            .child(*glyph),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .gap(px(3.))
                            .child(div().text_size(px(13.)).text_color(c.foreground).child(format!("\"{query}\"")))
                            .child(div().text_size(px(11.)).text_color(c.muted_foreground).child(*at)),
                    )
                    .child(
                        div()
                            .px(px(6.))
                            .rounded(px(3.))
                            .border_1()
                            .border_color(c.border)
                            .text_size(px(10.))
                            .text_color(c.muted_foreground)
                            .child(*tag),
                    )
            })))
            .child(
                h_flex()
                    .gap(px(10.))
                    .items_center()
                    .text_size(px(12.))
                    .text_color(c.muted_foreground)
                    .child(div().child(glyphs::LOCK))
                    .child(div().child("Everything runs on this computer. Your files are never uploaded.")),
            )
    }

    fn render_model(&self, cx: &App) -> impl IntoElement {
        let c = colors(cx);
        let total = TEXT_MB + MEDIA_MB;
        let done = self.downloaded_mb();
        let right = if !self.is_model_missing() {
            format!("{total} MB · done")
        } else if self.downloading {
            format!("{} / {total} MB", done.round())
        } else {
            format!("{total} MB")
        };
        v_flex()
            .w(px(620.))
            .h_full()
            .justify_center()
            .gap(px(18.))
            .child(div().text_size(px(22.)).font_semibold().text_color(c.foreground).child("Download the model"))
            .child(
                h_flex()
                    .gap(px(14.))
                    .p(px(16.))
                    .items_center()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(c.border)
                    .bg(c.surface)
                    .text_size(px(12.))
                    .child(div().w(px(170.)).font_medium().text_color(c.foreground).child("EmbeddingGemma 2"))
                    .child(
                        div()
                            .flex_1()
                            .h(px(6.))
                            .rounded(px(3.))
                            .bg(c.muted)
                            .child(div().h(px(6.)).rounded(px(3.)).bg(c.primary).w(relative((done / total) as f32))),
                    )
                    .child(div().w(px(110.)).flex().justify_end().text_color(c.muted_foreground).child(right)),
            )
            .child(
                v_flex()
                    .px(px(2.))
                    .gap(px(6.))
                    .text_size(px(11.))
                    .text_color(c.muted_foreground)
                    .child(div().whitespace_normal().child(
                        "Reads text, pictures and sound. Downloaded once, runs on this computer. You can pick folders \
                         while it downloads.",
                    ))
                    .child(div().whitespace_normal().child(
                        "By Google DeepMind, under the Apache License 2.0. From Hugging Face \
                         (ggml-org/embeddinggemma-2-GGUF), checked against its SHA-256.",
                    ))
                    .child(div().child("Saved in ~/.local/share/omafind/models")),
            )
    }
}

impl Render for Setup {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.first_run {
            self.render_onboarding(cx)
        } else {
            self.render_settings(cx)
        }
    }
}

fn heading(title: &str, text: &str, cx: &App) -> impl IntoElement {
    let c = colors(cx);
    v_flex()
        .gap(px(10.))
        .child(
            div()
                .text_size(px(22.))
                .font_semibold()
                .text_color(c.foreground)
                .child(title.to_owned()),
        )
        .child(
            div()
                .text_size(px(13.))
                .text_color(c.muted_foreground)
                .whitespace_normal()
                .child(text.to_owned()),
        )
}

fn keycap(key: &'static str, cx: &App) -> AnyElement {
    let c = colors(cx);
    if key == "+" {
        return div()
            .text_color(c.muted_foreground)
            .child("+")
            .into_any_element();
    }
    let strong = key == "G";
    div()
        .px(px(14.))
        .py(px(10.))
        .rounded(px(6.))
        .border_1()
        .border_b_3()
        .border_color(if strong { c.primary } else { c.border })
        .bg(c.surface)
        .text_size(px(15.))
        .font_semibold()
        .text_color(if strong { c.primary } else { c.foreground })
        .child(key)
        .into_any_element()
}

/// "4 photos · ~3 s" → seconds; summaries end in the survey's estimate.
fn estimate(summary: &str) -> f64 {
    let Some(at) = summary.rfind('~') else {
        return 0.;
    };
    let mut parts = summary[at + 1..].split_whitespace();
    let (Some(n), Some(unit)) = (parts.next(), parts.next()) else {
        return 0.;
    };
    let n: f64 = n.parse().unwrap_or(0.);
    n * match unit {
        "s" => 1.,
        "min" => 60.,
        "h" => 3600.,
        _ => 0.,
    }
}

fn span(seconds: f64) -> String {
    if seconds < 60. {
        format!("{} s", seconds.round())
    } else if seconds < 3600. {
        format!("{} min", (seconds / 60.).round())
    } else {
        format!("{:.1} h", seconds / 3600.)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimates_read_the_survey_summary() {
        assert_eq!(estimate("4 screenshots, 12 photos · ~17 s"), 17.);
        assert_eq!(estimate("569 videos · ~1.3 h"), 1.3 * 3600.);
        assert_eq!(estimate("nothing to index"), 0.);
        assert_eq!(span(40.), "40 s");
        assert_eq!(span(330.), "6 min");
    }
}

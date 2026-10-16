// The first-run and settings screen: what to index, power, the desktop hooks, the model files. Same shape as the search
// screen so it feels like one thing: a bar with two buttons, rows the cursor moves over, space toggles. Nothing is
// written until Save; leaving with changes asks first.

import { div } from "gpui-kit";
import { v_flex, h_flex, v_virtual_list, VirtualListScrollHandle } from "gpui-base";
import { candidates, applySetup, downloadModels, setupProgress, pickFolder, surveyFolder, status } from "omafind";
import { count, bytes, alpha } from "./kinds.js";

const CHECK_ON = "\u{f14a}";
const CHECK_OFF = "\u{f096}";
const GEAR = "\u{f013}";

const SECTIONS = [
  { key: "folders", label: "Folders", title: "Folders to index" },
  { key: "code", label: "Code", title: "Code · searched with a code: prefix" },
  { key: "power", label: "Power", title: "Power" },
  { key: "desktop", label: "Desktop", title: "Desktop" },
  { key: "model", label: "Model", title: "Model" },
];

export class Setup {
  constructor() {
    this.loading = true;
    this.folders = [];
    this.code = [];
    this.power = { pauseInPowerSaver: true, waitForAc: false };
    this.shortcut = true;
    this.textModel = true;
    this.mediaModel = true;
    this.configured = false;
    this.indexBytes = 0;
    this.cursor = 0;
    this.busy = null;
    this.error = null;
    this.message = null;
    /** Leaving with unsaved changes: { then(cx) } runs after Save or Discard. */
    this.confirm = null;
    this.saved = "";
    /** Sections shown, for the onboarding's steps; null = all (settings). */
    this.only = null;
    this.headings = true;
    this.downloading = false;
    this.scroll = VirtualListScrollHandle.new();
  }

  /** Loads the candidates; `cx` is an AsyncContext. */
  load(cx) {
    this.loading = true;
    this.confirm = null;
    this.message = null;
    cx.spawn(async (cx) => {
      try {
        const c = await candidates();
        this.folders = c.folders;
        this.code = c.code;
        this.power = c.power;
        this.configured = c.configured;
        this.indexBytes = c.indexBytes;
        this.textModel = c.textModel;
        this.mediaModel = c.mediaModel;
        // Offered ticked on a first run; in settings, only added when ticked. (The window rules and autostart always go
        // in; the shortcut is the only choice.)
        this.shortcut = !c.configured && !c.binding;
        this.installed = { binding: c.binding };
        this.error = null;
      } catch (error) {
        this.error = String(error);
      }
      this.saved = this.snapshot();
      this.loading = false;
      this.cursor = 0;
      cx.notify();
    });
  }

  /** Everything Save writes, to tell whether anything changed. */
  snapshot() {
    const on = (list) => list.filter((f) => f.selected).map((f) => f.path).sort();
    return JSON.stringify([on(this.folders), on(this.code), this.power, this.shortcut]);
  }

  dirty() {
    return !this.loading && this.snapshot() !== this.saved;
  }

  /** Save is offered when it would do something: a change, or on a first run, a start. */
  canSave(firstRun) {
    if (this.loading || this.busy || this.downloading) return false;
    return firstRun || this.dirty();
  }

  /** The rows the cursor walks, in order, each in a section. */
  rows() {
    const rows = [];
    for (let i = 0; i < this.folders.length; i++) rows.push({ kind: "folder", section: "folders", index: i });
    rows.push({ kind: "add", section: "folders", code: false });
    for (let i = 0; i < this.code.length; i++) rows.push({ kind: "code", section: "code", index: i });
    rows.push({ kind: "add", section: "code", code: true });
    rows.push({ kind: "powerSaver", section: "power" });
    rows.push({ kind: "charger", section: "power" });
    rows.push({ kind: "shortcut", section: "desktop" });
    rows.push({ kind: "models", section: "model" });
    return this.only ? rows.filter((r) => this.only.includes(r.section)) : rows;
  }

  /** The list, flat: section headings and rows, each with its height, rows numbered as `rows()` does. */
  items() {
    const out = [];
    let section = null;
    this.rows().forEach((r, i) => {
      if (r.section !== section && this.headings !== false) {
        section = r.section;
        out.push({ kind: "section", label: SECTIONS.find((s) => s.key === section).title, height: out.length ? 40 : 30 });
      }
      out.push({ ...r, row: i, height: r.kind === "add" ? 38 : 46 });
    });
    return out;
  }

  move(delta, cx) {
    const n = this.rows().length;
    this.moveTo(Math.max(0, Math.min(n - 1, this.cursor + delta)), cx);
  }

  moveTo(row, cx) {
    this.cursor = row;
    const items = this.items();
    const item = items.findIndex((it) => it.row === row);
    // The section heading too, when the row is a section's first.
    if (item >= 0) this.scroll.scroll_to_item(item > 0 && items[item - 1].kind === "section" ? item - 1 : item);
    cx.notify();
  }

  toggle(cx) {
    const row = this.rows()[this.cursor];
    if (!row) return;
    switch (row.kind) {
      case "folder": this.folders[row.index].selected = !this.folders[row.index].selected; break;
      case "code": this.code[row.index].selected = !this.code[row.index].selected; break;
      case "powerSaver": this.power = { ...this.power, pauseInPowerSaver: !this.power.pauseInPowerSaver }; break;
      case "charger": this.power = { ...this.power, waitForAc: !this.power.waitForAc }; break;
      case "shortcut": if (!this.has("shortcut")) this.shortcut = !this.shortcut; break;
      case "add": this.add(row.code, cx); return;
      case "models": if (!this.textModel || !this.mediaModel) this.download(cx); return;
    }
    cx.notify();
  }

  has(what) {
    const i = this.installed;
    if (!i) return false;
    return what === "shortcut" ? i.binding : false;
  }

  /** Asks the desktop for a folder and adds it, ticked, with what it holds. */
  add(code, cx) {
    if (this.busy) return;
    this.busy = "pick";
    this.picking = code;
    cx.notify();
    cx.spawn(async (cx) => {
      try {
        const path = await pickFolder();
        if (path) {
          const list = code ? this.code : this.folders;
          const known = list.findIndex((f) => f.path === path);
          if (known >= 0) list[known].selected = true;
          else list.push(await surveyFolder(path, code));
          const at = this.rows().findIndex((r) => (code ? r.kind === "code" : r.kind === "folder") && list[r.index].path === path);
          if (at >= 0) this.cursor = at;
        }
        this.error = null;
      } catch (error) {
        this.error = String(error);
      }
      this.busy = null;
      cx.notify();
    });
  }

  /** Its own flag rather than `busy`: folders can be picked while it runs. */
  download(cx) {
    if (this.downloading) return;
    this.downloading = true;
    cx.spawn(async (cx) => {
      try {
        await downloadModels();
        this.textModel = true;
        this.mediaModel = true;
        this.error = null;
      } catch (error) {
        this.error = String(error);
      }
      this.downloading = false;
      cx.notify();
    });
    cx.notify();
  }

  /** Saves and starts; `done(message, cx)` is called when it worked. */
  apply(cx, firstRun, done) {
    if (!this.canSave(firstRun)) return;
    const folders = this.folders.filter((f) => f.selected).map((f) => f.path);
    const code = this.code.filter((f) => f.selected).map((f) => f.path);
    if (folders.length === 0 && code.length === 0) {
      this.error = "Pick at least one folder";
      cx.notify();
      return;
    }
    if (!this.textModel) {
      this.error = "Download the model files first";
      cx.notify();
      return;
    }
    this.busy = "apply";
    cx.spawn(async (cx) => {
      try {
        const message = await applySetup({ folders, code, power: this.power, shortcut: this.shortcut });
        this.error = null;
        this.message = message;
        this.busy = null;
        for (const f of [...this.folders, ...this.code]) f.chosen = f.selected;
        this.configured = true;
        this.saved = this.snapshot();
        cx.notify();
        if (done) done(message, cx);
      } catch (error) {
        this.error = String(error);
        this.busy = null;
        this.confirm = null;
        cx.notify();
      }
    });
    cx.notify();
  }

  /** Save from the bar or Ctrl+S: a first run moves on to searching; settings stay up, Save greyed again. */
  save(cx, firstRun) {
    this.apply(cx, firstRun, firstRun ? this.onSaved : null);
  }

  /** Runs `then` now, or once the user says what to do with the changes. */
  leave(cx, then) {
    if (!this.dirty()) {
      then(cx);
      return;
    }
    this.confirm = { then };
    cx.notify();
  }

  /** The confirm dialog's answers. */
  keepEditing(cx) {
    this.confirm = null;
    cx.notify();
  }

  discard(cx) {
    const then = this.confirm?.then;
    this.confirm = null;
    this.load(cx);
    if (then) then(cx);
  }

  saveAndLeave(cx, firstRun) {
    const then = this.confirm?.then;
    this.apply(cx, firstRun, (_message, cx) => {
      this.confirm = null;
      if (then) then(cx);
    });
  }

  /** What saving would change, for the footer. */
  pending() {
    const added = [...this.folders, ...this.code].filter((f) => f.selected && !f.chosen);
    const removed = [...this.folders, ...this.code].filter((f) => !f.selected && f.chosen);
    if (!this.configured) {
      const n = added.length;
      return n ? `${n} ${n === 1 ? "folder" : "folders"} to index` : "Pick what to index";
    }
    if (added.length === 1 && removed.length === 0) {
      const time = added[0].summary.split(" · ").pop();
      return time.startsWith("~") ? `${added[0].label} adds ${time} of indexing` : `${added[0].label} will be indexed`;
    }
    const parts = [];
    if (added.length) parts.push(`adds ${added.length} ${added.length === 1 ? "folder" : "folders"}`);
    if (removed.length) parts.push(`removes ${removed.length} from the index`);
    return parts.length ? `Saving ${parts.join(", ")}` : "Unsaved changes";
  }

  hints() {
    if (this.confirm) return "⏎ save · d discard · esc keep editing";
    return "↑↓ move · space toggle · ⌃S save · esc back";
  }

  /** The footer's left side while this screen is up; null to keep the usual status. */
  footerNote(firstRun) {
    if (this.error) return this.error;
    if (this.busy === "apply") return "Saving…";
    if (this.busy === "pick") return "Choosing a folder…";
    if (firstRun || this.dirty()) return this.pending();
    if (this.message) return `Saved · ${this.message}`;
    return null;
  }

  render(cx, firstRun, onBack) {
    const colors = cx.theme().colors;
    return v_flex()
      .flex_1()
      .min_h_0()
      .child(this.bar(cx, firstRun, onBack))
      .child(
        h_flex()
          .flex_1()
          .min_h_0()
          .items_start()
          .child(this.nav(cx))
          .child(this.list(cx, firstRun)),
      );
  }

  bar(cx, firstRun, onBack) {
    const c = cx.theme().colors;
    const enabled = this.canSave(firstRun);
    const saveLabel = this.busy === "apply" ? "Saving…" : firstRun ? "Start indexing" : "Save";
    return h_flex()
      .h(48)
      .flex_none()
      .pl(16)
      .pr(9)
      .gap(10)
      .items_center()
      .border_b(1)
      .border_color(c.border)
      .child(div().text_size(15).text_color(c.muted_foreground).child(GEAR))
      .child(div().flex_1().text_size(15).font_medium().text_color(c.foreground).child(firstRun ? "Set up Omafind" : "Settings"))
      .child(div().w(1).h(20).bg(c.border).mx(2))
      .child(
        h_flex().gap(6)
          .child(button(firstRun ? "Close" : "Back", "esc", false, true, (cx) => onBack(cx), cx))
          .child(button(saveLabel, firstRun ? "⏎" : "⌃S", true, enabled, (cx) => this.save(cx, firstRun), cx)),
      );
  }

  nav(cx) {
    const c = cx.theme().colors;
    const rows = this.rows();
    const current = rows[this.cursor]?.section;
    const s = status();
    const stats = this.configured
      ? [`${count(s.indexed)} files indexed`, `${bytes(this.indexBytes)} on disk`, "Files never leave this machine."]
      : ["Files never leave this machine."];
    return v_flex()
      .w(220)
      .flex_none()
      .h_full()
      .px(6)
      .py(10)
      .gap(2)
      .border_r(1)
      .border_color(c.border)
      .children(
        SECTIONS.map((sec) => {
          const on = sec.key === current;
          const first = rows.findIndex((r) => r.section === sec.key);
          const picked = sec.key === "folders" ? this.folders : sec.key === "code" ? this.code : null;
          return h_flex()
            .id(`nav-${sec.key}`)
            .h(34)
            .px(12)
            .justify_between()
            .items_center()
            .rounded(5)
            .text_size(13)
            .bg(on ? c.accent : c.background)
            .text_color(on ? c.primary : c.foreground)
            .when(on, (d) => d.font_semibold())
            .hover((st) => st.bg(on ? c.accent : c.muted))
            .on_click((_e, cx) => this.moveTo(first, cx))
            .child(div().child(sec.label))
            .child(picked && picked.length ? div().text_size(11).font_normal().text_color(c.muted_foreground).child(`${picked.filter((f) => f.selected).length} of ${picked.length}`) : div());
        }),
      )
      .child(div().flex_1())
      .child(v_flex().px(12).py(10).gap(3).text_size(11).text_color(c.muted_foreground).children(stats.map((line) => div().child(line))));
  }

  list(cx, firstRun) {
    const c = cx.theme().colors;
    if (this.loading) {
      return v_flex().flex_1().px(26).py(18).text_size(12).text_color(c.muted_foreground).child("Looking at your folders…");
    }
    const items = this.items();
    const progress = setupProgress();
    const configured = this.configured && !firstRun;
    const status = (f) => {
      if (!configured) return null;
      if (f.selected && f.chosen) return { text: "indexed", color: c.muted_foreground };
      if (f.selected) return { text: "new · will be read", color: c.primary };
      if (f.chosen) return { text: "will be removed", color: c.destructive };
      return null;
    };
    const row = (i, h, left, caption, mark, note, onClick, dim) => {
      const selected = i === this.cursor;
      return h_flex()
        .id(`setup-${i}`)
        .w_full()
        .h(h)
        .px(12)
        .gap(12)
        .items_center()
        .rounded(6)
        .bg(selected ? c.accent : c.background)
        .on_click((_e, cx) => {
          this.cursor = i;
          onClick(cx);
        })
        .child(div().w(16).text_size(15).text_color(mark === CHECK_ON ? c.primary : c.muted_foreground).child(mark))
        .child(
          v_flex().flex_1().min_w_0().gap(3)
            .child(div().text_size(13).text_color(dim ? c.muted_foreground : c.foreground).when(selected, (d) => d.font_medium()).child(left))
            .child(caption ? div().text_size(11).text_color(c.muted_foreground).overflow_hidden().whitespace_nowrap().text_ellipsis().child(caption) : div()),
        )
        .child(note ? div().flex_none().text_size(11).text_color(note.color).child(note.text) : div());
    };
    const toggle = (cx) => this.toggle(cx);
    const render = (it) => {
      switch (it.kind) {
        case "section":
          return div().h(it.height).flex().items_end().pb(6).px(12).text_size(11).text_color(c.muted_foreground).child(it.label.toUpperCase());
        case "folder":
        case "code": {
          const f = (it.kind === "folder" ? this.folders : this.code)[it.index];
          return row(it.row, 46, f.label, f.summary, f.selected ? CHECK_ON : CHECK_OFF, status(f), toggle, !f.selected);
        }
        case "add": {
          const selected = it.row === this.cursor;
          return h_flex().h(38).px(12).items_center().child(
            div()
              .id(`add-${it.code ? "code" : "folder"}`)
              .h(28)
              .px(10)
              .flex()
              .items_center()
              .rounded(4)
              .border(1)
              .border_dashed()
              .border_color(selected ? c.primary : c.border)
              .text_size(12)
              .text_color(selected ? c.foreground : c.muted_foreground)
              .hover((st) => st.text_color(c.foreground))
              .on_click((_e, cx) => {
                this.cursor = it.row;
                this.add(it.code, cx);
              })
              .child(this.busy === "pick" && this.picking === it.code ? "Choosing…" : it.code ? "+ Add a code folder…" : "+ Add a folder…"),
          );
        }
        case "powerSaver":
          return row(it.row, 46, "Pause in power-saver mode", "Search keeps working", this.power.pauseInPowerSaver ? CHECK_ON : CHECK_OFF, null, toggle, !this.power.pauseInPowerSaver);
        case "charger":
          return row(it.row, 46, "Only index on a charger", "Waits while on battery", this.power.waitForAc ? CHECK_ON : CHECK_OFF, null, toggle, !this.power.waitForAc);
        case "shortcut": {
          const have = this.has("shortcut");
          const label = firstRun ? "Add the shortcut" : "SUPER + CTRL + G opens Omafind";
          const caption = have ? "In ~/.config/hypr/omafind.lua" : firstRun ? "SUPER + CTRL + G, in ~/.config/hypr/omafind.lua" : "Adds it to ~/.config/hypr/omafind.lua";
          return row(it.row, 46, label, caption, have ? "\u{f00c}" : this.shortcut ? CHECK_ON : CHECK_OFF, have ? { text: "installed", color: c.muted_foreground } : null, toggle, !have && !this.shortcut);
        }
        case "models": {
          const missing = !this.textModel || !this.mediaModel;
          const caption = progress ? progress.message : this.downloading ? "Downloading…" : missing ? "865 MB, once · stays on this machine" : "Text, image and audio encoders on this machine";
          return v_flex().w_full()
            .child(row(it.row, 46, missing ? (this.textModel ? "Image and audio encoders missing" : "Download the model files") : "EmbeddingGemma 2", caption, this.downloading ? "\u{f110}" : missing ? "\u{f019}" : "\u{f00c}", missing ? null : { text: "installed", color: c.muted_foreground }, toggle, false))
            .child(progress ? div().mx(12).h(3).rounded(2).bg(c.muted).child(div().h(3).w(`${Math.round(progress.fraction * 100)}%`).rounded(2).bg(c.primary)) : div());
        }
      }
      return div();
    };
    return v_virtual_list("setup-list", items.length, items.map((it) => it.height), (i) => `s-${i}`, (range, _cx) => {
      const out = [];
      for (let i = range.start; i < range.end; i++) out.push(render(items[i]));
      return out;
    }).flex_1().min_w_0().h_full().px(14).py(6).track_scroll(this.scroll);
  }

  /** The leave-with-changes dialog, over the whole window. */
  dialog(cx, firstRun) {
    const c = cx.theme().colors;
    const added = [...this.folders, ...this.code].filter((f) => f.selected && !f.chosen).map((f) => f.label);
    const removed = [...this.folders, ...this.code].filter((f) => !f.selected && f.chosen).map((f) => f.label);
    const list = (xs) => (xs.length <= 2 ? xs.join(" and ") : `${xs.length} folders`);
    let detail;
    if (!this.configured) detail = "Nothing is indexed until you start.";
    else if (added.length && !removed.length) detail = `${list(added)} ${added.length === 1 ? "was" : "were"} added to the folders to index. Leaving now drops ${added.length === 1 ? "it" : "them"}.`;
    else if (removed.length && !added.length) detail = `${list(removed)} would leave the index. Leaving now keeps ${removed.length === 1 ? "it" : "them"}.`;
    else detail = "Your changes aren't saved yet.";
    return div()
      .absolute()
      .top(0)
      .left(0)
      .size_full()
      .flex()
      .justify_center()
      .items_center()
      .bg(alpha(c.background, 0.6))
      .child(
        v_flex()
          .id("leave")
          .w(440)
          .px(22)
          .pt(20)
          .pb(18)
          .gap(10)
          .rounded(8)
          .border(1)
          .border_color(c.border)
          .bg(c.surface)
          .shadow_lg()
          .on_click((_e, _cx) => {})
          .child(div().text_size(15).font_semibold().text_color(c.foreground).child(firstRun ? "Start indexing?" : "Save your changes?"))
          .child(div().text_size(12).text_color(c.muted_foreground).whitespace_normal().child(detail))
          .child(
            h_flex().justify_end().gap(6).mt(8)
              .child(button("Keep editing", "esc", false, true, (cx) => this.keepEditing(cx), cx))
              .child(button("Discard", "D", "danger", true, (cx) => this.discard(cx), cx))
              .child(button(firstRun ? "Start" : "Save", "⏎", true, this.canSave(firstRun), (cx) => this.saveAndLeave(cx, firstRun), cx)),
          ),
      );
  }
}

/** A bar button with its key: `kind` true = primary, "danger", false = plain; a disabled one is drawn faint and ignores clicks. */
export function button(label, keys, kind, enabled, run, cx) {
  const c = cx.theme().colors;
  const primary = kind === true && enabled;
  const danger = kind === "danger";
  const fg = !enabled ? alpha(c.muted_foreground, 0.6) : primary ? c.primary_foreground : danger ? c.destructive : c.foreground;
  const edge = danger ? alpha(c.destructive, 0.5) : c.border;
  const keycap = div()
    .h(16)
    .min_w(16)
    .px(4)
    .flex()
    .items_center()
    .justify_center()
    .rounded(3)
    .text_size(10)
    .bg(primary ? alpha(c.primary_foreground, 0.18) : c.background)
    .border(primary ? 0 : 1)
    .border_color(edge)
    .text_color(primary ? c.primary_foreground : danger ? alpha(c.destructive, 0.8) : alpha(c.muted_foreground, enabled ? 1 : 0.6))
    .child(keys);
  return h_flex()
    .id(`btn-${label}`)
    .h(30)
    .pl(12)
    .pr(7)
    .gap(8)
    .items_center()
    .rounded(5)
    .text_size(12)
    .bg(primary ? c.primary : enabled ? c.background : c.surface)
    .border(primary ? 0 : 1)
    .border_color(edge)
    .text_color(fg)
    .when(kind === true, (d) => d.font_semibold())
    .when(enabled, (d) => d.hover((st) => st.bg(primary ? alpha(c.primary, 0.85) : c.muted)).on_click((_e, cx) => run(cx)))
    .child(div().child(label))
    .child(keycap);
}

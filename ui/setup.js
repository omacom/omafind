// The first-run and settings screen: what to index, the model files, the desktop hooks. Same shape as the search
// screen so it feels like one thing: rows the cursor moves over, space toggles, Enter starts.

import { div } from "gpui-kit";
import { v_flex, h_flex, v_virtual_list, VirtualListScrollHandle } from "gpui-base";
import { candidates, applySetup, downloadModels, setupProgress } from "dig";
import { count } from "./kinds.js";

const CHECK_ON = "\u{f14a}";
const CHECK_OFF = "\u{f096}";

export class Setup {
  constructor() {
    this.loading = true;
    this.folders = [];
    this.code = [];
    this.hyprland = true;
    this.autostart = true;
    this.textModel = true;
    this.mediaModel = true;
    this.cursor = 0;
    this.busy = null;
    this.error = null;
    this.message = null;
    this.scroll = VirtualListScrollHandle.new();
  }

  /** Loads the candidates; `cx` is an AsyncContext. */
  load(cx) {
    this.loading = true;
    cx.spawn(async (cx) => {
      try {
        const c = await candidates();
        this.folders = c.folders;
        this.code = c.code;
        this.textModel = c.textModel;
        this.mediaModel = c.mediaModel;
        this.hyprland = !(c.binding && c.rules);
        this.autostart = !c.autostart;
        this.installed = { binding: c.binding, rules: c.rules, autostart: c.autostart };
        this.error = null;
      } catch (error) {
        this.error = String(error);
      }
      this.loading = false;
      this.cursor = 0;
      cx.notify();
    });
  }

  /** The rows the cursor walks, in order. */
  rows() {
    const rows = [];
    if (!this.textModel || !this.mediaModel) rows.push({ kind: "models" });
    for (let i = 0; i < this.folders.length; i++) rows.push({ kind: "folder", index: i });
    for (let i = 0; i < this.code.length; i++) rows.push({ kind: "code", index: i });
    rows.push({ kind: "hyprland" });
    rows.push({ kind: "autostart" });
    rows.push({ kind: "apply" });
    return rows;
  }

  move(delta, cx) {
    const n = this.rows().length;
    this.cursor = Math.max(0, Math.min(n - 1, this.cursor + delta));
    // The list item holding the cursor row (sections sit between rows).
    const item = this.items().findIndex((it) => it.row === this.cursor);
    if (item >= 0) this.scroll.scroll_to_item(item);
    cx.notify();
  }

  /** The list, flat: sections, rows and the button, each with its height, rows numbered as `rows()` does. */
  items() {
    const out = [];
    let i = 0;
    for (const r of this.rows()) {
      if (r.kind === "models" && i === 0) out.push({ kind: "section", label: "Model", height: 30 });
      if (r.kind === "folder" && r.index === 0) out.push({ kind: "section", label: "Folders to index", height: 30 });
      if (r.kind === "code" && r.index === 0) out.push({ kind: "section", label: "Code, searched with a code: prefix", height: 30 });
      if (r.kind === "hyprland") out.push({ kind: "section", label: "Desktop", height: 30 });
      out.push({ kind: r.kind, index: r.index, row: i, height: r.kind === "apply" ? 70 : 40 });
      i += 1;
    }
    return out;
  }

  toggle(cx) {
    const row = this.rows()[this.cursor];
    if (!row) return;
    switch (row.kind) {
      case "folder": this.folders[row.index].selected = !this.folders[row.index].selected; break;
      case "code": this.code[row.index].selected = !this.code[row.index].selected; break;
      case "hyprland": this.hyprland = !this.hyprland; break;
      case "autostart": this.autostart = !this.autostart; break;
      case "models": this.download(cx); return;
      case "apply": this.apply(cx); return;
    }
    cx.notify();
  }

  download(cx) {
    if (this.busy) return;
    this.busy = "download";
    cx.spawn(async (cx) => {
      try {
        await downloadModels();
        this.textModel = true;
        this.mediaModel = true;
        this.error = null;
      } catch (error) {
        this.error = String(error);
      }
      this.busy = null;
      cx.notify();
    });
    cx.notify();
  }

  /** Saves and starts; `done(message)` is called when it worked. */
  apply(cx, done) {
    if (this.busy) return;
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
    this.onDone = done;
    cx.spawn(async (cx) => {
      try {
        const message = await applySetup({ folders, code, hyprland: this.hyprland, autostart: this.autostart });
        this.error = null;
        this.message = message;
        this.busy = null;
        cx.notify();
        if (this.onDone) this.onDone(message, cx);
      } catch (error) {
        this.error = String(error);
        this.busy = null;
        cx.notify();
      }
    });
    cx.notify();
  }

  render(cx, firstRun) {
    const colors = cx.theme().colors;
    const progress = setupProgress();
    const items = this.items();
    const section = (label) => div().h(30).pt(10).pb(2).px(12).text_size(11).text_color(colors.muted_foreground).child(label.toUpperCase());
    const row = (i, left, caption, right, onClick) => {
      const selected = i === this.cursor;
      return h_flex()
        .id(`setup-${i}`)
        .w_full()
        .h(40)
        .px(12)
        .gap(12)
        .items_center()
        .rounded(cx.theme().radius.md)
        .bg(selected ? colors.accent : colors.background)
        .on_click((_e, cx) => { this.cursor = i; onClick(cx); })
        .child(div().w(20).text_size(15).text_color(selected ? colors.accent_foreground : colors.muted_foreground).child(right))
        .child(
          v_flex().flex_1().min_w_0().gap(2)
            .child(div().text_size(13).text_color(selected ? colors.accent_foreground : colors.foreground).child(left))
            .child(caption ? div().text_size(11).text_color(colors.muted_foreground).overflow_hidden().whitespace_nowrap().text_ellipsis().child(caption) : div()),
        );
    };
    const render = (it) => {
      switch (it.kind) {
        case "section": return section(it.label);
        case "models": {
          const caption = progress ? progress.message : this.busy === "download" ? "Downloading…" : "EmbeddingGemma 2 · 865 MB, once · stays on this machine";
          return v_flex().w_full()
            .child(row(it.row, this.textModel ? "Image and audio encoders missing" : "Download the model files", caption, this.busy === "download" ? "\u{f110}" : "\u{f019}", (cx) => this.download(cx)))
            .child(progress ? div().mx(12).h(3).rounded(2).bg(colors.muted).child(div().h(3).w(`${Math.round(progress.fraction * 100)}%`).rounded(2).bg(colors.primary)) : div());
        }
        case "folder": {
          const f = this.folders[it.index];
          return row(it.row, f.label, f.summary, f.selected ? CHECK_ON : CHECK_OFF, (cx) => this.toggle(cx));
        }
        case "code": {
          const f = this.code[it.index];
          return row(it.row, f.label, f.summary, f.selected ? CHECK_ON : CHECK_OFF, (cx) => this.toggle(cx));
        }
        case "hyprland": {
          const have = this.installed && this.installed.binding && this.installed.rules;
          return row(it.row, "SUPER + CTRL + G opens Dig", have ? "Keybinding and window rules are in ~/.config/hypr/dig.lua" : "Adds a keybinding and window rules to ~/.config/hypr/dig.lua", have ? "\u{f00c}" : this.hyprland ? CHECK_ON : CHECK_OFF, (cx) => this.toggle(cx));
        }
        case "autostart": {
          const have = this.installed && this.installed.autostart;
          return row(it.row, "Start with the session", have ? "In ~/.config/hypr/dig.lua" : "Keeps the window ready so it opens instantly", have ? "\u{f00c}" : this.autostart ? CHECK_ON : CHECK_OFF, (cx) => this.toggle(cx));
        }
        case "apply": {
          const selected = it.row === this.cursor;
          return h_flex().w_full().h(70).pt(16).pb(12).px(12).child(
            div()
              .id("apply")
              .px(18)
              .h(34)
              .flex()
              .items_center()
              .rounded(cx.theme().radius.md)
              .bg(colors.primary)
              .text_size(13)
              .font_medium()
              .text_color(colors.primary_foreground)
              .border(selected ? 2 : 0)
              .border_color(colors.foreground)
              .on_click((_e, cx) => this.apply(cx, this.onDone))
              .child(this.busy === "apply" ? "Setting up…" : firstRun ? "Start indexing" : "Save"),
          );
        }
      }
      return div();
    };
    return v_flex()
      .flex_1()
      .min_h_0()
      .px(8)
      .py(6)
      .overflow_hidden()
      .child(
        v_flex().px(12).pt(10).pb(2).gap(4)
          .child(div().text_size(16).font_medium().text_color(colors.foreground).child(firstRun ? "Set up Dig" : "Settings"))
          .child(div().text_size(12).text_color(colors.muted_foreground).child(this.loading ? "Looking at your folders…" : this.error ? this.error : "Pick what to index. Files never leave this machine.")),
      )
      .child(
        v_virtual_list("setup-list", items.length, items.map((it) => it.height), (i) => `s-${i}`, (range, cx) => {
          const out = [];
          for (let i = range.start; i < range.end; i++) out.push(render(items[i]));
          return out;
        }).flex_1().min_h_0().track_scroll(this.scroll),
      );
  }
}

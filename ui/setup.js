// The first-run and settings screen: what to index, the model files, the desktop hooks. Same shape as the search
// screen so it feels like one thing: rows the cursor moves over, space toggles, Enter starts.

import { div } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
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
    cx.notify();
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
    const rows = this.rows();
    const progress = setupProgress();
    const section = (label) => div().pt(10).pb(2).px(12).text_size(11).text_color(colors.muted_foreground).child(label.toUpperCase());
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
    const items = [];
    let i = 0;
    for (const r of rows) {
      const index = i;
      if (r.kind === "models") {
        if (index === 0) items.push(section("Model"));
        const caption = progress ? progress.message : this.busy === "download" ? "Downloading…" : "EmbeddingGemma 2 · 865 MB, once · stays on this machine";
        items.push(row(index, this.textModel ? "Image and audio encoders missing" : "Download the model files", caption, this.busy === "download" ? "\u{f110}" : "\u{f019}", (cx) => this.download(cx)));
        if (progress) items.push(div().mx(12).h(3).rounded(2).bg(colors.muted).child(div().h(3).w(`${Math.round(progress.fraction * 100)}%`).rounded(2).bg(colors.primary)));
      } else if (r.kind === "folder") {
        if (r.index === 0) items.push(section("Folders to index"));
        const f = this.folders[r.index];
        items.push(row(index, f.label, f.summary, f.selected ? CHECK_ON : CHECK_OFF, (cx) => this.toggle(cx)));
      } else if (r.kind === "code") {
        if (r.index === 0) items.push(section("Code, searched with a code: prefix"));
        const f = this.code[r.index];
        items.push(row(index, f.label, f.summary, f.selected ? CHECK_ON : CHECK_OFF, (cx) => this.toggle(cx)));
      } else if (r.kind === "hyprland") {
        items.push(section("Desktop"));
        const have = this.installed && this.installed.binding && this.installed.rules;
        items.push(row(index, "SUPER + CTRL + G opens Dig", have ? "Keybinding and window rules are in ~/.config/hypr" : "Adds a keybinding and window rules to ~/.config/hypr", have ? "\u{f00c}" : this.hyprland ? CHECK_ON : CHECK_OFF, (cx) => this.toggle(cx)));
      } else if (r.kind === "autostart") {
        const have = this.installed && this.installed.autostart;
        items.push(row(index, "Start with the session", have ? "In ~/.config/hypr/autostart.lua" : "Keeps the window ready so it opens instantly", have ? "\u{f00c}" : this.autostart ? CHECK_ON : CHECK_OFF, (cx) => this.toggle(cx)));
      } else if (r.kind === "apply") {
        const selected = index === this.cursor;
        items.push(
          h_flex().w_full().pt(16).pb(12).px(12).child(
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
          ),
        );
      }
      i += 1;
    }
    return v_flex()
      .id("setup-scroll")
      .flex_1()
      .min_h_0()
      .px(8)
      .py(6)
      .overflow_hidden()
      .scrollable(true)
      .child(
        v_flex().px(12).pt(10).pb(2).gap(4)
          .child(div().text_size(16).font_medium().text_color(colors.foreground).child(firstRun ? "Set up Dig" : "Settings"))
          .child(div().text_size(12).text_color(colors.muted_foreground).child(this.loading ? "Looking at your folders…" : this.error ? this.error : "Pick what to index. Files never leave this machine.")),
      )
      .children(items);
  }
}

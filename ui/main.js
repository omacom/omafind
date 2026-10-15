// Dig's search window: a search field, a result list and a footer, over the `dig` host module.
//
// Phase 0 spike: proves the stack. The host answers `search` from Rust (canned results for now), `status` is read
// once, Escape hides, ↑/↓ move, Enter opens. Everything that touches files or the index stays in Rust.
//
//   cargo run -p dig-app -- --dev      # hot-reloads this file on save

import { View, div } from "gpui-kit";
import { v_flex, h_flex, InputState, Input } from "gpui-base";
import { search, status, open, hide } from "dig";
/** @import { AsyncContext, Context } from "gpui-kit" */
/** @import { Result } from "dig" */

const SPACE = { xs: 4, sm: 8, md: 12, lg: 16 };
const DEBOUNCE_MS = 120;

export default class Dig extends View {
  /** @param {unknown} _props @param {AsyncContext} cx */
  init(_props, cx) {
    /** @type {InputState} */
    this.query = InputState.new({ placeholder: "Search your files… (code: for code)" });
    this.query.on("change", (_event, cx) => this.schedule(cx));
    // Enter is handled once, by the root's on_key_down (a `submit` hook here would open the result twice).
    /** @type {Result[]} */
    this.results = [];
    this.selected = 0;
    this.pending = null;
    this.tookMs = null;
    this.calls = 0;
    this.keys = 0;
    this.lastKey = "";
    this.status = status();
    this.focus = cx.focus_handle();
    // Something must hold the keyboard for key events to be dispatched at all; the search field takes it from here.
    this.focus.focus();
  }

  /** @param {Context} cx */
  schedule(cx) {
    if (this.pending) this.pending.cancel();
    this.pending = cx.timer.after(DEBOUNCE_MS, (cx) => this.run(cx));
  }

  /** @param {AsyncContext} cx */
  run(cx) {
    const text = this.query.value().trim();
    if (text === "") {
      this.results = [];
      this.tookMs = null;
      cx.notify();
      return;
    }
    const started = Date.now();
    cx.spawn(async (cx) => {
      const answer = await search(text, { limit: 20 });
      // A slower earlier search must not overwrite a newer one.
      if (this.query.value().trim() !== answer.query) return;
      this.results = answer.results;
      this.tookMs = Date.now() - started;
      this.calls += 1;
      this.selected = 0;
      cx.notify();
    });
  }

  /** @param {Context} cx */
  openSelected(cx) {
    const row = this.results[this.selected];
    if (row) open(row.id);
    cx.notify();
  }

  /** @param {number} delta @param {Context} cx */
  move(delta, cx) {
    if (this.results.length === 0) return;
    this.selected = (this.selected + delta + this.results.length) % this.results.length;
    cx.notify();
  }

  /** @param {import("gpui-kit").KeyEvent} event @param {Context} cx */
  onKey(event, cx) {
    this.keys += 1;
    this.lastKey = event.keystroke;
    switch (event.keystroke) {
      case "escape":
        hide();
        return;
      case "down":
      case "ctrl-n":
        this.move(1, cx);
        return;
      case "up":
      case "ctrl-p":
        this.move(-1, cx);
        return;
      case "enter":
        this.openSelected(cx);
        return;
    }
    cx.notify();
  }

  /** @param {Result} row @param {number} index @param {Context} cx */
  row(row, index, cx) {
    const colors = cx.theme().colors;
    const selected = index === this.selected;
    return h_flex()
      .id(`row-${row.id}`)
      .w_full()
      .h(44)
      .px(SPACE.md)
      .gap(SPACE.md)
      .items_center()
      .rounded(6)
      .bg(selected ? colors.muted : colors.background)
      .on_click((_event, cx) => {
        this.selected = index;
        this.openSelected(cx);
      })
      .child(div().w(24).text_size(16).text_color(colors.muted_foreground).child(glyph(row.kind)))
      .child(
        v_flex()
          .flex_1()
          .min_w_0()
          .gap(2)
          .child(div().text_size(13).font_medium().text_color(colors.foreground).child(row.name))
          .child(div().text_size(12).text_color(colors.muted_foreground).child(row.snippet)),
      )
      .child(div().text_size(12).text_color(colors.muted_foreground).child(row.locatorLabel));
  }

  /** @param {Context} cx */
  render(cx) {
    const colors = cx.theme().colors;
    const footer =
      this.tookMs === null
        ? `${this.status.state} · ${this.status.indexed} files indexed`
        : `${this.results.length} results in ${this.tookMs} ms · host call ${this.calls} · keys ${this.keys} (${this.lastKey})`;

    return v_flex()
      .size_full()
      .bg(colors.background)
      .track_focus(this.focus)
      .on_key_down((event, cx) => this.onKey(event, cx))
      .child(
        h_flex()
          .w_full()
          .p(SPACE.md)
          .gap(SPACE.sm)
          .border_b(1)
          .border_color(colors.border)
          .child(div().text_size(18).text_color(colors.muted_foreground).child("⌕"))
          .child(
            Input.new(this.query)
              .flex_1()
              .h(32)
              .px(SPACE.sm)
              .border(1)
              .border_color(colors.input)
              .bg(colors.surface)
              .rounded(6)
              .text_size(14),
          ),
      )
      .child(
        v_flex()
          .flex_1()
          .min_h_0()
          .p(SPACE.sm)
          .gap(2)
          .overflow_hidden()
          .children(this.results.map((row, index) => this.row(row, index, cx)))
          .child(
            this.results.length === 0
              ? div()
                  .flex_1()
                  .flex()
                  .items_center()
                  .justify_center()
                  .text_size(13)
                  .text_color(colors.muted_foreground)
                  .child(this.query.value().trim() === "" ? "Type to search photos, PDFs, documents, recordings and video." : "Nothing matched.")
              : div(),
          ),
      )
      .child(
        h_flex()
          .w_full()
          .px(SPACE.md)
          .py(SPACE.sm)
          .border_t(1)
          .border_color(colors.border)
          .justify_between()
          .text_size(12)
          .text_color(colors.muted_foreground)
          .child(div().child(footer))
          .child(div().child("↑↓ move · ⏎ open · esc hide")),
      );
  }
}

/** @param {string} kind */
function glyph(kind) {
  switch (kind) {
    case "image": return "▣";
    case "pdf": return "▤";
    case "audio": return "♪";
    case "video": return "▶";
    case "code": return "{}";
    default: return "≡";
  }
}

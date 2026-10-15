// Dig's search window, in the shape of Omarchy's own overlays (the menu, the clipboard history): a field on top,
// rows below, a footer that says what the indexer is doing. Everything that touches files, the index or processes
// stays in Rust behind the `dig` host module; this file is layout and keys.
//
//   cargo run -p dig-app -- --dev      # hot-reloads this file on save

import { View, div, image } from "gpui-kit";
import { v_flex, h_flex, InputState, Input, VirtualListScrollHandle, v_virtual_list } from "gpui-base";
import { search, thumb, status, open, reveal, copy, hide } from "dig";
import { FILTERS, glyph, count } from "./kinds.js";
/** @import { AsyncContext, Context } from "gpui-kit" */
/** @import { Result } from "dig" */

const DEBOUNCE_MS = 120;
const ROW = 56;
const THUMB = 40;

export default class Dig extends View {
  /** @param {unknown} _props @param {AsyncContext} cx */
  init(_props, cx) {
    /** @type {InputState} */
    this.query = InputState.new({ placeholder: "Search your files" });
    this.query.on("change", (_event, cx) => this.schedule(cx));
    /** @type {Result[]} */
    this.results = [];
    this.selected = 0;
    this.filter = 0;
    this.pending = null;
    this.tookMs = null;
    this.searched = "";
    /** @type {Map<string, string | null>} */
    this.thumbs = new Map();
    this.thumbTask = null;
    this.scroll = VirtualListScrollHandle.new();
    this.focus = cx.focus_handle();
    this.focus.focus();
    this.shown = status().shown;
    this.focusField(cx);
  }

  /** Moves the keyboard onto the search field: the root holds it, the field is the next tab stop. */
  focusField(cx) {
    cx.timer.after(30, () => {
      this.focus.focus();
      window.focus_next();
    });
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
      this.searched = "";
      cx.notify();
      return;
    }
    const kinds = FILTERS[this.filter].kinds;
    cx.spawn(async (cx) => {
      let answer;
      try {
        answer = await search(text, { limit: 40, kinds });
      } catch (error) {
        this.error = String(error);
        cx.notify();
        return;
      }
      // A slower earlier search must not overwrite a newer one.
      if (this.query.value().trim() !== answer.query) return;
      this.error = null;
      this.results = answer.results;
      this.tookMs = answer.tookMs;
      this.searched = answer.query;
      this.selected = 0;
      cx.notify();
      this.loadThumbs(cx);
    });
  }

  /** Thumbnails one at a time, top of the list first; each arrival repaints. */
  loadThumbs(cx) {
    if (this.thumbTask) this.thumbTask.cancel();
    const wanted = this.results.filter((r) => !this.thumbs.has(thumbKey(r)) && hasThumb(r.kind));
    if (wanted.length === 0) return;
    this.thumbTask = cx.spawn(async (cx) => {
      for (const row of wanted) {
        const key = thumbKey(row);
        if (this.thumbs.has(key)) continue;
        try {
          this.thumbs.set(key, await thumb(row.path, row.kind, row.locator));
        } catch (_) {
          this.thumbs.set(key, null);
        }
        cx.notify();
      }
    });
  }

  /** @param {Context} cx */
  openSelected(cx) {
    const row = this.results[this.selected];
    if (row) open(row.path, row.kind, row.locator);
    cx.notify();
  }

  move(delta, cx) {
    if (this.results.length === 0) return;
    this.selected = Math.max(0, Math.min(this.results.length - 1, this.selected + delta));
    this.scroll.scroll_to_item(this.selected);
    cx.notify();
  }

  cycleFilter(delta, cx) {
    this.filter = (this.filter + delta + FILTERS.length) % FILTERS.length;
    cx.notify();
    this.run(cx);
  }

  /** @param {import("gpui-kit").KeyEvent} event @param {Context} cx */
  onKey(event, cx) {
    const row = this.results[this.selected];
    switch (event.keystroke) {
      case "escape": hide(); return;
      case "down": case "ctrl-n": case "ctrl-j": this.move(1, cx); return;
      case "up": case "ctrl-p": case "ctrl-k": this.move(-1, cx); return;
      case "pagedown": this.move(8, cx); return;
      case "pageup": this.move(-8, cx); return;
      case "enter": this.openSelected(cx); return;
      case "ctrl-enter": if (row) reveal(row.path); return;
      case "ctrl-c": case "ctrl-shift-c": if (row) copy(row.path); return;
      case "tab": this.cycleFilter(1, cx); return;
      case "shift-tab": this.cycleFilter(-1, cx); return;
    }
  }

  /** One result row. */
  row(row, index, cx) {
    const colors = cx.theme().colors;
    const selected = index === this.selected;
    const key = thumbKey(row);
    const picture = this.thumbs.get(key);
    const caption = row.snippet !== "" ? row.snippet : row.shortPath;
    return h_flex()
      .id(`row-${row.id}`)
      .w_full()
      .h(ROW)
      .px(12)
      .gap(12)
      .items_center()
      .rounded(cx.theme().radius.md)
      .bg(selected ? colors.accent : colors.background)
      .on_click((_event, cx) => {
        this.selected = index;
        this.openSelected(cx);
      })
      .child(
        div()
          .w(THUMB)
          .h(THUMB)
          .flex_none()
          .rounded(cx.theme().radius.sm)
          .overflow_hidden()
          .bg(colors.muted)
          .flex()
          .items_center()
          .justify_center()
          .text_size(18)
          .text_color(colors.muted_foreground)
          .child(picture ? image(picture).w(THUMB).h(THUMB) : div().child(glyph(row.kind))),
      )
      .child(
        v_flex()
          .flex_1()
          .min_w_0()
          .gap(3)
          .child(
            h_flex()
              .gap(8)
              .items_center()
              .child(div().text_size(13).font_medium().text_color(selected ? colors.accent_foreground : colors.foreground).overflow_hidden().whitespace_nowrap().text_ellipsis().child(row.name))
              .child(row.locatorLabel !== "" ? div().text_size(11).text_color(colors.muted_foreground).child(row.locatorLabel) : div()),
          )
          .child(div().text_size(12).text_color(colors.muted_foreground).overflow_hidden().whitespace_nowrap().text_ellipsis().child(caption)),
      )
      .child(div().flex_none().text_size(13).text_color(colors.muted_foreground).child(glyph(row.kind)));
  }

  footer(cx) {
    const s = status();
    const colors = cx.theme().colors;
    let left;
    if (!s.daemon) left = "digd isn't running";
    else if (s.state === "indexing") left = `Indexing ${count(s.pending)} left${s.current ? " · " + s.current.split("/").pop() : ""}`;
    else if (s.state === "crawling") left = "Looking for new files";
    else if (s.state === "paused") left = `Paused · ${count(s.indexed)} files`;
    else if (s.state === "paused_power_saver") left = `Paused in power-saver · ${count(s.indexed)} files`;
    else if (s.state === "waiting_for_ac") left = `Waiting for a charger · ${count(s.indexed)} files`;
    else if (this.tookMs !== null) left = `${count(this.results.length)} results · ${Math.round(this.tookMs)} ms`;
    else left = `${count(s.indexed)} files indexed`;
    return h_flex()
      .w_full()
      .px(16)
      .py(8)
      .border_t(1)
      .border_color(colors.border)
      .justify_between()
      .text_size(11)
      .text_color(colors.muted_foreground)
      .child(div().child(left))
      .child(div().child("↑↓ move · ⏎ open · ⌃⏎ reveal · ⌃C copy · ⇥ filter · esc close"));
  }

  empty(cx) {
    const s = status();
    const colors = cx.theme().colors;
    let title, hint;
    if (!s.daemon) {
      title = "Dig isn't running";
      hint = "systemctl --user start digd";
    } else if (s.roots.length === 0) {
      title = "Nothing indexed yet";
      hint = "dig index ~/Pictures ~/Documents";
    } else if (this.error) {
      title = "Search failed";
      hint = this.error;
    } else if (this.searched !== "") {
      title = "Nothing matched";
      hint = FILTERS[this.filter].kinds.length ? "Tab widens the filter" : "Try other words: what it shows, what it says, when it was";
    } else {
      title = "Search by what it is about";
      hint = "a dog on the beach · the clause about pets in the lease · where they talk about sleep";
    }
    return v_flex()
      .flex_1()
      .items_center()
      .justify_center()
      .gap(6)
      .child(div().text_size(14).text_color(colors.foreground).child(title))
      .child(div().text_size(12).text_color(colors.muted_foreground).child(hint));
  }

  /** @param {Context} cx */
  render(cx) {
    const colors = cx.theme().colors;
    const s = status();
    if (s.shown !== this.shown) {
      // Shown again: fresh field, keyboard on it.
      this.shown = s.shown;
      this.query.set_value("");
      this.results = [];
      this.tookMs = null;
      this.searched = "";
      this.focusField(cx);
    }
    const results = this.results;
    const filters = h_flex().gap(14).items_center().children(
      FILTERS.map((f, i) =>
        div()
          .id(`filter-${i}`)
          .text_size(11)
          .text_color(i === this.filter ? colors.accent_foreground : colors.muted_foreground)
          .on_click((_e, cx) => {
            this.filter = i;
            cx.notify();
            this.run(cx);
          })
          .child(f.label),
      ),
    );

    return v_flex()
      .size_full()
      .bg(colors.background)
      .track_focus(this.focus)
      .on_key_down((event, cx) => this.onKey(event, cx))
      .child(
        h_flex()
          .w_full()
          .px(16)
          .py(12)
          .gap(12)
          .items_center()
          .border_b(1)
          .border_color(colors.border)
          .child(div().text_size(16).text_color(colors.muted_foreground).child("\u{f002}"))
          .child(Input.new(this.query).flex_1().h(34).px(10).border(1).border_color(colors.input).bg(colors.surface).rounded(cx.theme().radius.md).text_size(14))
          .child(filters),
      )
      .child(
        results.length === 0
          ? this.empty(cx)
          : div()
              .flex_1()
              .min_h_0()
              .px(8)
              .py(6)
              .child(
                v_virtual_list("results", results.length, ROW, (i) => `r-${results[i].id}`, (range, cx) => {
                  const out = [];
                  for (let i = range.start; i < range.end; i++) out.push(this.row(results[i], i, cx));
                  return out;
                }).size_full().track_scroll(this.scroll),
              ),
      )
      .child(this.footer(cx));
  }
}

function thumbKey(row) {
  return `${row.path}#${row.locator ?? ""}`;
}

function hasThumb(kind) {
  return kind === "image" || kind === "screenshot" || kind === "video" || kind === "pdf";
}

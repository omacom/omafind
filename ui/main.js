// Omafind's search window: a field on top with the filters as chips, results on the left (rows, or a grid for photos),
// a preview of the selected result on the right, a footer that says what the indexer is doing. Opened with an empty
// field it shows recent searches and files instead; on a first run, setup. Everything that touches files, the index
// or processes stays in Rust behind the `omafind` host module: this file is layout and keys.
//
//   cargo run -p omafind-app -- --dev      # hot-reloads this file on save

import { View, div, image } from "gpui-kit";
import { homedir } from "os";
import { v_flex, h_flex, InputState, Input, VirtualListScrollHandle, v_virtual_list, TextView } from "gpui-base";
import { search, similar, recents, passage, lines, section, syntax, thumb, status, open, reveal, copy, copyText, trash, hide } from "omafind";
import { FILTERS, glyph, count, filterCount, tag, when, bytes, duration, marks, alpha, escapeMarkdown } from "./kinds.js";
import { Setup } from "./setup.js";
/** @import { AsyncContext, Context } from "gpui-kit" */
/** @import { Result, Answer } from "omafind" */

// Names and keywords answer in a few ms: shown almost at once. The model's answer follows when typing pauses.
const QUICK_MS = 30;
const FULL_MS = 110;
const ROW = 90;
// Result thumbnails are page-shaped, like the design's: a PDF page, a document card, a frame.
const THUMB = 44;
const THUMB_H = 56;
const TILE = 140;
const TILE_GAP = 8;

// Filters typed into the field turn into chips once they are complete (followed by a space).
const FILTER_RE = /(^|\s)((?:in|type|kind|after|since|before|on|during):\S+|today|yesterday|(?:this|last) (?:week|month|year))(?=\s)/gi;

export default class Omafind extends View {
  /** @param {unknown} _props @param {AsyncContext} cx */
  init(_props, cx) {
    this.query = InputState.new({ placeholder: "Describe it: what it shows, says, or when it was" });
    this.query.on("change", (_event, cx) => this.changed(cx));
    /** @type {Result[]} */
    this.results = [];
    this.selected = 0;
    this.filter = 0;
    this.counts = null;
    this.chips = [];
    this.tookMs = null;
    this.searched = "";
    this.quickShown = false;
    this.error = null;
    this.timers = [];
    /** @type {Map<string, string | null>} */
    this.thumbs = new Map();
    this.thumbTask = null;
    /** Preview data per result key: { large, passage, lines }. */
    this.previews = new Map();
    this.scroll = VirtualListScrollHandle.new();
    this.focus = cx.focus_handle();
    this.focus.focus();
    // Tab is focus traversal unless something nearer binds it: these bindings hold inside the window's context.
    cx.bind_keys([
      { keystroke: "tab", action: "omafind_next", context: "Omafind" },
      { keystroke: "shift-tab", action: "omafind_previous", context: "Omafind" },
    ]);
    this.shown = status().shown;
    this.mode = "search";
    this.setup = new Setup();
    this.recents = null;
    this.historyAt = -1;
    this.menu = null; // { items, selected, confirm }
    this.like = null; // the result "more like this" started from
    this.grid = null; // null = follow the filter
    this.cols = 4;
    this.listW = 500;
    this.decided = false;
    this.focusField(cx);
    this.loadRecents(cx);
    cx.timer.after(3000, (cx) => {
      const s = status();
      if (!this.decided && !s.daemon && !s.configured) {
        this.decided = true;
        this.openSetup(cx, true);
      }
    });
  }

  // ---- modes -----------------------------------------------------------------------------------------------

  openSetup(cx, firstRun) {
    this.mode = "setup";
    this.firstRun = firstRun;
    this.setup.load(cx);
    cx.notify();
    cx.timer.after(30, () => this.focus.focus());
  }

  closeSetup(cx) {
    this.mode = "search";
    this.focusField(cx);
    cx.notify();
  }

  /** Moves the keyboard onto the search field: the root holds it, the field is the next tab stop. */
  focusField(cx) {
    cx.timer.after(30, () => {
      this.focus.focus();
      window.focus_next();
    });
  }

  /** The list pane follows the window, which Omafind sizes to the monitor: wider for the grid, never past half. */
  layout() {
    const width = window.viewport_size().width;
    if (this.isGrid()) {
      this.cols = Math.max(3, Math.min(6, Math.floor((width * 0.55 - 20) / (TILE + TILE_GAP))));
      this.listW = this.cols * (TILE + TILE_GAP) + 20;
    } else {
      this.cols = 1;
      this.listW = Math.round(Math.max(420, Math.min(620, width * 0.44)));
    }
  }

  isGrid() {
    return this.grid ?? FILTERS[this.filter].grid;
  }

  // ---- searching -------------------------------------------------------------------------------------------

  text() {
    return this.query.value().trim();
  }

  changed(cx) {
    this.tokenize();
    this.historyAt = -1;
    this.like = null;
    this.schedule(cx);
  }

  /**
   * The first completed filter in the field that isn't a chip yet becomes one. The chip takes the space after it too,
   * so the caret lands past it and the words typed next stay separate. Replacing fires another change, which picks
   * up the next filter. Offsets are UTF-16, as the input counts them.
   */
  tokenize() {
    const content = this.query.content();
    const taken = content.tokens.map((t) => t.range);
    FILTER_RE.lastIndex = 0;
    let m;
    while ((m = FILTER_RE.exec(content.text))) {
      const start = m.index + m[1].length;
      const end = start + m[2].length + 1;
      if (taken.some((r) => r.start < end && start < r.end)) continue;
      this.query.replace_range_with_token({ start, end }, { id: `${start}-${m[2]}`, text: `${m[2]} `, label: chipLabel(m[2]) });
      return;
    }
  }

  schedule(cx) {
    for (const t of this.timers) t.cancel();
    this.timers = [cx.timer.after(QUICK_MS, (cx) => this.run(cx, true)), cx.timer.after(FULL_MS, (cx) => this.run(cx, false))];
  }

  /** @param {AsyncContext} cx @param {boolean} quick */
  run(cx, quick) {
    const text = this.text();
    if (text === "") {
      this.results = [];
      this.tookMs = null;
      this.searched = "";
      this.counts = null;
      this.chips = [];
      this.loadRecents(cx);
      cx.notify();
      return;
    }
    const kinds = FILTERS[this.filter].kinds;
    cx.spawn(async (cx) => {
      /** @type {Answer} */
      let answer;
      try {
        answer = await search(text, { limit: 60, kinds, quick, session: "window" });
      } catch (error) {
        if (!quick) {
          this.error = String(error);
          cx.notify();
        }
        return;
      }
      // A slower earlier search must not overwrite a newer one, nor a quick answer the full one.
      if (answer.superseded || this.text() !== answer.query) return;
      if (quick && this.searched === answer.query && !this.quickShown) return;
      this.show(answer, quick, cx);
    });
  }

  /** Puts an answer on screen, keeping the selected file selected when it is still there. */
  show(answer, quick, cx) {
    const keep = this.results[this.selected]?.path;
    if (this.menu) this.menu = null;
    this.error = null;
    this.results = answer.results;
    this.tookMs = answer.tookMs;
    this.searched = answer.query;
    this.quickShown = quick;
    this.counts = answer.counts;
    this.chips = answer.chips;
    const again = keep ? this.results.findIndex((r) => r.path === keep) : -1;
    this.selected = again >= 0 && !quick ? again : 0;
    this.scroll.scroll_to_item(this.rowOf(this.selected));
    cx.notify();
    this.loadThumbs(cx);
    this.loadPreview(cx);
  }

  loadRecents(cx) {
    cx.spawn(async (cx) => {
      try {
        this.recents = await recents();
      } catch (_) {
        this.recents = null;
      }
      cx.notify();
      this.loadThumbsFor([...(this.recents?.added ?? []), ...(this.recents?.opened ?? [])], cx);
    });
  }

  moreLikeThis(cx) {
    const row = this.results[this.selected];
    if (!row) return;
    cx.spawn(async (cx) => {
      let answer;
      try {
        answer = await similar(row.id, { code: row.kind === "code" });
      } catch (error) {
        this.error = String(error);
        cx.notify();
        return;
      }
      this.like = row;
      this.show({ ...answer, query: this.text() }, false, cx);
    });
  }

  // ---- pictures and previews -------------------------------------------------------------------------------

  loadThumbs(cx) {
    this.loadThumbsFor(this.results, cx);
  }

  /** Small pictures, top of the list first, two at a time; each arrival repaints. Most were made while indexing. */
  loadThumbsFor(rows, cx) {
    if (this.thumbTask) this.thumbTask.cancel();
    const wanted = rows.filter((r) => !this.thumbs.has(thumbKey(r)) && hasThumb(r.kind));
    if (wanted.length === 0) return;
    this.thumbTask = cx.spawn(async (cx) => {
      const lane = async () => {
        while (wanted.length) {
          const row = wanted.shift();
          const key = thumbKey(row);
          if (this.thumbs.has(key)) continue;
          try {
            this.thumbs.set(key, await thumb(row.path, row.kind, row.locator, "small"));
          } catch (_) {
            this.thumbs.set(key, null);
          }
          cx.notify();
        }
      };
      await Promise.all([lane(), lane()]);
    });
  }

  /** What the preview of the selected result needs: the large picture, the passage, the lines of code. */
  loadPreview(cx) {
    const row = this.results[this.selected];
    if (!row) return;
    const key = thumbKey(row);
    if (this.previews.has(key)) return;
    const entry = { large: undefined, passage: undefined, lines: undefined, first: 0, last: 0 };
    this.previews.set(key, entry);
    cx.spawn(async (cx) => {
      try {
        if (hasThumb(row.kind)) entry.large = await thumb(row.path, row.kind, row.locator, "large");
        if (row.kind === "code") {
          entry.first = row.locator ?? 1;
          entry.last = Math.min(row.locatorEnd ?? entry.first, entry.first + 79);
          entry.lines = await lines(row.path, Math.max(1, entry.first - 4), entry.last + 4);
          entry.best = bestLine(entry, row.matchedWords);
          // Open the editor where the words are, not where the stretch starts.
          row.openAt = entry.best;
        } else if (row.kind === "doc" && row.locator != null && row.locatorEnd != null) {
          // A plain text file shows as lines, like code; an office document's text isn't the file's, so a passage.
          const found = await section(row.path, row.locator, row.locatorEnd);
          if (found) {
            Object.assign(entry, found);
            entry.best = bestLine(entry, row.matchedWords);
            row.openAt = entry.best;
          }
          else entry.passage = await passage(row.id, false);
        } else if (row.kind === "doc" || row.kind === "pdf" || row.kind === "screenshot") {
          entry.passage = await passage(row.id, false);
        }
      } catch (_) {
        // A preview that can't be made shows the row's own text.
      }
      if (this.previews.size > 200) this.previews.delete(this.previews.keys().next().value);
      cx.notify();
    });
  }

  // ---- moving and acting -----------------------------------------------------------------------------------

  rowOf(index) {
    return this.isGrid() ? Math.floor(index / this.cols) : index;
  }

  move(delta, cx) {
    if (this.results.length === 0) return;
    this.selected = Math.max(0, Math.min(this.results.length - 1, this.selected + delta));
    this.scroll.scroll_to_item(this.rowOf(this.selected));
    this.loadPreview(cx);
    cx.notify();
  }

  cycleFilter(delta, cx) {
    this.filter = (this.filter + delta + FILTERS.length) % FILTERS.length;
    this.grid = null;
    cx.notify();
    if (this.text() !== "") {
      this.run(cx, true);
      this.run(cx, false);
    }
  }

  openSelected(cx) {
    const row = this.results[this.selected];
    if (row) open(row.path, row.kind, openLocator(row));
    cx.notify();
  }

  /** Actions for the selected result, in the order the menu shows them. */
  actionsFor(row) {
    const primary = primaryLabel(row);
    const items = [
      { label: primary, keys: "⏎", run: () => open(row.path, row.kind, openLocator(row)) },
      { label: "More like this", keys: "⌃S", run: (cx) => this.moreLikeThis(cx), stay: true },
      { label: "Show in Files", keys: "⌃⏎", run: () => reveal(row.path) },
      { label: "Copy file", keys: "⌃C", run: () => copy(row.path) },
      { label: "Copy path", keys: "⌃⇧C", run: () => copyText(row.path) },
    ];
    const preview = this.previews.get(thumbKey(row));
    const text = preview?.passage?.text || row.snippet;
    if (text) items.push({ label: row.kind === "screenshot" ? "Copy text in image" : "Copy passage", keys: "", run: () => copyText(text) });
    items.push({ label: "Move to trash", keys: "", danger: true, confirm: true, run: (cx) => this.trashSelected(cx), stay: true });
    return items;
  }

  openMenu(cx) {
    const row = this.results[this.selected];
    if (!row) return;
    this.menu = { row, items: this.actionsFor(row), selected: 0, confirm: false };
    cx.notify();
  }

  runMenu(cx) {
    const item = this.menu.items[this.menu.selected];
    if (item.confirm && !this.menu.confirm) {
      this.menu.confirm = true;
      cx.notify();
      return;
    }
    this.menu = null;
    item.run(cx);
    cx.notify();
  }

  trashSelected(cx) {
    const row = this.results[this.selected];
    if (!row) return;
    try {
      trash(row.path);
      this.results = this.results.filter((r) => r.path !== row.path);
      this.selected = Math.min(this.selected, Math.max(0, this.results.length - 1));
    } catch (error) {
      this.error = String(error);
    }
    cx.notify();
  }

  /** @param {import("gpui-kit").KeyEvent} event @param {Context} cx */
  onKey(event, cx) {
    const key = event.keystroke;
    if (this.mode === "setup") {
      switch (key) {
        case "escape": if (this.firstRun) hide(); else this.closeSetup(cx); return;
        case "down": case "ctrl-n": case "ctrl-j": case "tab": this.setup.move(1, cx); return;
        case "up": case "ctrl-p": case "ctrl-k": case "shift-tab": this.setup.move(-1, cx); return;
        case "space": this.setup.toggle(cx); return;
        case "enter": this.setup.apply(cx, (_message, cx) => { this.firstRun = false; this.closeSetup(cx); }); return;
      }
      return;
    }
    if (this.menu) {
      const n = this.menu.items.length;
      switch (key) {
        case "escape": this.menu = null; cx.notify(); return;
        case "down": case "ctrl-n": case "ctrl-j": this.menu.selected = (this.menu.selected + 1) % n; this.menu.confirm = false; cx.notify(); return;
        case "up": case "ctrl-p": case "ctrl-k": this.menu.selected = (this.menu.selected + n - 1) % n; this.menu.confirm = false; cx.notify(); return;
        case "enter": this.runMenu(cx); return;
      }
      return;
    }
    const row = this.results[this.selected];
    const grid = this.isGrid() && this.results.length > 0;
    switch (key) {
      case "escape":
        if (this.like) {
          this.like = null;
          this.run(cx, false);
        } else hide();
        return;
      case "ctrl-,": this.openSetup(cx, false); return;
      case "down": case "ctrl-n": case "ctrl-j": this.move(grid ? this.cols : 1, cx); return;
      case "up":
        if (this.text() === "" && this.recents?.searches.length) {
          this.historyAt = Math.min(this.historyAt + 1, this.recents.searches.length - 1);
          this.query.set_value(this.recents.searches[this.historyAt].query);
          this.schedule(cx);
          return;
        }
        this.move(grid ? -this.cols : -1, cx);
        return;
      case "ctrl-p": case "ctrl-k":
        if (key === "ctrl-k" && row) { this.openMenu(cx); return; }
        this.move(grid ? -this.cols : -1, cx);
        return;
      case "right": if (grid) this.move(1, cx); return;
      case "left": if (grid) this.move(-1, cx); return;
      case "pagedown": this.move(grid ? this.cols * 3 : 8, cx); return;
      case "pageup": this.move(grid ? -this.cols * 3 : -8, cx); return;
      case "enter": this.openSelected(cx); return;
      case "ctrl-enter": if (row) reveal(row.path); return;
      case "ctrl-c": if (row) copy(row.path); return;
      case "ctrl-shift-c": if (row) copyText(row.path); return;
      case "ctrl-s": this.moreLikeThis(cx); return;
      case "ctrl-g": this.grid = !this.isGrid(); this.scroll.scroll_to_item(this.rowOf(this.selected)); cx.notify(); return;
      case "tab": this.cycleFilter(1, cx); return;
      case "shift-tab": this.cycleFilter(-1, cx); return;
    }
  }

  // ---- pieces ----------------------------------------------------------------------------------------------

  /** A small rounded tag. */
  pill(text, cx, strong) {
    const c = cx.theme().colors;
    return div()
      .flex_none()
      .px(6)
      .py(1)
      .rounded(3)
      .text_size(10)
      .bg(strong ? c.muted : c.background)
      .border(1)
      .border_color(strong ? c.muted : c.border)
      .text_color(strong ? c.foreground : c.muted_foreground)
      .child(text);
  }

  /** One line of text with the query's words in the accent. */
  marked(text, words, color, hitColor, size) {
    return h_flex()
      .min_w_0()
      .overflow_hidden()
      .whitespace_nowrap()
      .text_size(size)
      .children(marks(text, words).map((m) => div().flex_none().text_color(m.hit ? hitColor : color).child(m.text)));
  }

  picture(row, w, h, cx) {
    const c = cx.theme().colors;
    const p = this.thumbs.get(thumbKey(row));
    const cell = div().w(w).h(h).flex_none().rounded(3).overflow_hidden().bg(c.muted).flex().items_center().justify_center().relative();
    // A moment in a recording or video sits on the picture, as on a video thumbnail.
    const badge = (row.kind === "video" || row.kind === "audio") && row.locatorLabel
      ? div().absolute().left(3).bottom(3).px(3).rounded(2).bg(alpha(c.background, 0.75)).text_size(9).text_color(c.foreground).child(row.locatorLabel)
      : div();
    if (p) return cell.child(image(p).size_full()).child(badge);
    // Documents and code without a picture show their type, the way a file card would.
    if (row.kind === "doc" || row.kind === "code") {
      const ext = row.name.includes(".") ? row.name.split(".").pop().toUpperCase().slice(0, 4) : "TXT";
      return cell.bg(c.surface).border(1).border_color(c.border).text_size(10).text_color(c.muted_foreground).child(ext);
    }
    return cell.text_size(Math.min(w, h) * 0.4).text_color(c.muted_foreground).child(glyph(row.kind)).child(badge);
  }

  /** The matched passage under a result's name: up to two lines, the query's words tinted like a highlighter. */
  snippet(text, words, color, cx) {
    const c = cx.theme().colors;
    const pieces = marks(text, words).flatMap((m) => m.text.split(/(?<= )/).filter(Boolean).map((part) => ({ part, hit: m.hit })));
    return h_flex()
      .w_full()
      .h(38)
      .overflow_hidden()
      .flex_wrap()
      .content_start()
      .text_size(12)
      .children(
        pieces.map(({ part, hit }) =>
          hit
            ? div().flex_none().rounded(2).bg(alpha(c.primary, 0.2)).text_color(c.primary).child(part)
            : div().flex_none().text_color(color).child(part),
        ),
      );
  }

  row(row, index, cx) {
    const c = cx.theme().colors;
    const selected = index === this.selected;
    const words = row.matchedWords;
    const caption = row.snippet !== "" ? plain(row.snippet) : row.reason === "sound" ? `Matched by sound${row.locatorLabel ? " at " + row.locatorLabel : ""}` : "";
    const meta = [row.folder, when(row.mtime), row.kind === "doc" || row.kind === "pdf" ? bytes(row.size) : ""].filter(Boolean).join(" · ");
    return h_flex()
      .id(`row-${row.id}`)
      .w_full()
      .h(ROW - 2)
      .px(10)
      .gap(12)
      .items_center()
      .rounded(cx.theme().radius.md)
      .bg(selected ? c.accent : c.background)
      .hover((s) => s.bg(selected ? c.accent : alpha(c.accent, 0.6)))
      .on_click((_e, cx) => {
        if (this.selected === index) this.openSelected(cx);
        else {
          this.selected = index;
          this.loadPreview(cx);
          cx.notify();
        }
      })
      .child(this.picture(row, THUMB, THUMB_H, cx))
      .child(
        v_flex()
          .flex_1()
          .min_w_0()
          .gap(4)
          .child(
            h_flex()
              .gap(8)
              .items_center()
              .min_w_0()
              .child(div().text_size(13).when(selected, (d) => d.font_semibold()).when(!selected, (d) => d.font_medium()).text_color(selected ? c.accent_foreground : c.foreground).overflow_hidden().whitespace_nowrap().text_ellipsis().child(row.name))
              .child(this.pill(tag(row), cx, selected))
              .child(row.duplicates.length ? this.pill(`+${row.duplicates.length} similar`, cx, false) : div()),
          )
          .child(row.snippet !== "" ? this.snippet(caption, words, selected ? c.foreground : c.muted_foreground, cx) : caption ? div().text_size(12).text_color(c.muted_foreground).child(caption) : div())
          .child(div().text_size(11).text_color(c.muted_foreground).overflow_hidden().whitespace_nowrap().text_ellipsis().child(meta)),
      );
  }

  tile(row, index, cx) {
    const c = cx.theme().colors;
    const selected = index === this.selected;
    const p = this.thumbs.get(thumbKey(row));
    return div()
      .id(`tile-${row.id}`)
      .w(TILE)
      .h(TILE)
      .flex_none()
      .rounded(cx.theme().radius.md)
      .overflow_hidden()
      .bg(c.muted)
      .border(2)
      .border_color(selected ? c.primary : c.background)
      .flex()
      .items_center()
      .justify_center()
      .on_click((_e, cx) => {
        if (this.selected === index) this.openSelected(cx);
        else {
          this.selected = index;
          this.loadPreview(cx);
          cx.notify();
        }
      })
      .relative()
      .child(p ? image(p).size_full() : div().text_size(28).text_color(c.muted_foreground).child(glyph(row.kind)))
      .child(
        row.duplicates.length
          ? div().absolute().top(6).right(6).px(6).py(1).rounded(3).bg(alpha(c.background, 0.8)).text_size(10).text_color(c.foreground).child(`+${row.duplicates.length}`)
          : div(),
      );
  }

  header(cx) {
    const c = cx.theme().colors;
    const s = status();
    const indexing = s.daemon && s.pending > 0 && s.files > 0;
    return v_flex()
      .w_full()
      .child(
        h_flex()
          .w_full()
          .h(48)
          .pl(16)
          .pr(9)
          .gap(10)
          .items_center()
          .child(div().text_size(15).text_color(c.muted_foreground).child(this.like ? "\u{f0c5}" : "\u{f002}"))
          .child(
            this.like
              ? h_flex().flex_1().gap(8).items_center().text_size(14)
                  .child(div().text_color(c.muted_foreground).child("Like"))
                  .child(div().text_color(c.foreground).font_medium().child(this.like.name))
                  .child(div().text_size(11).text_color(c.muted_foreground).child("esc back"))
              : Input.new(this.query)
                  .flex_1()
                  .h(36)
                  .px(4)
                  .border(0)
                  .bg(c.background)
                  .text_size(15)
                  .token((t, cx) => this.chipToken(t, cx)),
          )
          .child(div().text_size(11).text_color(c.muted_foreground).child(this.tookMs !== null && !this.quickShown ? `${Math.round(this.tookMs)} ms` : ""))
          .child(this.barActions(cx)),
      )
      .child(indexing ? this.progressLine(s, cx) : div().h(1).w_full().bg(c.border));
  }

  /** The selected result's two likeliest actions, at the bar's right end: what Enter does, and the next thing
   *  people do with that kind of file (more like a photo or a video frame, the folder of a document). */
  barActions(cx) {
    const row = this.mode === "setup" || this.menu ? null : this.results[this.selected];
    if (!row) return div();
    const c = cx.theme().colors;
    const visual = row.kind === "image" || row.kind === "screenshot" || row.kind === "video";
    const second = visual
      ? { icon: "\u{f0c5}", label: "Similar", keys: "⌃S", run: (cx) => this.moreLikeThis(cx) }
      : { icon: "\u{f07b}", label: "Show in Files", keys: "⌃⏎", run: () => reveal(row.path) };
    return h_flex()
      .gap(6)
      .items_center()
      .child(div().w(1).h(20).mx(2).bg(c.border))
      .child(this.barButton(second, false, cx))
      .child(this.barButton({ label: primaryLabel(row), keys: "⏎", run: (cx) => this.openSelected(cx) }, true, cx));
  }

  barButton(action, primary, cx) {
    const c = cx.theme().colors;
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
      .border_color(c.border)
      .text_color(primary ? c.primary_foreground : c.muted_foreground)
      .child(action.keys);
    return h_flex()
      .id(`bar-${action.label}`)
      .h(30)
      .pl(action.icon ? 10 : 12)
      .pr(7)
      .gap(action.icon ? 7 : 8)
      .items_center()
      .rounded(5)
      .text_size(12)
      .bg(primary ? c.primary : c.background)
      .border(primary ? 0 : 1)
      .border_color(c.border)
      .text_color(primary ? c.primary_foreground : c.foreground)
      .when(primary, (d) => d.font_semibold())
      .hover((st) => st.bg(primary ? alpha(c.primary, 0.85) : c.muted))
      .on_click((_e, cx) => action.run(cx))
      .child(action.icon ? div().text_size(12).text_color(c.muted_foreground).child(action.icon) : div())
      .child(div().child(action.label))
      .child(keycap);
  }

  chipToken(t, cx) {
    const c = cx.theme().colors;
    return h_flex()
      .h(24)
      .px(8)
      .gap(6)
      .items_center()
      .rounded(4)
      .bg(t.selected ? c.primary : c.muted)
      .text_size(12)
      .text_color(t.selected ? c.primary_foreground : c.foreground)
      .child(t.token.label || t.token.text);
  }

  progressLine(s, cx) {
    const c = cx.theme().colors;
    const done = s.files > 0 ? (s.files - s.pending) / s.files : 0;
    return div().w_full().h(2).bg(c.border).child(div().h(2).w(`${Math.round(done * 1000) / 10}%`).bg(c.primary));
  }

  filters(cx) {
    const c = cx.theme().colors;
    return h_flex()
      .w_full()
      .h(38)
      .px(14)
      .gap(6)
      .items_center()
      .border_b(1)
      .border_color(c.border)
      .children(
        FILTERS.map((f, i) => {
          const active = i === this.filter;
          const n = this.searched ? filterCount(f, this.counts) : null;
          const label = n != null && f.kinds[0] !== "code" ? `${f.label} ${count(n)}` : f.label;
          return div()
            .id(`filter-${i}`)
            .h(24)
            .px(10)
            .flex()
            .items_center()
            .rounded(4)
            .border(1)
            .border_color(active ? c.primary : c.border)
            .bg(active ? alpha(c.primary, 0.12) : c.background)
            .text_size(12)
            .text_color(active ? c.primary : n === 0 ? c.muted_foreground : c.foreground)
            .on_click((_e, cx) => {
              this.filter = i;
              this.grid = null;
              cx.notify();
              if (this.text() !== "") {
                this.run(cx, true);
                this.run(cx, false);
              }
            })
            .child(label);
        }),
      )
      .child(div().flex_1())
      .child(div().text_size(11).text_color(c.muted_foreground).child(this.isGrid() ? "grid · ⌃G list" : FILTERS[this.filter].kinds.includes("image") ? "list · ⌃G grid" : "⇥ next filter"));
  }

  list(cx) {
    const results = this.results;
    if (this.isGrid()) {
      const rows = Math.ceil(results.length / this.cols);
      return div()
        .w(this.listW)
        .flex_none()
        .h_full()
        .px(10)
        .py(10)
        .child(
          v_virtual_list("grid", rows, TILE + TILE_GAP, (i) => `g-${i}-${results[i * this.cols]?.id}`, (range, cx) => {
            const out = [];
            for (let r = range.start; r < range.end; r++) {
              const tiles = [];
              for (let k = r * this.cols; k < Math.min(results.length, (r + 1) * this.cols); k++) tiles.push(this.tile(results[k], k, cx));
              out.push(h_flex().gap(TILE_GAP).h(TILE + TILE_GAP).children(tiles));
            }
            return out;
          }).size_full().track_scroll(this.scroll),
        );
    }
    return div()
      .w(this.listW)
      .flex_none()
      .h_full()
      .px(6)
      .py(6)
      .child(
        v_virtual_list("results", results.length, ROW, (i) => `r-${results[i].id}`, (range, cx) => {
          const out = [];
          for (let i = range.start; i < range.end; i++) out.push(this.row(results[i], i, cx));
          return out;
        }).size_full().track_scroll(this.scroll),
      );
  }

  preview(cx) {
    const c = cx.theme().colors;
    const row = this.results[this.selected];
    if (!row) return div().flex_1();
    const data = this.previews.get(thumbKey(row)) ?? {};
    const picture = data.large ?? this.thumbs.get(thumbKey(row));
    const words = row.matchedWords;
    const info = [row.folder, row.locatorLabel && row.kind === "pdf" ? `page ${row.locator}` : "", when(row.mtime), bytes(row.size)].filter(Boolean).join(" · ");
    let body;
    if (data.lines) {
      body = this.linesView(row, data, cx);
    } else if (picture && row.kind !== "doc") {
      const pane = div().flex_1().min_h_0().rounded(cx.theme().radius.md).overflow_hidden().bg(c.surface).flex().items_center().justify_center().child(image(picture).size_full());
      body = v_flex().flex_1().min_h_0().gap(10).child(pane);
      if (data.passage?.text && row.kind === "pdf") body = body.child(this.passageView(row, data.passage, 140, cx));
    } else if (data.passage || row.snippet) {
      body = this.passageView(row, data.passage ?? { text: row.snippet, before: "", after: "" }, null, cx);
    } else {
      body = v_flex()
        .flex_1()
        .items_center()
        .justify_center()
        .gap(8)
        .child(div().text_size(48).text_color(c.muted_foreground).child(glyph(row.kind)))
        .child(div().text_size(12).text_color(c.muted_foreground).child(row.reason === "sound" ? `Matched by sound${row.locatorLabel ? ", " + row.locatorLabel : ""}` : ""));
    }
    return v_flex()
      .flex_1()
      .min_w_0()
      .h_full()
      .px(20)
      .py(16)
      .gap(12)
      .border_l(1)
      .border_color(c.border)
      .child(
        v_flex()
          .gap(3)
          .child(h_flex().gap(8).items_center().child(div().text_size(14).font_medium().text_color(c.foreground).overflow_hidden().whitespace_nowrap().text_ellipsis().child(row.name)).child(this.pill(tag(row), cx, true)).child(row.duplicates.length ? this.pill(`${row.duplicates.length} more like it: ${row.duplicates.map((d) => d.split("/").pop()).slice(0, 3).join(", ")}${row.duplicates.length > 3 ? "…" : ""}`, cx, false) : div()))
          .child(div().text_size(11).text_color(c.muted_foreground).overflow_hidden().whitespace_nowrap().text_ellipsis().child(info)),
      )
      .child(body);
  }

  /** Numbered lines of code or plain text, the matched stretch tinted and its likeliest line (the one holding the
   *  most query words) stronger. Code is coloured by a small tokenizer; text wraps. */
  linesView(row, data, cx) {
    const c = cx.theme().colors;
    const code = row.kind === "code";
    const words = row.matchedWords;
    const inside = (n) => n >= data.first && n <= data.last;
    const best = data.best ?? data.first;
    const lang = code ? language(row.path) : null;
    const palette = syntax();
    const colors = {
      plain: c.foreground,
      keyword: c.primary,
      number: palette?.number ?? c.primary,
      string: palette?.string ?? c.foreground,
      comment: c.muted_foreground,
    };
    // The pane fits about 30 lines: start a little above the strongest line when it would fall below them.
    const from = Math.max(data.lines[0]?.n ?? 1, best - 8);
    return v_flex()
      .flex_1()
      .min_h_0()
      .overflow_hidden()
      .rounded(cx.theme().radius.md)
      .bg(c.surface)
      .py(8)
      .children(
        data.lines.filter((l) => l.n >= from || best - data.first < 22).map((l) => {
          const strength = l.n === best ? 0.18 : inside(l.n) ? 0.1 : 0;
          const gutter = l.n === best ? c.foreground : inside(l.n) ? c.muted_foreground : alpha(c.muted_foreground, 0.5);
          const text = code
            ? h_flex().min_w_0().overflow_hidden().whitespace_nowrap().children(tokens(l.text || " ", lang).map((t) => div().flex_none().text_color(colors[t.kind]).child(t.text)))
            : h_flex().flex_1().min_w_0().flex_wrap().children(marks(l.text || " ", words).flatMap((m) => m.text.split(/(?<= )/).map((part) => div().flex_none().text_color(m.hit ? c.primary : c.foreground).child(part))));
          return h_flex()
            .items_start()
            .px(14)
            .py(code ? 1 : 2)
            .bg(strength ? alpha(c.primary, strength) : c.surface)
            .text_size(12)
            .child(div().w(40).flex_none().text_color(gutter).child(String(l.n)))
            .child(text);
        }),
      );
  }

  passageView(row, p, height, cx) {
    const c = cx.theme().colors;
    const words = row.matchedWords;
    const bold = (text) => marks(plain(text), words).map((m) => (m.hit ? `**${escapeMarkdown(m.text)}**` : escapeMarkdown(m.text))).join("");
    const trim = (t, n, end) => (t.length <= n ? t : end ? "…" + t.slice(t.length - n) : t.slice(0, n) + "…");
    const md = [p.before ? escapeMarkdown(trim(plain(p.before), 400, true)) : "", bold(p.text), p.after ? escapeMarkdown(trim(plain(p.after), 400, false)) : ""].filter(Boolean).join("\n\n");
    const view = TextView.markdown(`passage-${row.id}`, md).selectable(true).scrollable(true);
    const box = div().w_full().rounded(cx.theme().radius.md).bg(c.surface).px(14).py(12).text_size(12).text_color(c.foreground).overflow_hidden().child(view);
    return height ? box.h(height) : box.flex_1().min_h_0();
  }

  menuView(cx) {
    const c = cx.theme().colors;
    const m = this.menu;
    return div()
      .absolute()
      .top(0)
      .left(0)
      .size_full()
      .flex()
      .justify_center()
      .items_start()
      .pt(110)
      .bg(alpha(c.background, 0.6))
      .on_click((_e, cx) => {
        this.menu = null;
        cx.notify();
      })
      .child(
        v_flex()
          .w(440)
          .h_auto()
          .rounded(8)
          .border(1)
          .border_color(c.border)
          .bg(c.surface)
          .shadow_lg()
          .overflow_hidden()
          .child(
            h_flex().gap(10).px(14).py(12).items_center().border_b(1).border_color(c.border)
              .child(this.picture(m.row, 40, 40, cx))
              .child(v_flex().flex_1().min_w_0().gap(2)
                .child(div().text_size(13).font_medium().text_color(c.foreground).overflow_hidden().whitespace_nowrap().text_ellipsis().child(m.row.name))
                .child(div().text_size(11).text_color(c.muted_foreground).overflow_hidden().whitespace_nowrap().text_ellipsis().child(m.row.folder))),
          )
          .child(
            v_flex().p(6).children(
              m.items.map((item, i) => {
                const selected = i === m.selected;
                const label = item.confirm && selected && m.confirm ? "Press ⏎ again to move to trash" : item.label;
                return h_flex()
                  .id(`act-${i}`)
                  .justify_between()
                  .px(10)
                  .py(9)
                  .rounded(4)
                  .bg(selected ? c.accent : c.surface)
                  .text_size(12)
                  .on_click((_e, cx) => {
                    m.selected = i;
                    this.runMenu(cx);
                  })
                  .child(div().text_color(item.danger ? c.destructive : selected ? c.accent_foreground : c.foreground).child(label))
                  .child(div().text_color(c.muted_foreground).child(item.keys));
              }),
            ),
          ),
      );
  }

  empty(cx) {
    const c = cx.theme().colors;
    const s = status();
    if (!s.daemon) return this.message("Omafind isn't running", "systemctl --user start omafindd", cx);
    if (s.roots.length === 0) return this.message("Nothing indexed yet", "Ctrl+, picks the folders", cx);
    if (this.error) return this.message("Search failed", this.error, cx);
    if (this.searched !== "") {
      const hint = FILTERS[this.filter].kinds.length ? "Tab widens the filter" : "Try other words: what it shows, what it says, when it was";
      return this.message("Nothing matched", hint, cx);
    }
    if (s.pending > 0 && s.indexed < s.files * 0.6) return this.indexing(s, cx);
    const r = this.recents;
    const label = (t) => div().text_size(11).text_color(c.muted_foreground).pb(6).child(t);
    const searches = (r?.searches ?? []).slice(0, 7);
    return h_flex()
      .flex_1()
      .min_h_0()
      .items_start()
      .child(
        v_flex()
          .w(380)
          .flex_none()
          .h_full()
          .px(12)
          .py(16)
          .gap(2)
          .border_r(1)
          .border_color(c.border)
          .child(label(searches.length ? "RECENT SEARCHES · ↑ recalls" : "TRY"))
          .children(
            (searches.length ? searches.map((x) => ({ q: x.query, at: when(x.at) })) : [
              { q: "a dog on the beach", at: "" },
              { q: "the clause about pets in the lease", at: "" },
              { q: "where they talk about sleep", at: "" },
              { q: "invoice type:pdf after:2025", at: "" },
            ]).map((x, i) =>
              h_flex()
                .id(`recent-${i}`)
                .justify_between()
                .px(8)
                .py(8)
                .rounded(5)
                .bg(i === this.historyAt ? c.accent : c.background)
                .hover((st) => st.bg(c.accent))
                .on_click((_e, cx) => {
                  this.query.set_value(x.q);
                  this.schedule(cx);
                })
                .child(div().text_size(13).text_color(i === this.historyAt ? c.accent_foreground : c.foreground).overflow_hidden().whitespace_nowrap().text_ellipsis().child(x.q))
                .child(div().text_size(11).text_color(c.muted_foreground).child(x.at)),
            ),
          ),
      )
      .child(
        v_flex()
          .flex_1()
          .min_w_0()
          .h_full()
          .px(18)
          .py(16)
          .gap(18)
          .child(
            v_flex()
              .gap(4)
              .child(label("JUST ADDED"))
              .child(
                h_flex().gap(10).children(
                  (r?.added ?? []).slice(0, 5).map((row, i) =>
                    v_flex()
                      .id(`added-${i}`)
                      .w(104)
                      .gap(5)
                      .on_click(() => open(row.path, row.kind, row.locator))
                      .child(this.picture(row, 104, 104, cx))
                      .child(div().text_size(11).text_color(c.foreground).overflow_hidden().whitespace_nowrap().text_ellipsis().child(row.name))
                      .child(div().text_size(10).text_color(c.muted_foreground).child(when(row.mtime))),
                  ),
                ),
              ),
          )
          .child(
            (r?.opened ?? []).length
              ? v_flex()
                  .gap(2)
                  .child(label("OPENED OFTEN"))
                  .children(
                    r.opened.slice(0, 5).map((row, i) =>
                      h_flex()
                        .id(`opened-${i}`)
                        .justify_between()
                        .py(7)
                        .border_b(1)
                        .border_color(c.muted)
                        .on_click(() => open(row.path, row.kind, row.locator))
                        .child(div().text_size(13).text_color(c.foreground).child(row.name))
                        .child(div().text_size(11).text_color(c.muted_foreground).child(row.folder)),
                    ),
                  )
              : div(),
          )
          .child(div().flex_1())
          .child(
            h_flex().gap(6).text_size(11).text_color(c.muted_foreground)
              .child(div().child("Narrow it while you type:"))
              .children(["in:~/Pictures", "type:pdf", "after:2024", "last week", "code:"].map((t) => div().text_color(c.foreground).child(t))),
          ),
      );
  }

  indexing(s, cx) {
    const c = cx.theme().colors;
    const names = { screenshot: "Screenshots", image: "Photos", pdf: "PDFs", doc: "Documents", audio: "Recordings", video: "Videos", code: "Code" };
    return v_flex()
      .flex_1()
      .items_center()
      .justify_center()
      .child(
        v_flex()
          .w(560)
          .gap(20)
          .child(
            v_flex()
              .gap(6)
              .child(div().text_size(18).font_medium().text_color(c.foreground).child("Reading your files"))
              .child(div().text_size(12).text_color(c.muted_foreground).child(`${count(s.files - s.pending)} of ${count(s.files)}${s.eta != null ? " · " + duration(s.eta) + " left" : ""}. The newest are read first, so search works already.`)),
          )
          .child(
            v_flex().gap(10).children(
              s.kinds.map((k) =>
                h_flex()
                  .gap(12)
                  .items_center()
                  .text_size(12)
                  .child(div().w(110).text_color(c.foreground).child(names[k.kind] ?? k.kind))
                  .child(div().flex_1().h(6).rounded(3).bg(c.muted).child(div().h(6).rounded(3).bg(c.primary).w(`${k.total ? Math.round((k.indexed / k.total) * 100) : 0}%`)))
                  .child(div().w(120).text_color(c.muted_foreground).child(k.indexed === k.total ? `${count(k.total)} done` : `${count(k.indexed)} / ${count(k.total)}`)),
              ),
            ),
          )
          .child(s.failed > 0 ? div().text_size(11).text_color(c.destructive).child(`${count(s.failed)} files couldn't be read · omafind status --failed`) : div()),
      );
  }

  message(title, hint, cx) {
    const c = cx.theme().colors;
    return v_flex()
      .flex_1()
      .items_center()
      .justify_center()
      .gap(6)
      .child(div().text_size(14).text_color(c.foreground).child(title))
      .child(div().text_size(12).text_color(c.muted_foreground).child(hint));
  }

  footer(cx) {
    const s = status();
    const c = cx.theme().colors;
    let left;
    if (!s.daemon) left = "omafindd isn't running";
    else if (s.state === "paused") left = `Paused · ${count(s.indexed)} files`;
    else if (s.state === "paused_power_saver") left = `Paused in power-saver · ${count(s.indexed)} files`;
    else if (s.state === "waiting_for_ac") left = `Waiting for a charger · ${count(s.indexed)} files`;
    else if (this.like) left = `${count(this.results.length)} like ${this.like.name}`;
    else if (this.searched !== "" && this.tookMs !== null) left = `${count(this.results.length)} ${this.results.length === 1 ? "result" : "results"}${s.pending ? ` · indexing, ${count(s.pending)} left` : ""}`;
    else if (s.state === "indexing") left = `Indexing ${count(s.pending)} left${s.current ? " · " + s.current.split("/").pop() : ""}${s.eta != null ? " · " + duration(s.eta) : ""}`;
    else if (s.state === "crawling") left = "Looking for new files";
    else if (s.state === "starting") left = "Starting: watching your folders";
    else left = `${count(s.indexed)} files · up to date`;
    let hints;
    if (this.mode === "setup") hints = "↑↓ move · space toggle · ⏎ start · esc back";
    else if (this.menu) hints = "↑↓ move · ⏎ run · esc back";
    else if (this.results.length) hints = `${this.isGrid() ? "←↑↓→" : "↑↓"} move · ⏎ ${openVerb(this.results[this.selected])} · ⌃S similar · ⌃K actions · esc close`;
    else hints = "↑ last search · ⇥ filter · ⌃, settings · esc close";
    return h_flex()
      .w_full()
      .h(32)
      .px(16)
      .border_t(1)
      .border_color(c.border)
      .justify_between()
      .items_center()
      .text_size(11)
      .text_color(c.muted_foreground)
      .child(div().child(left))
      .child(
        h_flex()
          .gap(12)
          .child(div().child(hints))
          .child(div().id("settings").text_size(12).hover((st) => st.text_color(c.foreground)).on_click((_e, cx) => (this.mode === "setup" ? this.closeSetup(cx) : this.openSetup(cx, false))).child("\u{f013}")),
      );
  }

  /** @param {Context} cx */
  render(cx) {
    const c = cx.theme().colors;
    const s = status();
    if (!this.decided && s.daemon) {
      this.decided = true;
      if (s.roots.length === 0 && !s.configured) cx.timer.after(0, (cx) => this.openSetup(cx, true));
    }
    if (s.shown !== this.shown) {
      // Shown again: fresh field, keyboard on it, recents up to date.
      this.shown = s.shown;
      this.query.set_value("");
      this.results = [];
      this.tookMs = null;
      this.searched = "";
      this.counts = null;
      this.chips = [];
      this.menu = null;
      this.like = null;
      this.historyAt = -1;
      this.focusField(cx);
      cx.timer.after(0, (cx) => this.loadRecents(cx));
    }
    this.layout();
    const root = v_flex()
      .size_full()
      .relative()
      .bg(c.background)
      .font_family(cx.theme().typography.mono)
      .key_context("Omafind")
      .track_focus(this.focus)
      .on_key_down((event, cx) => this.onKey(event, cx))
      .on_action("omafind_next", (_e, cx) => this.onKey({ keystroke: "tab" }, cx))
      .on_action("omafind_previous", (_e, cx) => this.onKey({ keystroke: "shift-tab" }, cx));
    if (this.mode === "setup") return root.child(this.setup.render(cx, this.firstRun)).child(this.footer(cx));
    const body = this.results.length === 0 ? this.empty(cx) : h_flex().flex_1().min_h_0().child(this.list(cx)).child(this.preview(cx));
    return root
      .child(this.header(cx))
      .child(this.filters(cx))
      .child(body)
      .child(this.footer(cx))
      .child(this.menu ? this.menuView(cx) : div());
  }
}

const KEYWORDS = new Set(("as async await break case catch class const continue crate def default defer del do dyn elif else " +
  "enum except export extends false fi final finally fn for from func function go if impl import in interface " +
  "let local loop match mod module mut new nil None not null or package pass pub raise return self Self static " +
  "struct super switch then this throw trait true True False try type typeof undefined unsafe use var where while " +
  "with yield").split(" "));
const HASH_COMMENTS = new Set(["py", "sh", "bash", "zsh", "fish", "rb", "toml", "yaml", "yml", "pl", "r", "nix", "conf", "cfg", "ini", "dockerfile", "mk", "cmake"]);
const DASH_COMMENTS = new Set(["lua", "sql", "hs", "elm"]);

/** The line of the matched stretch holding the most query words (the first on a tie, the stretch's start if none). */
function bestLine(data, words) {
  let best = data.first;
  let most = 0;
  for (const l of data.lines) {
    if (l.n < data.first || l.n > data.last) continue;
    const lower = l.text.toLowerCase();
    const count = words.filter((w) => lower.includes(w.toLowerCase())).length;
    if (count > most) [best, most] = [l.n, count];
  }
  return best;
}

function language(path) {
  const name = path.split("/").pop().toLowerCase();
  return name.includes(".") ? name.split(".").pop() : name;
}

/** One line of code as coloured pieces: comments, strings, numbers, keywords, the rest plain. Line by line, so a
 *  block comment or string that spans lines colours only where it starts; good enough for a preview. */
function tokens(line, lang) {
  const comment = HASH_COMMENTS.has(lang) ? "#" : DASH_COMMENTS.has(lang) ? "--" : "//";
  const singleQuoted = lang !== "rs";
  const out = [];
  const push = (kind, text) => {
    const last = out[out.length - 1];
    if (last && last.kind === kind) last.text += text;
    else out.push({ kind, text });
  };
  let i = 0;
  while (i < line.length) {
    const rest = line.slice(i);
    if (rest.startsWith(comment) || rest.startsWith("/*")) {
      push("comment", rest);
      break;
    }
    const ch = line[i];
    if (ch === '"' || ch === "`" || (ch === "'" && singleQuoted)) {
      let j = i + 1;
      while (j < line.length && line[j] !== ch) j += line[j] === "\\" ? 2 : 1;
      push("string", line.slice(i, j + 1));
      i = j + 1;
      continue;
    }
    const word = /^[A-Za-z_][A-Za-z0-9_]*/.exec(rest);
    if (word) {
      push(KEYWORDS.has(word[0]) ? "keyword" : "plain", word[0]);
      i += word[0].length;
      continue;
    }
    const num = /^\d[\d_]*(\.\d+)?/.exec(rest);
    if (num) {
      push("number", num[0]);
      i += num[0].length;
      continue;
    }
    push("plain", ch);
    i += 1;
  }
  return out;
}

// Where to open: the line the preview found for code and plain text (a document's own locator is a character
// offset, which no app takes), else the page or moment.
function openLocator(row) {
  if (row.openAt != null) return row.openAt;
  return row.kind === "doc" ? null : row.locator;
}

// What Enter does to this row, for the footer.
function openVerb(row) {
  return row ? primaryLabel(row).toLowerCase() : "open";
}

// The bar's main button, what Enter does. Pages and lines go unsaid: opening at the match is simply what Open
// does. A recording or video says where it starts playing, since a moment isn't visible otherwise.
function primaryLabel(row) {
  if ((row.kind === "video" || row.kind === "audio") && row.locator != null) return `Play from ${row.locatorLabel}`;
  return "Open";
}

function thumbKey(row) {
  return `${row.path}#${row.locator ?? ""}`;
}

function hasThumb(kind) {
  return kind === "image" || kind === "screenshot" || kind === "video" || kind === "pdf";
}

const KIND_NAMES = { photo: "Photos", photos: "Photos", image: "Photos", images: "Photos", screenshot: "Screenshots", screenshots: "Screenshots", pdf: "PDFs", doc: "Documents", docs: "Documents", document: "Documents", documents: "Documents", audio: "Audio", recording: "Recordings", recordings: "Recordings", video: "Videos", videos: "Videos", media: "Media", code: "Code" };

/** A chip as people read it: `\u{f07b} ~/Documents`, `Documents`, `\u{f017} since 2025`, `\u{f017} last week`. */
function chipLabel(text) {
  const i = text.indexOf(":");
  if (i < 0) return `\u{f017} ${text}`;
  const key = text.slice(0, i).toLowerCase();
  const value = text.slice(i + 1);
  if (key === "in") {
    const home = homedir();
    return `\u{f07b} ${value.startsWith(home) ? "~" + value.slice(home.length) : value}`;
  }
  if (key === "type" || key === "kind") return KIND_NAMES[value.toLowerCase()] ?? value;
  const word = { after: "since", since: "since", before: "before", on: "on", during: "during" }[key] ?? key;
  return `\u{f017} ${word} ${value}`;
}

/** One paragraph of plain text: line breaks and Markdown heading and list marks out. */
function plain(text) {
  return text.replace(/(^|\s)(#{1,6}|[-*+]|\d+\.)\s+(?=\S)/g, "$1").replace(/\s*\n\s*/g, " ");
}

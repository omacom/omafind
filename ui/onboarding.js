// The first run, in the window: welcome, the model, folders, the shortcut, then indexing. Opening Omafind from the app
// launcher is all it takes; `omafind setup` does the same in a terminal. The choices are the settings screen's
// (`Setup`), shown a few at a time.

import { div } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { setupProgress } from "omafind";
import { button } from "./setup.js";

const STEPS = [
  { key: "welcome", label: "Welcome", only: [] },
  { key: "model", label: "Model", only: [] },
  { key: "folders", label: "Folders", only: ["folders", "code"] },
  { key: "shortcut", label: "Shortcut", only: ["desktop"] },
];

// EmbeddingGemma 2 comes as two files; the page shows them as the one model they are.
const MODEL_FILES = [
  { key: "text", mb: 310, flag: "textModel" },
  { key: "encoders", mb: 555, flag: "mediaModel" },
];

const EXAMPLES = [
  { query: "dog running on the beach", where: "IMG_4102.jpg · ~/Pictures/2024", tag: "photo", glyph: "\u{f03e}" },
  { query: "the clause about pets in the lease", where: "lease-2024.pdf · page 7", tag: "pdf", glyph: "\u{f1c1}" },
  { query: "where they talk about sleep", where: "podcast-ep12.m4a · at 23:40", tag: "said", glyph: "\u{f130}" },
];

export class Onboarding {
  constructor(setup) {
    this.setup = setup;
    this.step = 0;
  }

  start(cx) {
    this.setup.load(cx);
    this.go(0, cx);
  }

  go(step, cx) {
    this.step = step;
    this.setup.only = STEPS[step].only;
    // One section on the step's own page needs no heading of its own.
    this.setup.headings = STEPS[step].key !== "shortcut";
    this.setup.cursor = 0;
    this.setup.error = null;
    cx.notify();
  }

  get key() {
    return STEPS[this.step].key;
  }

  missing() {
    return !this.setup.textModel || !this.setup.mediaModel;
  }

  /** Enter and the primary button: one step forward; on the model step a first press starts the download. */
  next(cx, done) {
    const s = this.setup;
    switch (this.key) {
      case "model":
        if (this.missing() && !s.downloading) {
          s.download(cx);
          return;
        }
        break;
      case "folders":
        if (s.loading) return;
        if (!s.folders.some((f) => f.selected) && !s.code.some((f) => f.selected)) {
          s.error = "Pick at least one folder";
          cx.notify();
          return;
        }
        break;
      case "shortcut":
        s.apply(cx, true, done);
        return;
    }
    this.go(this.step + 1, cx);
  }

  /** Esc and Back: a step back; from the welcome, `later`. */
  back(cx, later) {
    if (this.step > 0) this.go(this.step - 1, cx);
    else later(cx);
  }

  primary() {
    const s = this.setup;
    switch (this.key) {
      case "welcome": return { label: "Get started", enabled: true };
      case "model": return { label: this.missing() && !s.downloading ? "Download" : "Continue", enabled: !s.loading };
      case "folders": return { label: "Continue", enabled: !s.loading };
      default:
        if (s.downloading) return { label: "Waiting for the model…", enabled: false };
        return { label: s.busy === "apply" ? "Starting…" : "Start indexing", enabled: s.canSave(true) };
    }
  }

  note() {
    const s = this.setup;
    if (s.error) return s.error;
    switch (this.key) {
      case "welcome": return "About two minutes to set up";
      case "model": {
        const p = setupProgress();
        if (p && p.message.startsWith("Checking")) return "Checking the download…";
        if (s.downloading) return `Downloading · ${Math.round((this.downloaded() / 865) * 100)} %`;
        return this.missing() ? "865 MB, downloaded once" : "Already on this computer";
      }
      case "folders": {
        if (s.loading) return "Looking at your folders…";
        const on = [...s.folders, ...s.code].filter((f) => f.selected);
        if (!on.length) return "Pick at least one folder";
        const seconds = on.reduce((sum, f) => sum + estimate(f.summary), 0);
        return `${on.length} ${on.length === 1 ? "folder" : "folders"} · ~${span(seconds)} of indexing, newest first`;
      }
      default: {
        if (s.busy === "apply") return "Starting the indexer…";
        if (s.downloading) return "Indexing starts when the model has downloaded";
        return "Then Omafind starts reading your files";
      }
    }
  }

  hints() {
    if (this.key === "welcome") return "⏎ get started · esc later";
    if (this.key === "model") return "⏎ continue · esc back";
    return "↑↓ move · space toggle · ⏎ continue · esc back";
  }

  render(cx, later, done) {
    return v_flex()
      .flex_1()
      .min_h_0()
      .child(this.bar(cx, later, done))
      .child(
        h_flex()
          .flex_1()
          .min_h_0()
          .justify_center()
          .child(this.body(cx)),
      );
  }

  bar(cx, later, done) {
    const c = cx.theme().colors;
    const p = this.primary();
    return h_flex()
      .h(48)
      .flex_none()
      .pl(16)
      .pr(9)
      .gap(10)
      .items_center()
      .border_b(1)
      .border_color(c.border)
      .child(div().text_size(15).text_color(c.primary).child("\u{f002}"))
      .child(div().w(150).text_size(15).font_medium().text_color(c.foreground).child("Omafind"))
      .child(h_flex().flex_1().justify_center().items_center().gap(8).children(STEPS.map((step, i) => this.stepMark(step, i, cx))))
      .child(
        h_flex().w(150).justify_end().gap(6)
          .child(this.step > 0 ? button("Back", "esc", false, true, (cx) => this.back(cx, later), cx) : div())
          .child(button(p.label, "⏎", true, p.enabled, (cx) => this.next(cx, done), cx)),
      );
  }

  stepMark(step, i, cx) {
    const c = cx.theme().colors;
    const at = this.step;
    const dot = div()
      .size(18)
      .flex()
      .items_center()
      .justify_center()
      .rounded(9)
      .border(1)
      .border_color(i <= at ? c.primary : c.border)
      .bg(i < at ? c.primary : c.background)
      .text_size(10)
      .text_color(i < at ? c.primary_foreground : i === at ? c.primary : c.muted_foreground)
      .child(i < at ? "\u{f00c}" : String(i + 1));
    return h_flex()
      .gap(8)
      .items_center()
      .child(
        h_flex().gap(7).items_center().text_size(12)
          .text_color(i === at ? c.foreground : i < at ? c.foreground : c.muted_foreground)
          .when(i === at, (d) => d.font_semibold())
          .child(dot)
          .child(div().child(step.label)),
      )
      .child(i < STEPS.length - 1 ? div().w(28).h(1).bg(c.border) : div());
  }

  heading(title, text, cx) {
    const c = cx.theme().colors;
    return v_flex()
      .gap(10)
      .child(div().text_size(22).font_semibold().text_color(c.foreground).child(title))
      .child(div().text_size(13).text_color(c.muted_foreground).whitespace_normal().child(text));
  }

  body(cx) {
    switch (this.key) {
      case "welcome": return this.welcome(cx);
      case "model": return this.model(cx);
      case "folders":
        return v_flex()
          .w(720)
          .h_full()
          .pt(30)
          .gap(14)
          .child(div().px(12).child(this.heading("What should Omafind read?", "The newest files go first, so search works within minutes. You can change this later in settings.", cx)))
          .child(v_flex().flex_1().min_h_0().child(this.setup.list(cx, true)));
      default:
        return v_flex()
          .w(620)
          .h_full()
          .justify_center()
          .gap(24)
          .child(div().px(26).child(this.heading("Open it from anywhere", "Omafind starts with your session and waits in the background, opening over whatever you're doing. Its window rules go in ~/.config/hypr/omafind.lua either way; the shortcut is up to you.", cx)))
          .child(h_flex().px(26).gap(8).items_center().children(["SUPER", "+", "CTRL", "+", "G"].map((k) => this.keycap(k, cx))))
          .child(v_flex().h(56).child(this.setup.list(cx, true)));
    }
  }

  keycap(k, cx) {
    const c = cx.theme().colors;
    if (k === "+") return div().text_color(c.muted_foreground).child("+");
    return div()
      .px(14)
      .py(10)
      .rounded(6)
      .border(1)
      .border_b(3)
      .border_color(k === "G" ? c.primary : c.border)
      .bg(c.surface)
      .text_size(15)
      .font_semibold()
      .text_color(k === "G" ? c.primary : c.foreground)
      .child(k);
  }

  welcome(cx) {
    const c = cx.theme().colors;
    return v_flex()
      .w(620)
      .h_full()
      .justify_center()
      .gap(28)
      .child(
        v_flex()
          .gap(10)
          .child(div().text_size(26).font_semibold().text_color(c.foreground).child("Find files by describing them"))
          .child(div().text_size(13).text_color(c.muted_foreground).whitespace_normal().child("Say what's in it, what it says or when it was. Omafind finds the photo, the page, the passage, or the moment in a recording.")),
      )
      .child(
        v_flex().gap(2).children(
          EXAMPLES.map((e, i) =>
            h_flex()
              .gap(14)
              .px(12)
              .py(10)
              .items_center()
              .rounded(6)
              .bg(i === 0 ? c.surface : c.background)
              .child(div().size(40).flex_none().flex().items_center().justify_center().rounded(4).border(1).border_color(c.border).text_size(16).text_color(c.muted_foreground).child(e.glyph))
              .child(
                v_flex().flex_1().gap(3)
                  .child(div().text_size(13).text_color(c.foreground).child(`"${e.query}"`))
                  .child(div().text_size(11).text_color(c.muted_foreground).child(e.where)),
              )
              .child(div().px(6).rounded(3).border(1).border_color(c.border).text_size(10).text_color(c.muted_foreground).child(e.tag)),
          ),
        ),
      )
      .child(
        h_flex().gap(10).items_center().text_size(12).text_color(c.muted_foreground)
          .child(div().child("\u{f023}"))
          .child(div().child("Everything runs on this computer. Your files are never uploaded.")),
      );
  }

  /** Megabytes on disk of the model's two files (text model, then the image and audio encoders), counting the one
   *  downloading now by its progress. */
  downloaded() {
    const s = this.setup;
    const p = setupProgress();
    const current = p ? (p.message.includes("encoders") ? "encoders" : "text") : null;
    return MODEL_FILES.reduce((sum, f) => {
      if (s[f.flag] || (current === "encoders" && f.key === "text")) return sum + f.mb;
      return sum + (current === f.key ? f.mb * p.fraction : 0);
    }, 0);
  }

  model(cx) {
    const c = cx.theme().colors;
    const s = this.setup;
    const total = MODEL_FILES.reduce((sum, f) => sum + f.mb, 0);
    const done = this.downloaded();
    const right = !this.missing() ? `${total} MB · done` : s.downloading ? `${Math.round(done)} / ${total} MB` : `${total} MB`;
    return v_flex()
      .w(620)
      .h_full()
      .justify_center()
      .gap(18)
      .child(div().text_size(22).font_semibold().text_color(c.foreground).child("Download the model"))
      .child(
        h_flex()
          .gap(14)
          .p(16)
          .items_center()
          .rounded(8)
          .border(1)
          .border_color(c.border)
          .bg(c.surface)
          .text_size(12)
          .child(div().w(170).font_medium().text_color(c.foreground).child("EmbeddingGemma 2"))
          .child(div().flex_1().h(6).rounded(3).bg(c.muted).child(div().h(6).rounded(3).bg(c.primary).w(`${Math.round((done / total) * 100)}%`)))
          .child(div().w(110).flex().justify_end().text_color(c.muted_foreground).child(right)),
      )
      .child(
        v_flex().px(2).gap(6).text_size(11).text_color(c.muted_foreground)
          .child(div().whitespace_normal().child("Reads text, pictures and sound. Downloaded once, runs on this computer. You can pick folders while it downloads."))
          .child(div().whitespace_normal().child("From Hugging Face (ggml-org/embeddinggemma-2-GGUF), checked against its SHA-256."))
          .child(div().child("Saved in ~/.local/share/omafind/models")),
      );
  }

}

/** "4 photos · ~3 s" → seconds; summaries end in the survey's estimate. */
function estimate(summary) {
  const m = /~([\d.]+) (s|min|h)/.exec(summary);
  if (!m) return 0;
  return Number(m[1]) * { s: 1, min: 60, h: 3600 }[m[2]];
}

function span(seconds) {
  if (seconds < 60) return `${Math.round(seconds)} s`;
  if (seconds < 3600) return `${Math.round(seconds / 60)} min`;
  return `${(seconds / 3600).toFixed(1)} h`;
}

# Working on Dig

Dig is local, on-device semantic file search for Omarchy: describe a thing ("dog on the beach", "the clause about
pets in the lease", "where they talk about sleep") and it finds the photo, PDF page, passage, moment in a recording
or video, or code. The phase-by-phase brief is `PLAN.md` (kept out of git by `.gitignore`, as is
`docs/`). Reference checkouts next to this repo: `../omarchy-quattro` (upstream Omarchy, branch `quattro`),
`../gpui-kit` (GPUI Kit at the pinned revision).

## Status

Phases 0–3 are done, 2026-10-14, on this machine (Omarchy, Hyprland 0.56, Radeon RX 480 on RADV, 8-thread
x86-64): engine spikes; images + plain text; PDFs (poppler `pdftotext`, scanned pages rendered with `pdftoppm`),
screenshots OCR'd with `tesseract`, docx/odt/epub/html read as (zipped) XML, `pandoc` for the rest when installed;
audio as 30 s windows at a 20 s hop and video as scene-change keyframes plus its soundtrack, all through `ffmpeg`.
Phase 4 (daemon) too: `digd` crawls and watches (inotify, 1 s debounce), indexes in a `digd --worker` child that
lingers 60 s after its queue empties, answers searches over `$XDG_RUNTIME_DIR/dig.sock` with a text-only encoder
loaded on demand (unloaded after 10 idle minutes, 2 min after the window hides), pauses in power-saver / on battery
per config (UPower + power-profiles-daemon over D-Bus signals). Measured: a new photo is searchable 2–3 s after it
lands; the daemon idles at 15 MB RSS without the encoder, ~300 MB with it. The power-saver pause is implemented but
untested (polkit refuses `powerprofilesctl` from an agent shell). Phase 5 (the window): `dig-app` is resident;
Hyprland keeps its window in the `special:dig` workspace (packaging/hypr-window-rules.lua) and `dig toggle` flips
that workspace (≈12 ms), like Omarchy's scratchpad console. The app follows Hyprland's event socket for shown/hidden,
subscribes to digd for status, retints live from Omarchy's colors.toml, and shows thumbnails made on demand. Phase 6: `dig setup` (models downloaded in Rust with resume + SHA-256, folder picker with counts and time estimates
through gum, systemd user unit, `~/.config/hypr/dig.lua`), the same setup as a screen in the window (first run, or
Ctrl+, / the footer gear: `ui/setup.js` over `candidates`/`applySetup`/`downloadModels`), `dig doctor`, `dig eval`
(recall@1/@5 over `evals/queries.json`), `dig uninstall`, code index (`code:` prefix, separate `code.sqlite`,
whole-line stretches), `install.sh`, `packaging/PKGBUILD` (bundles the engine tarball into /usr/lib/dig/engine).
Notes with numbers: `docs/phase-0.md`.

Not done / not verified: the real `dig setup` on this machine (the agent never writes the user's Hyprland config;
run it yourself), the power-saver pause (polkit), tree-sitter function boundaries for code (whole-line stretches for
now), a `dig-bin` AUR package from CI releases, the long-file second pass (big PDFs/recordings are capped per file),
`dig search` highlighting of matched words in the window.

## Layout

```
crates/dig-core      prompts, model pins, VECTOR_VERSION; later schema + ranking
crates/dig-embed     bindgen over the engine's llama.h/mtmd.h, image sizing, Embedder, `embed-spike` binary
crates/dig-store     SQLite index: files, chunks, vec0 vectors (cosine), external-content FTS5, roots, meta
crates/dig-extract   classify by extension; readers → chunks: name, image, passages, pdf pages (+ rendered scans),
                     OCR text, office/html text, audio windows, video frames (ffmpeg); scratch under ~/.cache/dig
crates/dig-index     rules + crawler (ignore crate, gitignore only inside repos), indexer (rounds, batched texts), search
crates/dig-client    socket protocol (JSON lines: search, status, pause, resume, reindex, warm, idle, subscribe,
                     set_roots, shutdown) and the client
crates/digd          daemon: coordinator (daemon.rs), worker child (worker.rs), watcher, power, searcher, server
crates/dig-cli       `dig index|search|status|pause|resume|watch|toggle|setup|doctor|eval|uninstall`
                     (setup.rs, doctor.rs, eval.rs, download.rs, hypr.rs; through digd when it runs, else in-process)
systemd/digd.service user unit: Nice=10, IOSchedulingClass=idle, CPUWeight=20, MemoryHigh=3G
crates/dig-app       GPUI Shell host: window (app id `dig`), `dig` host module (search/thumb/status/open/reveal/
                     copy/hide), hypr.rs (Lua dispatchers over the IPC sockets), theme.rs (colors.toml → tokens,
                     watched), thumbs.rs (ffmpegthumbnailer/pdftoppm → ~/.cache/dig/thumbs), actions.rs
ui/                  QuickJS UI scripts (gpui-shell.json, main.js, kinds.js); gpui-kit.d.ts is generated, not committed
packaging/           PKGBUILD, dig.desktop, dig.lua (what `dig setup` writes to ~/.config/hypr/dig.lua)
install.sh           from-checkout install for one user, then `dig setup`
scripts/             fetch-engine.sh (prebuilt llama.cpp), fetch-models.sh
evals/               make-testbed.sh; evals/testbed/ holds local test media (ignored)
```

## Dependencies: nothing compiled, nothing patched, nothing of our own

The engine is llama.cpp's **own prebuilt release**: every tag publishes `llama-<tag>-bin-ubuntu-{vulkan,cuda-*,
rocm-*}-{x64,arm64}.tar.gz` with `libllama.so`, `libmtmd.so`, `libggml*.so` and the backends as loadable modules
(`GGML_BACKEND_DL`, every CPU variant). `scripts/fetch-engine.sh` puts one tag's libraries plus its headers under
`~/.local/share/dig/engine/<tag>-<flavor>` and links `current`; `dig-embed`'s `build.rs` builds against `current`
(or `DIG_LLAMA_DIR`, or an installed package via `pkg-config llama` as a last resort) and bakes an rpath to it.
The final `dig-bin` AUR package bundles the same tarball at the same tag under `/usr/lib/dig/engine`
(`DIG_LLAMA_RPATH` at build time), so installing Dig is seconds, not a 20-minute compile. The AUR
`llama.cpp-*-git` packages compile from source and conflict with Arch's `ggml`; the `-bin` ones repackage these
tarballs without headers. Neither is needed.

Consequences to design for:

- The engine tag is pinned in `scripts/fetch-engine.sh` and `packaging/PKGBUILD` (`b11635`) and bumped deliberately: `embed-spike golden
  evals/reference` must stay ≥ 0.999 on every input, else bump `VECTOR_VERSION` (every index re-embeds). The index
  records `dig_embed::LLAMA_VERSION` and `VECTOR_VERSION`. Tags before `b11584` allocate a logits buffer per output
  token for embedding models (~1 MiB each with Gemma's 262k vocabulary): never go below it.
- Models: `ggml-org/embeddinggemma-2-GGUF` @ `bfcd298762cc34d0357ece5ebdd31791a3a374d8`:
  `embeddinggemma-2-Q8_0.gguf` (309,855,456 B) and `mmproj-embeddinggemma-2-Q8_0.gguf` (554,821,024 B); SHA-256s in
  `scripts/fetch-models.sh`. Installed to `~/.local/share/dig/models` (`$DIG_MODELS` overrides). Never the F16 files:
  activations overflow float16. The model revision is pinned because it defines the vectors; the engine is not.
- GPUI Kit / GPUI Shell is a Cargo git dependency pinned at `50dac1cafb34a4d453c92715219c0458d9a4727b` (v0.7.3 line,
  shell milestone M0, not on crates.io, API expected to change). A PKGBUILD vendors Cargo deps as usual. Keep every
  host-module/runtime call inside `crates/dig-app`.
- Prompts (model card): query `task: search result | query: <text>`, code query `task: code retrieval | query:`,
  document `title: <title or none> | text: <text>`. Media inputs take no prompt. Mean pooling, L2-normalized.

## Building and running

```bash
scripts/fetch-engine.sh [vulkan|cpu|cuda-13.4|rocm-10.0]   # prebuilt llama.cpp + headers, seconds
scripts/fetch-models.sh            # resumable, size + SHA-256 checked; reuses the HF cache
cargo build --release -p dig-embed && cargo build -p dig-app
cargo run --release -p dig-embed --bin embed-spike -- demo [--cpu]
cargo run --release -p dig-embed --bin embed-spike -- golden evals/reference [--write]
cargo run -p dig-app -- --dev      # hot-reloads ui/ on save, writes ui/gpui-kit.d.ts
cargo test --release
```

`sudo` needs a password in agent sessions: ask the user to install packages. Already on Omarchy: `ffmpeg`,
`tesseract`, `mpv`, `imv`, `evince`, `gum`, `wtype`, `grim`.

## Throughput (measured 2026-10-15, RX 480 / 8 threads)

Text: ~15 passages/s (~350 tokens each) on the GPU whatever the batch size (2k–16k tokens; flash attention is on by
default and matters: off is 2–4× slower); 1.6/s on the CPU. Photos at 140 tokens: 0.73 s each on the GPU, 13.6 s on
the CPU. So the GPU is the default and the model is the floor; the indexer overlaps extraction (OCR, pdftotext,
ffmpeg) with embedding on a 4-thread pool. `embed-spike bench` reproduces the numbers; `DIG_KV_UNIFIED=0` and
`DIG_FLASH=0|1` are experiment knobs.

## Ranking (dig-core::ranking)

Query → prompted embed → KNN top 200 (`vec0`, cosine) + FTS5 top 200 over the query's content words (stopwords
dropped, OR-joined, BM25). Vector hits are gated by z-score within their modality (≥ 1.5, top 3 always kept).
Reciprocal rank fusion (k = 60); a keyword hit counts by the square of the share of query words it holds; every word
present adds a small bonus. Then per file: best chunk + half the rest, represented by the best content chunk (not
the file-name chunk). Snippets open where the rarest query words cluster. Measured on the testbed: every query
ranks its file first; a search is ~50 ms warm.

## Things that bit us

- A `.gitignore` in a parent folder (a dotfiles repo in `$HOME` with `*`) would hide everything: the crawler honours
  gitignore only inside git repositories (`require_git(true)`), and inside those only screenshots are read anyway.

- **gpui-kit through git needs two extra steps**, both documented in `Cargo.toml`: repeat its
  `[patch.crates-io] rquickjs = { git = …gpui-kit, rev = … }` (its workspace patch to `crates/shell/rquickjs-compat`
  is not inherited), and keep `pkcs1 0.8.0-rc.4` / `rsa 0.10.0-rc.18` in `Cargo.lock` (newer prereleases break
  LLRT's crypto module). If `cargo update` touches those, re-pin with `cargo update -p <crate>@<ver> --precise <ver>`.
- `[profile.dev.package]` sets `opt-level = 3` for `quickjs-jit-*` and `gpui-pre*`, else script renders run ~3× slower.
- `llama-cpp-2` / `llama-cpp-sys-2` (checked 0.1.159, 2026-10-07) do wrap `mtmd` and have `system-ggml` and
  `dynamic-backends` features, but the llama.cpp they bundle is ggml 0.24 era: before EmbeddingGemma 2 (b11452+), no
  `llama_batch_ext`/`llama_process`, and incompatible with Arch's ggml 0.26. They also compile llama.cpp inside cargo
  rather than using an installed one. Revisit when a release bundles ≥ b11452: then `system-ggml` against the official
  `ggml` + `ggml-vulkan` packages could replace the AUR `llama.cpp-*-git` dependency and our bindgen.
- Images must be resized in Rust (`dig_embed::sizing`, bit-exact Pillow bicubic, sides rounded down to 48 px) with
  `image_min_tokens = 1` so mtmd never rescales; that is what makes vectors match the reference.
- llama.cpp logs "audio input is in experimental stage" on every load; harmless.
- Short queries embed faster on CPU (~22 ms) than on this GPU (~35 ms); media is 7× faster on the GPU. Measure both
  at first run and pick the query-encoder device.

## Hyprland 0.56 (Lua config) from Dig

Dispatchers are Lua: `hl.dsp.workspace.toggle_special("dig")`, `hl.dsp.window.move({ workspace = "special:dig",
follow = false, window = "class:dig" })`, `hl.dsp.window.float({ window = "class:dig", action = "on" })`; the old
`togglespecialworkspace` spelling is gone (`focuswindow class:dig` still works). Over the IPC socket the command is
`dispatch <lua>`. Events on `.socket2.sock`: `activespecial>>special:dig,eDP-1` when opened, `activespecial>>,eDP-1`
when closed. Dig's rules, binding and autostart live in one file, `~/.config/hypr/dig.lua`, loaded by
`require("hypr.dig") -- dig` appended to `~/.config/hypr/hyprland.lua`: Omarchy's user `hyprland.lua` requires
`hypr.bindings` and `hypr.autostart` but not every version loads a `windows.lua`, so a file of our own with its own
require is the only arrangement that works everywhere. `dig uninstall` removes both.

## GPUI Shell notes for the UI (Phase 5)

- Host modules are call-only: `HostModule::new("dig").function(...)/.async_function(...)/.declarations(ts)`,
  `export_module` before `ShellRuntime::new_with_components`. Scripts `import { search } from "dig"`. Values are
  null/bool/number/string/array/object (`HostObject::new().field(..)`). No host → script push: use
  `runtime.refresh(&root, cx)` after host state changes, or a long-poll async function.
- Nothing holds keyboard focus when the window opens; the root View calls `this.focus.focus()` then
  `window.focus_next()` (a tick later) to land on the `Input`. Done again on every show.
- `image(path)` only serves the application directory; `dig-app` installs its own `AssetSource` that also serves
  absolute paths under `~/.cache/dig/thumbs`. `uniform_list` ignores `track_scroll`; `v_virtual_list` (gpui-base)
  with a fixed row height takes a `VirtualListScrollHandle` and `scroll_to_item`.
- Modules: `div`, `image`, `svg`, style methods from `gpui-kit`; `v_flex`, `h_flex`, `Input`, `InputState`,
  `v_virtual_list`, `VirtualListScrollHandle` from `gpui-base`. The generated `ui/gpui-kit.d.ts` is the reference.
- Anything that must outlive the `application().run(...)` closure (watchers) must be leaked or stored globally:
  the closure runs once and drops its captures.
- Keystroke spelling is GPUI's: `escape`, `down`, `enter`, `ctrl-n`. Handle keys once (root `on_key_down`); an
  `InputState` `submit` hook as well fires Enter twice.
- `hide` closes the special workspace through Hyprland; the process and window stay.
- Theme tokens: `cx.theme().colors.{background,foreground,surface,border,input,muted,muted_foreground,primary,…}`.
- Set `tracing` filter `dig_app=info,gpui_shell=info,gpui_kit::shell::script=info` to see script errors.

## Running the daemon in isolation (tests)

```bash
export XDG_RUNTIME_DIR=/tmp/x/run XDG_CONFIG_HOME=/tmp/x/config DIG_INDEX=/tmp/x/index.sqlite
# /tmp/x/config/dig/config.toml: [index] folders = ["/tmp/x/testbed"]
./target/release/digd &            # socket at $XDG_RUNTIME_DIR/dig.sock
./target/release/dig status; ./target/release/dig watch; ./target/release/dig search "…"
pkill -x digd                      # SIGTERM: quits the worker, removes the socket
```

## Driving the UI from an agent shell

The tool shell has no Wayland env. Before launching anything graphical:

```bash
export XDG_RUNTIME_DIR=/run/user/1000 WAYLAND_DISPLAY=wayland-1 HYPRLAND_INSTANCE_SIGNATURE=$(ls /run/user/1000/hypr | head -1)
```

Then `./target/debug/dig-app --dev &`, `hyprctl clients -j` (class `dig`), `hyprctl dispatch focuswindow class:dig`,
`wtype -k Tab; wtype 'query'`, `grim -o eDP-1 shot.png` (3840×2160, scale 1.6), stop with `pkill -x dig-app`. Never
`pkill -f`: it matches the tool's own shell.

## Conventions

- Bash in Omarchy style: `#!/bin/bash`, `set -euo pipefail`, `[[ ]]`/`(( ))`, two-space indent. Our commands are
  `dig`/`digd`; only first-party Omarchy commands use the `omarchy-` prefix. Reuse what Omarchy ships (`ffmpeg`,
  `tesseract`, `mpv`, `imv`, `evince`, `gum`, `omarchy-launch-editor`, `omarchy-hw-*`). No polling: inotify, D-Bus,
  socket pushes.
- Rust edition 2024, workspace deps in the root `Cargo.toml`. Comments say why, not what.
- Commit only when the user asks; never add a Claude co-author trailer or "Generated with" line.
- Keep third-party media out of git (`evals/testbed/` is ignored); commit synthetic inputs only.

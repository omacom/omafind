# Working on Omafind

Omafind is local, on-device semantic file search for Omarchy: describe a thing ("dog on the beach", "the clause about
pets in the lease", "where they talk about sleep") and it finds the photo, PDF page, passage, moment in a recording
or video, or code. The review and the roadmap (phases 7–10, progress log) live in the "Omafind — architecture review and
roadmap" doc (claude.ai artifact c3ab6fa1-12a7-4c8c-8408-3570e457aec1); the UI design is the "Omafind UI redesign" canvas
(claude.ai/artifact/FHmvhSzna65LQ5H3ZRBF9M). `PLAN.md` (the original phase 0–6 brief) is retired; `docs/` stays out
of git. Reference checkouts next to this repo: `../omarchy-quattro` (upstream Omarchy, branch `quattro`),
`../gpui-kit` (GPUI Kit at the pinned revision).

## Status

Phases 0–3 are done, 2026-10-14, on this machine (Omarchy, Hyprland 0.56, Radeon RX 480 on RADV, 8-thread
x86-64): engine spikes; images + plain text; PDFs (poppler `pdftotext`, scanned pages rendered with `pdftoppm`),
screenshots OCR'd with `tesseract`, docx/odt/epub/html read as (zipped) XML, `pandoc` for the rest when installed;
audio as 30 s windows at a 20 s hop and video as scene-change keyframes plus its soundtrack, all through `ffmpeg`.
Phase 4 (daemon) too: `omafindd` crawls and watches (inotify, 1 s debounce), indexes in a `omafindd --worker` child that
lingers 60 s after its queue empties, answers searches over `$XDG_RUNTIME_DIR/omafind.sock` with a text-only encoder
loaded on demand (unloaded after 10 idle minutes, 2 min after the window hides), pauses in power-saver / on battery
per config (UPower + power-profiles-daemon over D-Bus signals). Measured: a new photo is searchable 2–3 s after it
lands; the daemon idles at 15 MB RSS without the encoder, ~300 MB with it. The power-saver pause is implemented but
untested (polkit refuses `powerprofilesctl` from an agent shell). Phase 5 (the window): `omafind-app` is resident;
Hyprland keeps its window in the `special:omafind` workspace (packaging/hypr-window-rules.lua) and `omafind toggle` flips
that workspace (≈12 ms), like Omarchy's scratchpad console. The app follows Hyprland's event socket for shown/hidden,
subscribes to omafindd for status, retints live from Omarchy's colors.toml, and shows thumbnails made on demand. Phase 6: `omafind setup` (models downloaded in Rust with resume + SHA-256, folder picker with counts and time estimates
with cliclack prompts, systemd user unit, `~/.config/hypr/omafind.lua`), the same setup as a screen in the window (first run, or
Ctrl+, / the footer gear: `ui/setup.js` over `candidates`/`applySetup`/`downloadModels`), `omafind doctor`, `omafind eval`
(recall@1/@5 over `evals/queries.json`), `omafind uninstall`, code index (`code:` prefix, separate `code.sqlite`,
whole-line stretches), `install.sh`, `packaging/PKGBUILD` (bundles the engine tarball into /usr/lib/omafind/engine).
Notes with numbers: `docs/phase-0.md`.

Phase 7 (search speed) and most of 8–9, 2026-10-15: vectors moved out of sqlite-vec into an in-memory int8 matrix
with a float32 rescore (`omafind-store/src/vectors.rs`), schema 3 (int8 rows in `vectors`, float32 apart in
`vectors_full`; schemas 1 and 2 convert in place, nothing re-embeds), a
trigram FTS5 index on file names, a keyword-only quick stage, per-session supersede and a query-vector cache in
`omafindd`, recency-first indexing, square thumbnails made while indexing (`omafind-extract/src/thumbs.rs`), EXIF capture
dates (`omafind-extract/src/exif.rs`), inline filters (`omafind-core/src/query.rs`: `in:`, `type:`, `after:`, `before:`,
`on:`, "last week"…), frecency, find similar, near-duplicate folding, `omafind bench`, `omafind status --failed`. The window:
list + preview pane, photo grid, filter chips with counts, filters as chips inside the field, Ctrl+K actions,
recents, sized to the monitor on every show (`hypr::fit_window`).

Not done / not verified: the real `omafind setup` on this machine (the agent never writes the user's Hyprland config;
run it yourself), the power-saver pause (polkit), tree-sitter function boundaries for code (whole-line stretches for
now), a `omafind-bin` AUR package from CI releases, the long-file second pass (big PDFs/recordings are capped per file),
`omafind search` highlighting of matched words in the window.

## Layout

```
crates/omafind-core      prompts, model pins, VECTOR_VERSION; later schema + ranking
crates/omafind-embed     bindgen over the engine's llama.h/mtmd.h, image sizing, Embedder, `embed-spike` binary
crates/omafind-store     SQLite index: files, chunks, vectors (int8; f32 in vectors_full; change log), external-content FTS5 over
                     chunks, trigram FTS5 over file names, roots, meta, searches/opens history; vectors.rs is the
                     in-memory matrix (VectorIndex)
crates/omafind-extract   classify by extension; readers → chunks: name, image, passages, pdf pages (+ rendered scans),
                     OCR text, office/html text, audio windows, video frames (ffmpeg); scratch under ~/.cache/omafind
crates/omafind-index     rules + crawler (ignore crate, gitignore only inside repos), indexer (rounds, batched texts), search
crates/omafind-client    socket protocol (JSON lines: search, status, pause, resume, reindex, warm, idle, subscribe,
                     set_roots, shutdown) and the client
crates/omafindd          daemon: coordinator (daemon.rs), worker child (worker.rs), watcher, power, searcher, server
crates/omafind-cli       `omafind index|search|status|pause|resume|watch|toggle|setup|doctor|eval|bench|uninstall`
                     (setup.rs, doctor.rs, eval.rs, download.rs, hypr.rs; through omafindd when it runs, else in-process)
systemd/omafindd.service user unit: Nice=10, IOSchedulingClass=idle, CPUWeight=20, MemoryHigh=3G
crates/omafind-app       GPUI Shell host: window (app id `omafind`), `omafind` host module (search/thumb/status/open/reveal/
                     copy/hide), hypr.rs (Lua dispatchers over the IPC sockets), theme.rs (colors.toml → tokens,
                     watched), thumbs.rs (ffmpegthumbnailer/pdftoppm → ~/.cache/omafind/thumbs), actions.rs
ui/                  QuickJS UI scripts (gpui-shell.json, main.js, kinds.js); gpui-kit.d.ts is generated, not committed
packaging/           PKGBUILD, omafind.desktop, omafind.lua (what `omafind setup` writes to ~/.config/hypr/omafind.lua)
install.sh           from-checkout install for one user, then `omafind setup`
scripts/             fetch-engine.sh (prebuilt llama.cpp), fetch-models.sh
evals/               make-testbed.sh; evals/testbed/ holds local test media (ignored)
```

## Dependencies: nothing compiled, nothing patched, nothing of our own

The engine is llama.cpp's **own prebuilt release**: every tag publishes `llama-<tag>-bin-ubuntu-{vulkan,cuda-*,
rocm-*}-{x64,arm64}.tar.gz` with `libllama.so`, `libmtmd.so`, `libggml*.so` and the backends as loadable modules
(`GGML_BACKEND_DL`, every CPU variant). `scripts/fetch-engine.sh` puts one tag's libraries plus its headers under
`~/.local/share/omafind/engine/<tag>-<flavor>` and links `current`; `omafind-embed`'s `build.rs` builds against `current`
(or `OMAFIND_LLAMA_DIR`, or an installed package via `pkg-config llama` as a last resort) and bakes an rpath to it.
The final `omafind-bin` AUR package bundles the same tarball at the same tag under `/usr/lib/omafind/engine`
(`OMAFIND_LLAMA_RPATH` at build time), so installing Omafind is seconds, not a 20-minute compile. The AUR
`llama.cpp-*-git` packages compile from source and conflict with Arch's `ggml`; the `-bin` ones repackage these
tarballs without headers. Neither is needed.

Consequences to design for:

- The engine tag is pinned in `scripts/fetch-engine.sh` and `packaging/PKGBUILD` (`b11635`) and bumped deliberately: `embed-spike golden
  evals/reference` must stay ≥ 0.999 on every input, else bump `VECTOR_VERSION` (every index re-embeds). The index
  records `omafind_embed::LLAMA_VERSION` and `VECTOR_VERSION`. Tags before `b11584` allocate a logits buffer per output
  token for embedding models (~1 MiB each with Gemma's 262k vocabulary): never go below it.
- Models: `ggml-org/embeddinggemma-2-GGUF` @ `bfcd298762cc34d0357ece5ebdd31791a3a374d8`:
  `embeddinggemma-2-Q8_0.gguf` (309,855,456 B) and `mmproj-embeddinggemma-2-Q8_0.gguf` (554,821,024 B); SHA-256s in
  `scripts/fetch-models.sh`. Installed to `~/.local/share/omafind/models` (`$OMAFIND_MODELS` overrides). Never the F16 files:
  activations overflow float16. The model revision is pinned because it defines the vectors; the engine is not.
- GPUI Kit / GPUI Shell is a Cargo git dependency pinned at `50dac1cafb34a4d453c92715219c0458d9a4727b` (v0.7.3 line,
  shell milestone M0, not on crates.io, API expected to change). A PKGBUILD vendors Cargo deps as usual. Keep every
  host-module/runtime call inside `crates/omafind-app`.
- Prompts (model card): query `task: search result | query: <text>`, code query `task: code retrieval | query:`,
  document `title: <title or none> | text: <text>`. Media inputs take no prompt. Mean pooling, L2-normalized.

## Building and running

The `justfile` holds the everyday commands (`just` lists them): `bootstrap` (engine + models), `build`, `dev`,
`check` (fmt, clippy `-D warnings`, tests), `golden`, `bench`, `eval`, `sandbox FOLDER` / `sandbox-app` /
`sandbox-stop`, `demo` / `demo-app` (a realistic fake home from `evals/make-home.sh` as `$HOME`: use it for
screenshots and UI checks, so paths and content look like real use), `install`. Underneath:

```bash
scripts/fetch-engine.sh [vulkan|cpu|cuda-13.4|rocm-10.0]   # prebuilt llama.cpp + headers, seconds
scripts/fetch-models.sh            # resumable, size + SHA-256 checked; reuses the HF cache
cargo build --release -p omafind-embed && cargo build -p omafind-app
cargo run --release -p omafind-embed --bin embed-spike -- demo [--cpu]
cargo run --release -p omafind-embed --bin embed-spike -- golden evals/reference [--write]
cargo run -p omafind-app -- --dev      # hot-reloads ui/ on save, writes ui/gpui-kit.d.ts
cargo nextest run --release   # or `just test`; nextest skips doctests: cargo test --doc
```

`sudo` needs a password in agent sessions: ask the user to install packages. Already on Omarchy: `ffmpeg`,
`tesseract`, `mpv`, `imv`, `evince`, `gum`, `wtype`, `grim`.

## Throughput (measured 2026-10-15, RX 480 / 8 threads)

Text: ~15 passages/s (~350 tokens each) on the GPU whatever the batch size (2k–16k tokens; flash attention is on by
default and matters: off is 2–4× slower); 1.6/s on the CPU. Photos at 140 tokens: 0.73 s each on the GPU, 13.6 s on
the CPU. So the GPU is the default and the model is the floor; the indexer overlaps extraction (OCR, pdftotext,
ffmpeg) with embedding on a 4-thread pool. `embed-spike bench` reproduces the numbers; `OMAFIND_KV_UNIFIED=0` and
`OMAFIND_FLASH=0|1` are experiment knobs.

## Ranking (omafind-core::ranking)

The query is parsed first (`omafind_core::query`): filters come out, the rest is searched. Query → prompted embed (cached)
→ int8 scan of the matrix with kind/folder/date filters applied per row, top 400 rescored in float32 → top 200, plus
FTS5 top 1000 over the query's content words (stopwords dropped, OR-joined, BM25; one query serves both the mixed
list and a kind filter) and file names holding every word as typed (trigram index, half words too). Vector hits are gated by z-score within their modality (≥ 1.5; the best always kept, the next two only within
0.04 of it).
Reciprocal rank fusion (k = 60); a keyword hit counts by the square of the share of query words it holds; every word
present adds a small bonus. Names are a third RRF list. Then per file: best chunk + half the rest, represented by the best content chunk (not
the file-name chunk); files opened from the window get up to +30 % (frecency, decaying weekly); photos at cosine
≥ 0.88 to one already listed fold into it (copies, crops, edits, bursts measure ≥ 0.91, different photos ≤ 0.7). Snippets open where the rarest query words cluster. `omafind eval` on a testbed copy outside git: recall@1 0.86, @5 0.93 (the `dentist` query's
file isn't in the testbed; `security deposit returned` ranks 4th because RRF ignores how weak a vector hit is, as
before the rewrite). `omafind bench` at 300k synthetic chunks: quick stage 16 ms, full 29 ms, photo filter 37 ms p50
(sqlite-vec's scalar scan alone took ~110 ms per 100k).

## Things that bit us

- **The real daemon and window run on this machine** (`omafindd.service`, `omafind-app` in `special:omafind`). AGENTS' old
  `pkill -x omafindd` kills the service too, and it stays dead: stop test processes by PID. A test `omafind-app` must be told
  `--ui ui/` (it prefers the installed `~/.local/share/omafind/ui`) and `OMAFIND_SOCKET` (the test daemon's socket), and
  `hyprctl dispatch` by window *address*, not `class:omafind`, which also matches the real window.
- **Driving the window with `wtype`**: Escape hides the workspace and the keyboard goes back to whatever the user
  was in, so keys typed after it land there. Check `hyprctl activewindow` first; never send Escape in a sequence.
- **External-content FTS5 and triggers**: a migration that fills a column while its update trigger exists issues
  `delete` for rows the index never had, and the table is corrupt ("database disk image is malformed"). Fill, then
  `rebuild`, then create the triggers.
- **Binaries must carry the engine's rpath.** `cargo:rustc-link-arg` in omafind-embed's build.rs only reaches that
  crate's own binaries (`embed-spike`); until 2026-10-15 `digd` and the window loaded whatever `libllama.so` the
  system had (the AUR `llama.cpp-vulkan-git`, a different tag). Now omafind-embed publishes the path as `links`
  metadata and omafindd, omafind-cli and omafind-app bake it in from `DEP_LLAMA_RPATH` in their own build.rs. Check
  with `ldd target/release/omafindd | grep llama`; build.rs warns when it falls back to a system llama.cpp.
- **This machine's `/` is a spinning disk** (LUKS + btrfs, zstd), so a cold page cache shows what layout costs.
  With the float32 copy in the same row as the int8 one (schema 2) every row took a page and loading the matrix read
  the whole table: 5.6 s per 100k rows cold, 0.14 s warm. Split (schema 3): 0.4 s cold. The floats are then read
  once in the background (`Searcher::prefetch`, 2.2 s per 100k cold), else each early search pays ~1.5 s of
  scattered reads to rescore. Measure cold with `posix_fadvise(DONTNEED)` on the file after a `sync`, never in the
  scratchpad (tmpfs).
- The bundled SQLite has no math functions (`pow` fails at prepare time).
- Every lookup a search makes per result needs an index: `vectors(file_id)` missing made the photo filter 70× slower
  at 300k chunks. Run `omafind bench` after any change to ranking.

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
- Images must be resized in Rust (`omafind_embed::sizing`, bit-exact Pillow bicubic, sides rounded down to 48 px) with
  `image_min_tokens = 1` so mtmd never rescales; that is what makes vectors match the reference.
- llama.cpp logs "audio input is in experimental stage" on every load; harmless.
- Short queries embed faster on CPU (~22 ms) than on this GPU (~35 ms); media is 7× faster on the GPU. Measure both
  at first run and pick the query-encoder device.

## Hyprland 0.56 (Lua config) from Omafind

Dispatchers are Lua: `hl.dsp.workspace.toggle_special("omafind")`, `hl.dsp.window.move({ workspace = "special:omafind",
follow = false, window = "class:omafind" })`, `hl.dsp.window.float({ window = "class:omafind", action = "on" })`; the old
`togglespecialworkspace` spelling is gone (`focuswindow class:omafind` still works). Over the IPC socket the command is
`dispatch <lua>`. Events on `.socket2.sock`: `activespecial>>special:omafind,eDP-1` when opened, `activespecial>>,eDP-1`
when closed. Omafind's rules, binding and autostart live in one file, `~/.config/hypr/omafind.lua`, loaded by
`require("hypr.omafind") -- omafind` appended to `~/.config/hypr/hyprland.lua`: Omarchy's user `hyprland.lua` requires
`hypr.bindings` and `hypr.autostart` but not every version loads a `windows.lua`, so a file of our own with its own
require is the only arrangement that works everywhere. `omafind uninstall` removes both.

## GPUI Shell notes for the UI (Phase 5)

- Host modules are call-only: `HostModule::new("omafind").function(...)/.async_function(...)/.declarations(ts)`,
  `export_module` before `ShellRuntime::new_with_components`. Scripts `import { search } from "omafind"`. Values are
  null/bool/number/string/array/object (`HostObject::new().field(..)`). No host → script push: use
  `runtime.refresh(&root, cx)` after host state changes, or a long-poll async function.
- Nothing holds keyboard focus when the window opens; the root View calls `this.focus.focus()` then
  `window.focus_next()` (a tick later) to land on the `Input`. Done again on every show.
- `image(path)` only serves the application directory; `omafind-app` installs its own `AssetSource` that also serves
  absolute paths under `~/.cache/omafind/thumbs`. `uniform_list` ignores `track_scroll`; `v_virtual_list` (gpui-base)
  with a fixed row height takes a `VirtualListScrollHandle` and `scroll_to_item`.
- Modules: `div`, `image`, `svg`, style methods from `gpui-kit`; `v_flex`, `h_flex`, `Input`, `InputState`,
  `v_virtual_list`, `VirtualListScrollHandle` from `gpui-base`. The generated `ui/gpui-kit.d.ts` is the reference.
- Anything that must outlive the `application().run(...)` closure (watchers) must be leaked or stored globally:
  the closure runs once and drops its captures.
- Keystroke spelling is GPUI's: `escape`, `down`, `enter`, `ctrl-n`. Handle keys once (root `on_key_down`); an
  `InputState` `submit` hook as well fires Enter twice.
- `hide` closes the special workspace through Hyprland; the process and window stay.
- Theme tokens: `cx.theme().colors.{background,foreground,surface,border,input,muted,muted_foreground,primary,…}`.
- Set `tracing` filter `omafind_app=info,gpui_shell=info,gpui_kit::shell::script=info` to see script errors.

## Running the daemon in isolation (tests)

```bash
export XDG_RUNTIME_DIR=/tmp/x/run XDG_CONFIG_HOME=/tmp/x/config OMAFIND_INDEX=/tmp/x/index.sqlite
# /tmp/x/config/omafind/config.toml: [index] folders = ["/tmp/x/testbed"]
./target/release/omafindd &            # socket at $XDG_RUNTIME_DIR/omafind.sock
./target/release/omafind status; ./target/release/omafind watch; ./target/release/omafind search "…"
kill $!                                # by PID: `pkill -x omafindd` would stop the real service too
```

Or `just sandbox FOLDER` / `just sandbox-stop`, which do the same under `$TMPDIR/omafind-sandbox`.

## Driving the UI from an agent shell

The tool shell has no Wayland env. Before launching anything graphical:

```bash
export XDG_RUNTIME_DIR=/run/user/1000 WAYLAND_DISPLAY=wayland-1 HYPRLAND_INSTANCE_SIGNATURE=$(ls /run/user/1000/hypr | head -1)
```

Then `./target/debug/omafind-app --dev --ui $PWD/ui &`, `hyprctl clients -j` (class `omafind`), `hyprctl dispatch focuswindow class:omafind`,
`wtype -k Tab; wtype 'query'`, `grim -o eDP-1 shot.png` (3840×2160, scale 1.6), stop it by PID (`pkill -x omafind-app` also closes the real window). Never
`pkill -f`: it matches the tool's own shell.

## Conventions

- Bash in Omarchy style: `#!/bin/bash`, `set -euo pipefail`, `[[ ]]`/`(( ))`, two-space indent. Our commands are
  `omafind`/`omafindd`; only first-party Omarchy commands use the `omarchy-` prefix. Reuse what Omarchy ships (`ffmpeg`,
  `tesseract`, `mpv`, `imv`, `evince`, `omarchy-launch-editor`, `omarchy-hw-*`). No polling: inotify, D-Bus,
  socket pushes.
- Rust edition 2024, workspace deps in the root `Cargo.toml`. Comments say why, not what.
- Commit only when the user asks; never add a Claude co-author trailer or "Generated with" line.
- Keep third-party media out of git (`evals/testbed/` is ignored); commit synthetic inputs only.

# Working on Dig

Dig is local, on-device semantic file search for Omarchy: describe a thing ("dog on the beach", "the clause about
pets in the lease", "where they talk about sleep") and it finds the photo, PDF page, passage, moment in a recording
or video, or code. The phase-by-phase brief is `PLAN.md` (kept out of git by `.gitignore`, as is
`docs/`). Reference checkouts next to this repo: `../omarchy-quattro` (upstream Omarchy, branch `quattro`),
`../gpui-kit` (GPUI Kit at the pinned revision).

## Status

Phase 0 (spikes) is done, 2026-10-14. Both acceptance criteria passed on this machine (Omarchy, Hyprland 0.56,
Radeon RX 480 on RADV, 8-thread x86-64). Notes with numbers: `docs/phase-0.md`. Next: Phase 1 (text + images,
CLI only) per `PLAN.md`.

## Layout

```
crates/dig-core      prompts, model pins, VECTOR_VERSION; later schema + ranking
crates/dig-embed     bindgen over the engine's llama.h/mtmd.h, image sizing, Embedder, `embed-spike` binary
crates/dig-app       GPUI Shell host: window, `dig` host module; later theme bridge + app socket
ui/                  QuickJS UI scripts (gpui-shell.json, main.js); gpui-kit.d.ts is generated, not committed
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

- The engine tag is pinned in `scripts/fetch-engine.sh` (`b11635`) and bumped deliberately: `embed-spike golden
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

## Things that bit us

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

## GPUI Shell notes for the UI (Phase 5)

- Host modules are call-only: `HostModule::new("dig").function(...)/.async_function(...)/.declarations(ts)`,
  `export_module` before `ShellRuntime::new_with_components`. Scripts `import { search } from "dig"`. Values are
  null/bool/number/string/array/object (`HostObject::new().field(..)`). No host → script push: use
  `runtime.refresh(&root, cx)` after host state changes, or a long-poll async function.
- Nothing holds keyboard focus when the window opens; the root View calls `this.focus.focus()` in `init`, and Tab
  (ShellRoot navigation) moves onto the `Input`. There is no script API to focus an `Input` directly at this revision.
- Keystroke spelling is GPUI's: `escape`, `down`, `enter`, `ctrl-n`. Handle keys once (root `on_key_down`); an
  `InputState` `submit` hook as well fires Enter twice.
- `hide` quits for now; a resident hidden window is Phase 5 work (check GPUI Linux `minimize`/unmap at this rev).
- Theme tokens: `cx.theme().colors.{background,foreground,surface,border,input,muted,muted_foreground,primary,…}`.
- Set `tracing` filter `dig_app=info,gpui_shell=info,gpui_kit::shell::script=info` to see script errors.

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

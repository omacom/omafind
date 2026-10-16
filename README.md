# Omafind

Local, on-device semantic file search for [Omarchy](https://omarchy.org). Press `SUPER + CTRL + G`, describe what
you're looking for, and Omafind finds it: *a dog on the beach* finds the photo, *the clause about pets in the lease*
finds the page of the PDF, *where they talk about sleep* finds the moment in the recording. Photos, screenshots (with
the text on them), PDFs (scans too), documents (txt, md, html, docx, odt, epub), audio, video and, opt-in, code.
Nothing leaves the machine.

The window follows your Omarchy theme and font, live. Results open where the match is: the video at the second,
the PDF at the page, the code at the line.

## Install

From this checkout (an AUR package follows):

```bash
./install.sh
```

That builds Omafind, fetches llama.cpp's prebuilt engine (30 MB, no compiling) and opens the window, which walks you
through the rest: it downloads the model files (865 MB), asks which folders to index, and writes the window rules
and autostart line to `~/.config/hypr/omafind.lua`, with the `SUPER + CTRL + G` shortcut if you want it. Opening Omafind from the app launcher does the same on a first
run; `omafind setup` asks the same questions in a terminal. `omafind doctor` checks everything afterwards;
`omafind uninstall` takes it all out again. Ctrl+, in the window opens the settings.

## Use

- `SUPER + CTRL + G` (or `omafind toggle`) shows the search window; `Esc` hides it. `↑ ↓` move, `Enter` opens at the
  match, `Ctrl + Enter` reveals in the file manager, `Ctrl + C` copies the file, `Tab` cycles the kind filter.
- A query starting with `code:` searches the code index (`code: retry with backoff`).
- `omafind search "payment declined error"` does the same from a terminal (`--json`, `--kind pdf,doc`).
- `omafind index ~/Pictures ~/Documents --code ~/dev` picks folders; `omafind status`, `omafind watch`, `omafind pause`, `omafind resume`.
- New and changed files are searchable a few seconds after they land. Indexing pauses in the power-saver profile
  (`~/.config/omafind/config.toml` has the knobs).

## How it works

EmbeddingGemma 2 (text, vision and audio in one 768-dimension space) runs on llama.cpp with Vulkan, CUDA or the CPU.
`omafindd` watches the chosen folders, reads what changed (poppler, tesseract, ffmpeg, all already on Omarchy), embeds it
in a worker that exits when the queue is empty, and answers searches over a socket: nearest vectors plus exact
keywords (FTS5), fused and gated so only matches that stand out are shown. `omafind-app` is a GPUI Shell window whose UI
is a small JavaScript file you can restyle. Developer notes are in [AGENTS.md](AGENTS.md).

## Credits

Omafind is MIT licensed ([LICENSE](LICENSE)). The model is [EmbeddingGemma 2](https://huggingface.co/google/embeddinggemma-2) by Google
DeepMind, under the Apache License 2.0 and the [Gemma Prohibited Use Policy](https://ai.google.dev/gemma/prohibited_use_policy),
in [ggml-org's GGUF conversion](https://huggingface.co/ggml-org/embeddinggemma-2-GGUF); it is downloaded at setup, not
shipped with Omafind. It runs on [llama.cpp](https://github.com/ggml-org/llama.cpp) (MIT). Details in
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

# Dig

Local, on-device semantic file search for [Omarchy](https://omarchy.org). Press `SUPER + CTRL + G`, describe what
you're looking for, and Dig finds it: *a dog on the beach* finds the photo, *the clause about pets in the lease*
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

That builds Dig, fetches llama.cpp's prebuilt engine (30 MB, no compiling) and runs `dig setup`, which downloads
the model files (865 MB), lets you pick the folders to index, starts the indexing daemon at login, and adds the
keybinding, the window rules and the autostart line to your Hyprland config, asking before each step.
`dig doctor` checks everything afterwards; `dig uninstall` takes it all out again.

## Use

- `SUPER + CTRL + G` (or `dig toggle`) shows the search window; `Esc` hides it. `↑ ↓` move, `Enter` opens at the
  match, `Ctrl + Enter` reveals in the file manager, `Ctrl + C` copies the file, `Tab` cycles the kind filter.
- A query starting with `code:` searches the code index (`code: retry with backoff`).
- `dig search "payment declined error"` does the same from a terminal (`--json`, `--kind pdf,doc`).
- `dig index ~/Pictures ~/Documents --code ~/dev` picks folders; `dig status`, `dig watch`, `dig pause`, `dig resume`.
- New and changed files are searchable a few seconds after they land. Indexing pauses in the power-saver profile
  (`~/.config/dig/config.toml` has the knobs).

## How it works

EmbeddingGemma 2 (text, vision and audio in one 768-dimension space) runs on llama.cpp with Vulkan, CUDA or the CPU.
`digd` watches the chosen folders, reads what changed (poppler, tesseract, ffmpeg, all already on Omarchy), embeds it
in a worker that exits when the queue is empty, and answers searches over a socket: nearest vectors plus exact
keywords (FTS5), fused and gated so only matches that stand out are shown. `dig-app` is a GPUI Shell window whose UI
is a small JavaScript file you can restyle. Developer notes are in [AGENTS.md](AGENTS.md).

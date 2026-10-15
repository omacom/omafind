# Dig

Local, on-device semantic file search for [Omarchy](https://omarchy.org). Describe what you're looking for and Dig
finds the photo, PDF page, document passage, moment in a recording or video, or code. Nothing leaves the machine.

**Work in progress.** 

```bash
scripts/fetch-engine.sh      # llama.cpp's own prebuilt release (Vulkan, 30 MB), no compiling
scripts/fetch-models.sh      # EmbeddingGemma 2 GGUFs (865 MB) into ~/.local/share/dig/models
cargo build --release
```

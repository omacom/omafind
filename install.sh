#!/bin/bash
# Installs Dig from this checkout for the current user: binaries into ~/.local/bin, the UI into
# ~/.local/share/dig/ui, the engine via scripts/fetch-engine.sh, then `dig setup` for the model, folders, daemon,
# keybinding and window rules. The AUR package does the same system-wide.
set -euo pipefail
cd "$(dirname "$0")"

bin="${XDG_BIN_HOME:-$HOME/.local/bin}"
share="${XDG_DATA_HOME:-$HOME/.local/share}/dig"

scripts/fetch-engine.sh
cargo build --release
mkdir -p "$bin" "$share/ui"
install -m755 target/release/dig target/release/digd target/release/dig-app "$bin/"
cp ui/gpui-shell.json ui/main.js ui/kinds.js "$share/ui/"
mkdir -p "$HOME/.local/share/applications"
install -m644 packaging/dig.desktop "$HOME/.local/share/applications/dig.desktop"
if [[ ":$PATH:" != *":$bin:"* ]]; then
  echo "note: $bin isn't on your PATH; Hyprland bindings need it (uwsm reads ~/.config/environment.d)"
fi
echo "installed into $bin and $share"
"$bin/dig" setup "$@"

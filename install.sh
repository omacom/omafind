#!/bin/bash
# Installs Omafind from this checkout for the current user: binaries into ~/.local/bin, the UI into
# ~/.local/share/omafind/ui, the engine via scripts/fetch-engine.sh, then the setup for the model, folders, daemon,
# keybinding and window rules: in the window on a first install from a desktop session, else `omafind setup` here.
# The AUR package does the same system-wide.
set -euo pipefail
cd "$(dirname "$0")"

bin="${XDG_BIN_HOME:-$HOME/.local/bin}"
share="${XDG_DATA_HOME:-$HOME/.local/share}/omafind"

scripts/fetch-engine.sh
cargo build --profile dist
mkdir -p "$bin" "$share/ui"
# The dist profile (Cargo.toml): LTO, stripped, third-party code built for size. The release profile keeps debug info
# for profiling, nine tenths of each binary; it isn't what gets installed.
install -m755 target/dist/omafind target/dist/omafindd target/dist/omafind-app "$bin/"
cp ui/gpui-shell.json ui/*.js "$share/ui/"
mkdir -p "$HOME/.local/share/applications"
install -m644 packaging/omafind.desktop "$HOME/.local/share/applications/omafind.desktop"
if [[ ":$PATH:" != *":$bin:"* ]]; then
  echo "note: $bin isn't on your PATH; Hyprland bindings need it (uwsm reads ~/.config/environment.d)"
fi
# A running daemon or window keeps the old binary until restarted.
if systemctl --user is-active --quiet omafindd; then
  systemctl --user restart omafindd
fi
pkill -x omafind-app 2>/dev/null || true
echo "installed into $bin and $share"
config="${XDG_CONFIG_HOME:-$HOME/.config}/omafind/config.toml"
if (( $# == 0 )) && [[ -n ${WAYLAND_DISPLAY:-} && ! -f $config ]]; then
  "$bin/omafind" toggle
  echo "Omafind is open: set it up in its window (or run \`omafind setup\` here instead)"
else
  "$bin/omafind" setup "$@"
fi

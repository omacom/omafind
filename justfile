# Omafind's everyday commands. `just` lists them.
alias c := check
alias t := test
alias v := version
alias b := build
alias d := dev

set shell := ["bash", "-euo", "pipefail", "-c"]

# Throwaway daemons (eval, sandbox) live here, with their own runtime, config and cache dirs, so they never touch
# the real omafindd service or its index. They are always stopped by PID, never by name.
sandbox := env("TMPDIR", "/tmp") / "omafind-sandbox"

default:
    @just --list --unsorted

# --- setup -------------------------------------------------------------------------------------------------------

# Without the fetched engine the build falls back to the system llama.cpp, so fetch before building.
# Fetch the pinned llama.cpp engine: vulkan, cpu, cuda-13.4 or rocm-10.0
engine flavor="vulkan":
    scripts/fetch-engine.sh {{ flavor }}

# Download the pinned EmbeddingGemma 2 model files (resumable, SHA-256 checked, reuses the HF cache)
models:
    scripts/fetch-models.sh

# Engine and models: everything a fresh checkout needs before `just build`
bootstrap: engine models

# Build the test media for `just eval` into evals/testbed (needs espeak-ng and ffmpeg; downloads a few photos)
testbed:
    evals/make-testbed.sh

# --- build and run -----------------------------------------------------------------------------------------------

# Release build of omafind, omafindd and omafind-app
build:
    cargo build --release

# The window with hot reload of ui/, against whichever daemon is running
dev:
    cargo run -p omafind-app -- --dev --ui "$PWD/ui"

# Install for this user (binaries, UI, desktop entry), then `omafind setup`
install *args:
    ./install.sh {{ args }}

# update the version number (x.y.z | patch | minor | major) for app
version VER:
    cargo set-version {{ if VER =~ "^(patch|minor|major)$" { "--bump " + VER } else { VER } }}

# --- quality -----------------------------------------------------------------------------------------------------

fmt:
    cargo fmt --all

# Clippy over everything, warnings as errors
lint:
    cargo clippy --release --workspace --all-targets -- -D warnings

# Tests with cargo-nextest (`just test ci`: no fail-fast, JUnit report), then doctests, which nextest doesn't run
test profile="default":
    cargo nextest run --release --workspace --profile {{ profile }} --show-progress only
    cargo test --release --workspace --doc

# What CI would run: formatting, lints, tests
check:
    cargo fmt --all
    just lint
    just test

# The pinned engine still gives the reference vectors (≥ 0.999 cosine); run after bumping the engine tag
golden:
    cargo run --release -p omafind-embed --bin embed-spike -- golden evals/reference

# Search latency on synthetic indexes of these sizes (built once into ~/.cache/omafind/bench), plus the query encoder
bench sizes="10000 100000 300000":
    cargo run --release -p omafind-cli --bin omafind -- bench {{ sizes }}

# A throwaway daemon indexes a copy of evals/testbed outside git (inside a repo only screenshots are read), then
# stops. Exits non-zero when any query misses its file, like `omafind eval`.
# Recall@1 and @5 over evals/queries.json on the testbed
eval: build
    #!/bin/bash
    set -euo pipefail
    dir="{{ sandbox }}/eval"
    rm -rf "$dir" && mkdir -p "$dir/run" "$dir/config/omafind"
    cp -r evals/testbed "$dir/testbed"
    printf '[index]\nfolders = ["%s"]\n' "$dir/testbed" > "$dir/config/omafind/config.toml"
    export XDG_RUNTIME_DIR="$dir/run" XDG_CONFIG_HOME="$dir/config" XDG_CACHE_HOME="$dir/cache" OMAFIND_INDEX="$dir/index.sqlite"
    target/release/omafindd > "$dir/omafindd.log" 2>&1 &
    pid=$!
    trap 'kill $pid 2>/dev/null' EXIT
    for _ in $(seq 50); do [[ -S $dir/run/omafind.sock ]] && break; sleep 0.1; done
    timeout 600 target/release/omafind watch > /dev/null
    target/release/omafind eval

# --- sandbox: a throwaway daemon and window on any folder ----------------------------------------------------------

# Index FOLDER with a throwaway daemon (`just sandbox ~/Pictures`); `just sandbox-app` opens a window on it
sandbox folder: build
    #!/bin/bash
    set -euo pipefail
    dir="{{ sandbox }}/run-$(basename "{{ folder }}")"
    [[ -f {{ sandbox }}/omafindd.pid ]] && kill "$(cat {{ sandbox }}/omafindd.pid)" 2>/dev/null || true
    mkdir -p "$dir/run" "$dir/config/omafind"
    printf '[index]\nfolders = ["%s"]\n' "$(realpath "{{ folder }}")" > "$dir/config/omafind/config.toml"
    ln -sfn "$dir" "{{ sandbox }}/current"
    export XDG_RUNTIME_DIR="$dir/run" XDG_CONFIG_HOME="$dir/config" XDG_CACHE_HOME="$dir/cache" OMAFIND_INDEX="$dir/index.sqlite"
    target/release/omafindd > "$dir/omafindd.log" 2>&1 &
    echo $! > "{{ sandbox }}/omafindd.pid"
    echo "omafindd $(cat {{ sandbox }}/omafindd.pid) indexing {{ folder }}; log: $dir/omafindd.log"
    echo "CLI against it: OMAFIND_SOCKET=$dir/run/omafind.sock OMAFIND_INDEX=$dir/index.sqlite target/release/omafind status"

# A dev window on the sandbox daemon (hot reload of ui/)
sandbox-app:
    #!/bin/bash
    set -euo pipefail
    dir="$(readlink -f "{{ sandbox }}/current")"
    export OMAFIND_SOCKET="$dir/run/omafind.sock" XDG_CONFIG_HOME="$dir/config" OMAFIND_INDEX="$dir/index.sqlite"
    cargo build -p omafind-app
    target/debug/omafind-app --dev --ui "$PWD/ui" &
    echo $! > "{{ sandbox }}/omafind-app.pid"
    echo "omafind-app $(cat {{ sandbox }}/omafind-app.pid) on $dir"

# Stop the sandbox daemon and window (by PID; the real service is left alone)
# The fake home (evals/make-home.sh) is made once under the sandbox dir; paths and dates then look like real use.
# Daemon on a realistic fake home as $HOME; then `just demo-app`, and `just sandbox-stop` stops both
demo: build
    #!/bin/bash
    set -euo pipefail
    home="{{ sandbox }}/home"
    [[ -d $home/Documents ]] || evals/make-home.sh "$home"
    [[ -f {{ sandbox }}/omafindd.pid ]] && kill "$(cat {{ sandbox }}/omafindd.pid)" 2>/dev/null || true
    mkdir -p "$home/run"
    # The real models, found before HOME changes.
    models="${OMAFIND_MODELS:-$HOME/.local/share/omafind/models}"
    HOME="$home" XDG_CONFIG_HOME="$home/.config" XDG_CACHE_HOME="$home/.cache" XDG_DATA_HOME="$home/.local/share" \
      XDG_RUNTIME_DIR="$home/run" OMAFIND_MODELS="$models" \
      target/release/omafindd > "{{ sandbox }}/demo.log" 2>&1 &
    echo $! > "{{ sandbox }}/omafindd.pid"
    echo "omafindd $(cat {{ sandbox }}/omafindd.pid) on $home; log: {{ sandbox }}/demo.log"

# The window (debug build, hot reload) on the `just demo` home
demo-app:
    #!/bin/bash
    set -euo pipefail
    home="{{ sandbox }}/home"
    cargo build -p omafind-app
    HOME="$home" XDG_CONFIG_HOME="$home/.config" XDG_CACHE_HOME="$home/.cache" XDG_DATA_HOME="$home/.local/share" \
      OMAFIND_SOCKET="$home/run/omafind.sock" target/debug/omafind-app --dev --ui "$PWD/ui" &
    echo $! > "{{ sandbox }}/omafind-app.pid"
    echo "omafind-app $(cat {{ sandbox }}/omafind-app.pid) on $home"

sandbox-stop:
    #!/bin/bash
    for f in "{{ sandbox }}"/omafindd.pid "{{ sandbox }}"/omafind-app.pid; do
      [[ -f $f ]] || continue
      pid=$(cat "$f")
      rm -f "$f"
      kill "$pid" 2>/dev/null || continue
      # The daemon stops its worker first: wait until it is really gone.
      for _ in $(seq 50); do kill -0 "$pid" 2>/dev/null || break; sleep 0.1; done
      echo "stopped $pid"
    done

# --- the installed service ---------------------------------------------------------------------------------------

# Follow the real daemon's log
logs:
    journalctl --user -u omafindd -f

# Restart the real daemon (after `just install`)
restart:
    systemctl --user restart omafindd

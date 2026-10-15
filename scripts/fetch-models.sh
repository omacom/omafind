#!/bin/bash
# Downloads the EmbeddingGemma 2 GGUFs (ggml-org's Q8_0 conversion at one pinned revision) into
# ~/.local/share/omafind/models, resuming partial downloads and checking size and SHA-256 before a file gets its final
# name. Reuses a copy in the Hugging Face cache when one is there at the right size.
set -euo pipefail

REPO=ggml-org/embeddinggemma-2-GGUF
REVISION=bfcd298762cc34d0357ece5ebdd31791a3a374d8
MODELS="${OMAFIND_MODELS:-${XDG_DATA_HOME:-$HOME/.local/share}/omafind/models}"
# name bytes sha256
FILES=(
  "embeddinggemma-2-Q8_0.gguf 309855456 2188ac1deca4b77dffefd603c2776a9d76d9d74ec01841392982ebb840b09135"
  "mmproj-embeddinggemma-2-Q8_0.gguf 554821024 c4a8a52691ecef40618438928bdf9e68379b854e24166f292592353db0aab64f"
)
if [[ ${1:-} == --text-only ]]; then
  FILES=("${FILES[0]}")
fi

hf_hub="${HF_HUB_CACHE:-${HF_HOME:-$HOME/.cache/huggingface}/hub}"
snapshot="$hf_hub/models--${REPO//\//--}/snapshots/$REVISION"

mkdir -p "$MODELS"
for entry in "${FILES[@]}"; do
  read -r name bytes sha <<<"$entry"
  target="$MODELS/$name"
  if [[ -f $target ]]; then
    echo "$name: already here"
    continue
  fi
  part="$target.part"
  if [[ -f $snapshot/$name && $(stat -Lc %s "$snapshot/$name") == "$bytes" ]]; then
    echo "$name: copying from the Hugging Face cache"
    cp -L "$snapshot/$name" "$part"
  else
    echo "$name: downloading ($((bytes / 1000000)) MB)"
    curl -fL --retry 5 --retry-all-errors -C - -o "$part" \
      "https://huggingface.co/$REPO/resolve/$REVISION/$name"
  fi
  if [[ $(stat -c %s "$part") != "$bytes" ]]; then
    echo "$name: wrong size ($(stat -c %s "$part") bytes, expected $bytes)" >&2
    exit 1
  fi
  got=$(sha256sum "$part" | cut -d' ' -f1)
  if [[ $got != "$sha" ]]; then
    echo "$name: SHA-256 mismatch ($got)" >&2
    rm -f "$part"
    exit 1
  fi
  mv "$part" "$target"
  echo "$name: verified"
done
echo "$REPO@$REVISION" > "$MODELS/REVISION"
echo "Models in $MODELS"

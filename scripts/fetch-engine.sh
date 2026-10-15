#!/bin/bash
# Fetches the llama.cpp engine Dig runs on: upstream's own prebuilt release for this machine (no compiler, ~30 MB) plus
# the matching headers, into ~/.local/share/dig/engine/<tag>-<flavor> with a `current` link that dig-embed's build.rs
# and the binaries use. Nothing is built or patched. A second run with the same tag is a no-op.
#   scripts/fetch-engine.sh [vulkan|cpu|cuda-12.8|cuda-13.4|rocm-10.0]    (default: vulkan)
set -euo pipefail

TAG=b11635   # a release with Linux assets for x64 and arm64; ≥ b11584 (no logits buffer for embedding models)
FLAVOR=${1:-${DIG_ENGINE_FLAVOR:-vulkan}}
case $(uname -m) in
  x86_64) ARCH=x64 ;;
  aarch64) ARCH=arm64 ;;
  *) echo "no prebuilt llama.cpp for $(uname -m)" >&2; exit 1 ;;
esac
# Known checksums (others download unverified and say so).
declare -A SHA=(
  [vulkan-x64]=b70f9cc1b40ed1ebf5ba7d73b6b1585fd2c955e85acee954c24a1659f7b439e5
  [vulkan-arm64]=4d4192252556e27a848383a6cf1ae652ca4b8350ab6fffd4c5eff26f857d4db8
)
suffix=$FLAVOR-$ARCH
[[ $FLAVOR == cpu ]] && suffix=$ARCH
asset="llama-$TAG-bin-ubuntu-$suffix.tar.gz"

root="${DIG_ENGINE_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/dig/engine}"
dest="$root/$TAG-$FLAVOR"
if [[ -f $dest/VERSION && -f $dest/lib/libllama.so && -f $dest/include/mtmd.h ]]; then
  ln -sfn "$dest" "$root/current"
  echo "engine $TAG ($FLAVOR, $ARCH) already in $dest"
  exit 0
fi

tmp=$(mktemp -d "${TMPDIR:-/tmp}/dig-engine.XXXXXX")
trap 'rm -rf "$tmp"' EXIT
echo "downloading $asset"
curl -fL --retry 5 --retry-all-errors -o "$tmp/$asset" "https://github.com/ggml-org/llama.cpp/releases/download/$TAG/$asset"
if [[ -n ${SHA[$suffix]:-} ]]; then
  echo "${SHA[$suffix]}  $tmp/$asset" | sha256sum -c --quiet
else
  echo "no checksum known for $asset: unverified"
fi
mkdir -p "$tmp/x" && tar xzf "$tmp/$asset" -C "$tmp/x"
rm -rf "$dest" && mkdir -p "$dest/lib" "$dest/include"
# The libraries Dig loads (llama, mtmd, ggml and its backend modules); the tools and their helper libs stay out.
find "$tmp/x" \( -name 'libllama.so*' -o -name 'libmtmd.so*' -o -name 'libggml*.so*' \) ! -name 'libggml-rpc.so' \
  | xargs -I{} cp -a {} "$dest/lib/"
for f in include/llama.h ggml/include/ggml.h ggml/include/ggml-alloc.h ggml/include/ggml-backend.h \
         ggml/include/ggml-cpu.h ggml/include/ggml-opt.h ggml/include/gguf.h tools/mtmd/mtmd.h tools/mtmd/mtmd-helper.h; do
  curl -fsL --retry 5 -o "$dest/include/$(basename "$f")" "https://raw.githubusercontent.com/ggml-org/llama.cpp/$TAG/$f"
done
echo "$TAG-$FLAVOR" > "$dest/VERSION"
ln -sfn "$dest" "$root/current"
echo "engine $TAG ($FLAVOR, $ARCH) in $dest: $(ls "$dest/lib" | grep -c '\.so$') libraries"

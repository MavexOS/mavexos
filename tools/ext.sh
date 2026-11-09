#!/bin/sh
# Builds third-party OSS programs for wasm32-wasip1 from pinned upstream sources (cloned into .scratch/ext, gitignored).
# Usage: tools/ext.sh   -> .scratch/ext/<name>.wasm
set -e
cd "$(dirname "$0")/.."
export CARGO_PROFILE_RELEASE_STRIP=symbols CARGO_PROFILE_RELEASE_OPT_LEVEL=s CARGO_PROFILE_RELEASE_LTO=true CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 CARGO_PROFILE_RELEASE_DEBUG=false
mkdir -p .scratch/ext
build() { # name repo ref binname [patch]
  name=$1; repo=$2; ref=$3; bin=$4; patch=$5
  dir=.scratch/ext/$name-src
  if [ ! -d "$dir" ]; then
    git clone -q "https://github.com/$repo" "$dir"
    git -C "$dir" checkout -q "$ref"
    [ -n "$patch" ] && git -C "$dir" apply "$PWD/ext/$patch"
  fi
  (cd "$dir" && cargo build --release --target wasm32-wasip1 --bin "$bin" 2>&1 | tail -1)
  cp "$dir/target/wasm32-wasip1/release/$bin.wasm" ".scratch/ext/$name.wasm"
}
build rg BurntSushi/ripgrep 14.1.1 rg ripgrep-wasi.patch
build crabfetch LivacoNew/CrabFetch 187b0b2fc6bc6c8460deb8c7bfb240bd87ae3b60 crab-fetch crabfetch-wasi.patch
# mavex-init lives in its own repo; MAVEX_INIT_DIR overrides with a local checkout
if [ -n "$MAVEX_INIT_DIR" ]; then
  (cd "$MAVEX_INIT_DIR" && cargo build --release --target wasm32-wasip1 2>&1 | tail -1)
  cp "$MAVEX_INIT_DIR/target/wasm32-wasip1/release/mavex-init.wasm" .scratch/ext/init.wasm
else
  build init MavexOS/mavex-init c23710369cc36956ef55cafa6f2629aa85fbf6ae mavex-init
fi

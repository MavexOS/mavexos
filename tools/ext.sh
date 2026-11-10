#!/bin/sh
# Builds third-party OSS programs for wasm32-wasip1 from pinned upstream sources (cloned into .scratch/ext, gitignored).
# Usage: tools/ext.sh   -> .scratch/ext/<name>.wasm
set -e
cd "$(dirname "$0")/.."
export CARGO_PROFILE_RELEASE_STRIP=symbols CARGO_PROFILE_RELEASE_OPT_LEVEL=s CARGO_PROFILE_RELEASE_LTO=true CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 CARGO_PROFILE_RELEASE_DEBUG=false
export CC_wasm32_wasip1=clang CFLAGS_wasm32_wasip1="--target=wasm32-wasip1 --sysroot=/usr"  # needs apt: wasi-libc libclang-rt-dev-wasm32
export WASI_SYSROOT=/usr
mkdir -p .scratch/ext
build() { # name repo ref binname [patch] [srcdir-name] [cargo-toolchain]
  name=$1; repo=$2; ref=$3; bin=$4; patch=$5; src=${6:-$name}; tc=$7
  dir=.scratch/ext/$src-src
  if [ ! -d "$dir" ]; then
    git clone -q "https://github.com/$repo" "$dir"
    git -C "$dir" checkout -q "$ref"
    [ -n "$patch" ] && git -C "$dir" apply "$PWD/ext/$patch"
  fi
  (cd "$dir" && cargo $tc build --release --target wasm32-wasip1 --bin "$bin" 2>&1 | tail -1)
  cp "$dir/target/wasm32-wasip1/release/$bin.wasm" ".scratch/ext/$name.wasm"
}
build rg BurntSushi/ripgrep 14.1.1 rg ripgrep-wasi.patch
build sed uutils/sed 5e2821e69da7eb08304e17807bd89188dc868a22 sed uutils-sed-wasi.patch
build diffutils uutils/diffutils 919194279e5a27236e71dcf67d18c88558176c51 diffutils uutils-diffutils-wasi.patch
CARGO_PROFILE_RELEASE_OPT_LEVEL=3 build awk uutils/awk c7a7cd64d68507d61b125f02713850cf1973a06d awk uutils-awk-wasi.patch awk +nightly
build kibi ilai-deutel/kibi d42f6e34c7d0ea717aacc64cb2f2da4171f7b2b4 kibi kibi-wasi.patch kibi +nightly
build mwget rayylee/mwget 33d8f90e6c489b6c19dc47f5fa0cfcc4040da8df mwget mwget-wasi.patch mwget +nightly
build find uutils/findutils b861b03e27e4d941e6b34249d9496bc2ceaee80a find findutils-wasi.patch findutils
build xargs uutils/findutils b861b03e27e4d941e6b34249d9496bc2ceaee80a xargs "" findutils
# uutils tar uses the `tar` crate, whose wasm32 header code is unimplemented!(); patch a local copy
if [ ! -d .scratch/ext/tar-crate ]; then
  mkdir -p .scratch/ext/tar-crate
  curl -sL https://crates.io/api/v1/crates/tar/0.4.46/download | tar xz --strip-components=1 -C .scratch/ext/tar-crate
  patch -s -p1 -d .scratch/ext/tar-crate < ext/tar-crate-wasi.patch
fi
build tar uutils/tar 02b9c9469069fa0c4f92acccb093a1c880532faf tarapp uutils-tar-wasi.patch tar +nightly
build crabfetch LivacoNew/CrabFetch 187b0b2fc6bc6c8460deb8c7bfb240bd87ae3b60 crab-fetch crabfetch-wasi.patch
# mavex-init lives in its own repo; MAVEX_INIT_DIR overrides with a local checkout
if [ -n "$MAVEX_INIT_DIR" ]; then
  (cd "$MAVEX_INIT_DIR" && cargo build --release --target wasm32-wasip1 2>&1 | tail -1)
  cp "$MAVEX_INIT_DIR/target/wasm32-wasip1/release/mavex-init.wasm" .scratch/ext/init.wasm
else
  build init MavexOS/mavex-init c23710369cc36956ef55cafa6f2629aa85fbf6ae mavex-init
fi
# mavex-kernel lives in its own repo; MAVEX_KERNEL_DIR overrides with a local checkout
if [ -n "$MAVEX_KERNEL_DIR" ]; then
  kdir=$MAVEX_KERNEL_DIR
else
  kdir=.scratch/ext/kernel-src
  if [ ! -d "$kdir" ]; then
    git clone -q https://github.com/MavexOS/mavex-kernel "$kdir"
    git -C "$kdir" checkout -q aa8bce65e4df6af952cbb6aa557c65d9d511b7fb
  fi
fi
(cd "$kdir" && cargo build --release --target wasm32-unknown-unknown -p mavex-kernel 2>&1 | tail -1)
cp "$kdir/target/wasm32-unknown-unknown/release/mavex_kernel.wasm" .scratch/ext/kernel.wasm
cp "$kdir/host/wasi.js" .scratch/ext/wasi.js

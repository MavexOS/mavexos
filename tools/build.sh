#!/bin/sh
set -e
cd "$(dirname "$0")/.."
cargo build --release --target wasm32-unknown-unknown -p mavex-kernel
mkdir -p dist
WASM=target/wasm32-unknown-unknown/release/mavex_kernel.wasm
B64=$(base64 -w0 "$WASM")
sed "s|__KERNEL_WASM_BASE64__|$B64|" web/index.template.html > dist/index.html
echo "built dist/index.html ($(wc -c < dist/index.html) bytes)"

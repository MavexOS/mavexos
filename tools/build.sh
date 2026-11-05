#!/bin/sh
set -e
cd "$(dirname "$0")/.."
cargo build --release --target wasm32-unknown-unknown -p mavex-kernel
mkdir -p dist
WASM=target/wasm32-unknown-unknown/release/mavex_kernel.wasm
python3 - "$WASM" <<'PY'
import base64, sys
b64 = base64.b64encode(open(sys.argv[1], "rb").read()).decode()
tpl = open("web/index.template.html").read()
open("dist/index.html", "w").write(tpl.replace("__KERNEL_WASM_BASE64__", b64))
PY
echo "built dist/index.html ($(wc -c < dist/index.html) bytes)"

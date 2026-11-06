#!/bin/sh
# Builds dist/: index.html, styles.css, scripts.js, wasi.js, kernel.wasm.js, coreutils.wasm.js
set -e
cd "$(dirname "$0")/.."
cargo build --release --target wasm32-unknown-unknown -p mavex-kernel
(cd apps && cargo build --release --target wasm32-wasip1 -p mavex-coreutils -p mavex-brush -p mavex-utils)
mkdir -p dist
sh tools/ext.sh
cp web/index.html web/styles.css web/scripts.js web/wasi.js dist/
python3 - <<'PY'
import base64, re
def blob(path, var, extra=""):
    b64 = base64.b64encode(open(path, "rb").read()).decode()
    return f'window.{var} = "{b64}";\n{extra}'
utils = re.search(r"const UTILS: &\[&str\] = &\[(.*?)\];", open("apps/coreutils/src/main.rs").read()).group(1)
open("dist/kernel.wasm.js", "w").write(blob("target/wasm32-unknown-unknown/release/mavex_kernel.wasm", "MAVEX_KERNEL_B64"))
open("dist/coreutils.wasm.js", "w").write(blob("apps/target/wasm32-wasip1/release/coreutils.wasm", "MAVEX_COREUTILS_B64", f"window.MAVEX_COREUTILS_UTILS = [{utils}];\n"))
open("dist/brush.wasm.js", "w").write(blob("apps/target/wasm32-wasip1/release/brush.wasm", "MAVEX_BRUSH_B64"))
open("dist/ext-rg.wasm.js", "w").write(blob(".scratch/ext/rg.wasm", "MAVEX_EXT_RG_B64"))
open("dist/mavexutils.wasm.js", "w").write(blob("apps/target/wasm32-wasip1/release/mavexutils.wasm", "MAVEX_MAVEXUTILS_B64"))
PY
ls -la dist/*.js dist/index.html dist/styles.css

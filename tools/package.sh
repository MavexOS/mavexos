#!/bin/sh
set -e
cd "$(dirname "$0")/.."
sh tools/build.sh
VER=$(sed -n 's/^version = "\(.*\)"/\1/p' "${MAVEX_KERNEL_DIR:-.scratch/ext/kernel-src}/kernel/Cargo.toml" | head -1)
NAME="mavexos-$VER"
rm -rf "dist/$NAME" "dist/$NAME.zip"
mkdir -p "dist/$NAME"
for f in index.html styles.css scripts.js wasi.js vt.js kernel.wasm.js coreutils.wasm.js brush.wasm.js mavexutils.wasm.js ext-rg.wasm.js ext-sed.wasm.js ext-awk.wasm.js ext-find.wasm.js ext-xargs.wasm.js ext-tar.wasm.js ext-diffutils.wasm.js ext-crabfetch.wasm.js ext-init.wasm.js; do cp "dist/$f" "dist/$NAME/$f"; done
cat > "dist/$NAME/README.txt" <<TXT
MavexOS $VER (pre-release)

1. Extract this zip.
2. Open index.html (keep the other files next to it) in Chrome/Chromium/Edge or Firefox (double-click is fine).
3. Wait for the boot, then type commands at the prompt. Try: help

Everything runs inside the tab, offline. Nothing is saved yet: reloading the page starts a fresh first boot.
TXT
python3 - "dist" "$NAME" <<'PY'
import sys, zipfile, os
d, n = sys.argv[1:3]
with zipfile.ZipFile(f"{d}/{n}.zip", "w", zipfile.ZIP_DEFLATED) as z:
    for f in ("index.html", "styles.css", "scripts.js", "wasi.js", "vt.js", "kernel.wasm.js", "coreutils.wasm.js", "brush.wasm.js", "mavexutils.wasm.js", "ext-rg.wasm.js", "ext-sed.wasm.js", "ext-awk.wasm.js", "ext-find.wasm.js", "ext-xargs.wasm.js", "ext-tar.wasm.js", "ext-diffutils.wasm.js", "ext-crabfetch.wasm.js", "ext-init.wasm.js", "README.txt"):
        z.write(f"{d}/{n}/{f}", f"{n}/{f}")
PY
echo "packaged dist/$NAME.zip"

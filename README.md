# MavexOS

A Linux-like OS that runs in a browser tab from a single local `index.html`, built on WebAssembly (no VM, no emulation of the OS itself).

Status: 0.0.x pre-release. Boots a simulated first-boot tty with the real `brush` shell (bash-compatible) and uutils coreutils. There is a user/root permission model with setuid exec, plus `id`, `chmod`, `chown`, `stat`, a minimal custom `sudo`/`su` (not sudo-rs; no password prompts yet; `/etc/sudoers` is honored; root has an empty password so plain `su` works) and an `ip` reimplementation (link/addr/route on a virtual network namespace; not official iproute2, no real networking yet). The old `msh` shell remains as a fallback via `#msh` in the URL. The filesystem is RAM-only for now. Tested in Chromium and Firefox from `file://`.

## Build

    tools/build.sh

`tools/package.sh` additionally builds `dist/mavexos-<version>.zip` (index.html + README.txt) for releases.

Produces `dist/index.html` (wasm inlined as base64 so it works from `file://`). Requires Rust with the `wasm32-unknown-unknown` target.

## Layout

- `web/`: JS shell (terminal, persistence, boot)
- `apps/`: userland built for wasm32-wasip1 (brush, coreutils, mavexutils)
- Kernel: [MavexOS/mavex-kernel](https://github.com/MavexOS/mavex-kernel) (kernel, vfs, sys, shell crates + `wasi.js` host), pulled by `tools/ext.sh`; init: [MavexOS/mavex-init](https://github.com/MavexOS/mavex-init)
- `tests/`: Playwright browser tests (Chromium + Firefox)
- `tools/`: build scripts

## Test

    (cd apps && cargo test)   # kernel unit tests: cargo test in mavex-kernel
    # local kernel/init checkouts: MAVEX_KERNEL_DIR=... MAVEX_INIT_DIR=...
    tools/build.sh && cd tests && npm install && npx playwright install chromium firefox && npm test

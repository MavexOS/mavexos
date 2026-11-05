# MavexOS

A Linux-like OS that runs in a browser tab from a single local `index.html`, built on WebAssembly (no VM, no emulation of the OS itself).

Status: 0.0.x pre-release. Boots a simulated first-boot tty with the real `brush` shell (bash-compatible) and uutils coreutils. There is a user/root permission model with setuid exec, plus `id`, `chmod`, `chown`, `stat`, a minimal custom `sudo`/`su` (not sudo-rs; no password prompts yet; `/etc/sudoers` is honored; root is locked, use `sudo -i`) and an `ip` reimplementation (link/addr/route on a virtual network namespace; not official iproute2, no real networking yet). The old `msh` shell remains as a fallback via `#msh` in the URL. The filesystem is RAM-only for now. Tested in Chromium and Firefox from `file://`.

## Build

    tools/build.sh

`tools/package.sh` additionally builds `dist/mavexos-<version>.zip` (index.html + README.txt) for releases.

Produces `dist/index.html` (wasm inlined as base64 so it works from `file://`). Requires Rust with the `wasm32-unknown-unknown` target.

## Layout

- `kernel/`: Rust wasm kernel
- `shell/`: `msh` shell (native unit tests)
- `web/`: JS shell template
- `vfs/`: POSIX-like in-memory filesystem (native unit tests)
- `tests/`: Playwright browser tests (Chromium + Firefox)
- `tools/`: build scripts

## Test

    cargo test --workspace
    tools/build.sh && cd tests && npm install && npx playwright install chromium firefox && npm test

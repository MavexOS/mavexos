# MavexOS

A Linux-like OS that runs in a browser tab from a single local `index.html`, built on WebAssembly (no VM, no emulation of the OS itself).

Status: 0.0.x pre-release. Boots a simulated first-boot tty with `msh`, a small shell (pipes, redirects, globs, ~40 builtins, Tab completion). The filesystem is RAM-only for now. Tested in Chromium and Firefox from `file://`.

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

# MavexOS

A Linux-like OS that runs in a browser tab from a single local `index.html`, built on WebAssembly (no VM, no emulation of the OS itself).

Status: early scaffold. A Rust `wasm32` kernel boots with an in-memory VFS (`vfs/`), exposed to JS as syscalls and tested in Chromium and Firefox from `file://`.

## Build

    tools/build.sh

Produces `dist/index.html` (wasm inlined as base64 so it works from `file://`). Requires Rust with the `wasm32-unknown-unknown` target.

## Layout

- `kernel/`: Rust wasm kernel (no_std)
- `web/`: JS shell template
- `vfs/`: POSIX-like in-memory filesystem (native unit tests)
- `tests/`: Playwright browser tests (Chromium + Firefox)
- `tools/`: build scripts

## Test

    cargo test -p mavex-vfs
    tools/build.sh && cd tests && npm install && npx playwright install chromium firefox && npm test

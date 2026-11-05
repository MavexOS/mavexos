# MavexOS

A Linux-like OS that runs in a browser tab from a single local `index.html`, built on WebAssembly (no VM, no emulation of the OS itself).

Status: early scaffold. A Rust `wasm32` kernel stub boots and prints to a JS terminal.

## Build

    tools/build.sh

Produces `dist/index.html` (wasm inlined as base64 so it works from `file://`). Requires Rust with the `wasm32-unknown-unknown` target.

## Layout

- `kernel/`: Rust wasm kernel (no_std)
- `web/`: JS shell template
- `tools/`: build scripts

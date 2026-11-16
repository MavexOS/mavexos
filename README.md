# MavexOS

A Linux-like OS that runs in a browser tab from a single local `index.html`, built on WebAssembly (no VM, no emulation of the OS itself). It works from `file://`, with no server.

Status: 0.0.x pre-release. Tested in Chromium and Firefox.

## What is in it

Real upstream Rust projects, patched to run here (patches in `ext/`):

- `brush` (bash-compatible shell), uutils `coreutils`, `sed`, `diffutils`, `findutils` (`find`, `xargs`), `tar`, `awk`
- `ripgrep` (`rg`), `kibi` (installed as `nano`), `mwget` (installed as `wget`), `crabfetch`
- `boringtun` (WireGuard protocol library, behind `wg` / `wg-quick`)

MavexOS's own code, not ports of the tools they are named after:

- the kernel, VFS and JS host ([mavex-kernel](https://github.com/MavexOS/mavex-kernel)) and PID 1 ([mavex-init](https://github.com/MavexOS/mavex-init), systemd-style units and timers)
- `sudo` / `su` (not sudo-rs), `ip` (not iproute2), `ping` (not ICMP, see below), `top` / `htop`, `ps`, `less`, `stty` and the other small tools in `apps/mavexutils`
- the `wg` / `wg-quick` front ends, and `mavex-backup` / `mavex-restore`

`wget` is real mwget, but its HTTP layer is replaced by a small shim over the browser's `fetch`: downloads are whole-body, single connection, and redirects are followed by the browser.

The old `msh` shell remains as a fallback via `#msh` in the URL.

## Storage

The filesystem is saved to the browser's IndexedDB and restored at boot.

- It is per browser and per origin. Firefox gives every `file://` folder its own origin, so a new version unpacked into a new folder starts empty there. Chromium shares one origin for all `file://` pages.
- To move your files between versions or browsers: `sudo mavex-backup` downloads the whole disk as `mavexos-backup.bin`, and `sudo mavex-restore` loads one back and reboots. Both need root, because a backup contains root-only files.
- With two tabs open on the same profile, the last one to save wins.

## Network

A browser tab cannot open raw sockets. All networking goes through the browser's `fetch`, so it is HTTP(S) only and subject to CORS: a site that does not send CORS headers can be reached but not read. Tools say so plainly ("blocked by CORS") and never invent a response.

`/etc/mavex/network` (root-owned, read on every request) sets the policy:

    mode = direct-only      # direct-only | direct-first | relay-first | relay-only
    timeout = 30
    relay = https://...     # public CORS relays, tried in order
    doh = https://...       # DNS-over-HTTPS resolvers, tried in order
    transport = direct      # only "direct" works; "wireguard" is reserved and not implemented

- **Direct** asks the site itself. It only works for sites that allow cross-origin reads.
- **Relays** are third-party public servers that fetch the page for you. **A relay can read everything you send and receive through it, in plain text.** Do not use relay modes for logins, banking or anything private. They are off by default and often unreliable.
- **Local resolver**: `mavex-resolved` runs as a service on `127.0.0.1:53` and `/etc/resolv.conf` points at it. `dig name [type] [+short]` queries it. Both are MavexOS's own code. Sockets inside MavexOS are loopback-only: programs here can talk to each other, not to other machines.
- **Transport** is the place a tunnel would plug in. WireGuard needs UDP, which a tab cannot send, so `transport = wireguard` only fails with a clear message for now.
- **DoH** answers DNS lookups made inside MavexOS, with padded queries and failover across the listed resolvers. It does not change how the browser itself connects: the browser still uses its own DNS for the actual connection.

`ping` is an HTTPS HEAD request through the browser, not ICMP. `reply` means the site answered and allows reads, `reachable (no CORS)` means it answered but the browser hides the response, and `no reply` means it could not be reached (DNS failure, blocked, offline or a certificate problem; the browser does not say which).

## Keys

While a full-screen program (an editor, `htop`, `less`) has the terminal in raw mode, it owns the keyboard and MavexOS suppresses the browser's shortcuts where the browser allows it. Browsers never let a page override Ctrl+Q, Ctrl+W, Ctrl+T or Ctrl+N in a normal tab, so `/etc/mavex/keys` can give a program a second key:

    kibi: ctrl+] = ctrl+q
    nano: ctrl+] = ctrl+q

So `nano` (kibi) quits with **Ctrl+]**. The file is root-owned and read on every keypress.

## Known limits

- `sudo` asks for no password by default (`/etc/sudoers` has `NOPASSWD`); root has an empty password.
- `find` as a normal user prints a bare `Error: Permission denied (os error 2)` for each directory it cannot read, without naming it, and carries on; use `sudo find` to search everything.
- `awk -F:` is ignored upstream; use `BEGIN{FS=":"}`. `tar` has no `-C` and does not keep symlinks. `find -exec ... +` is missing.
- `ls -l` shows `somebody` / `somegroup` instead of user and group names.
- init: delayed shutdown, `ExecStartPre` and `ExecStop` are not supported.
- No threads, graphics or audio yet.

## Build

    tools/build.sh

`tools/package.sh` additionally builds `dist/mavexos-<version>.zip` (index.html + README.txt) for releases.

Produces `dist/index.html` (wasm inlined as base64 so it works from `file://`). Requires Rust with the `wasm32-unknown-unknown` target.

## Layout

- `web/`: JS shell (terminal, persistence, network policy, boot)
- `apps/`: userland built for wasm32-wasip1 (brush, coreutils, mavexutils)
- `ext/`: patches for the upstream tools, built by `tools/ext.sh`
- Kernel: [MavexOS/mavex-kernel](https://github.com/MavexOS/mavex-kernel) (kernel, vfs, sys, shell crates + `wasi.js` host), pulled by `tools/ext.sh`; init: [MavexOS/mavex-init](https://github.com/MavexOS/mavex-init)
- `tests/`: Playwright browser tests (Chromium + Firefox)
- `tools/`: build scripts

## Test

    (cd apps && cargo test)   # kernel unit tests: cargo test in mavex-kernel
    # local kernel/init checkouts: MAVEX_KERNEL_DIR=... MAVEX_INIT_DIR=...
    tools/build.sh && cd tests && npm install && npx playwright install chromium firefox && npm test

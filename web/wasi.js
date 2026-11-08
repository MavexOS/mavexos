/* MavexOS process runtime: kernel wrapper, WASI bridge, module registry. Classic script (works from file://). */
(function () {
  "use strict";
  const enc = new TextEncoder(), dec = new TextDecoder();
  const ENOSYS = 52, EBADF = 8, EINVAL = 28;
  const RIGHTS_ALL = 0x1fffffffn;
  const R_SEEK = 4n, R_TELL = 32n;

  class ExitSignal { constructor(code) { this.code = code; } }

  const K = { x: null, mem: null, scratch: 0, scratchLen: 0, modules: Object.create(null), consoleBuf: "" };

  const u8 = () => new Uint8Array(K.x.memory.buffer);
  const toBytes = (b64) => {
    const bin = atob(b64), out = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
    return out;
  };
  async function blobBytes(b64) {
    try { return new Uint8Array(await (await fetch("data:application/wasm;base64," + b64)).arrayBuffer()); }
    catch (_) { return toBytes(b64); }
  }

  /* ---- marshalling into kernel memory (single reusable scratch buffer) ---- */
  function scratch(n) {
    if (n > K.scratchLen) {
      if (K.scratch) K.x.k_free(K.scratch, K.scratchLen);
      K.scratchLen = Math.max(n, 65536, K.scratchLen * 2);
      K.scratch = K.x.k_alloc(K.scratchLen);
    }
    return K.scratch;
  }
  // Strings are packed back-to-back in scratch; returns [[ptr,len],...]
  function packed(...strs) {
    const bs = strs.map((s) => (typeof s === "string" ? enc.encode(s) : s));
    const p = scratch(bs.reduce((a, b) => a + b.length, 0));
    let off = 0; const out = [];
    for (const b of bs) { u8().set(b, p + off); out.push([p + off, b.length]); off += b.length; }
    return out;
  }
  const staged = (n) => u8().slice(K.x.k_result_ptr(), K.x.k_result_ptr() + n);
  const stagedStr = (n) => dec.decode(u8().subarray(K.x.k_result_ptr(), K.x.k_result_ptr() + n));
  const errOf = (r) => (r < 0 ? -r : 0);

  /* ---- Asyncify scheduler: tasks suspend inside blocking imports and are resumed by pump() ---- */
  const AGAIN = 6, EPIPE = 64, AS_UNWIND = 1, AS_REWIND = 2, ASYNC_BUF = 1 << 20;
  const S = { tasks: new Map(), cur: null, pumping: false, timer: 0, onIdle: null, ttys: [], cols: 80, rows: 24, busy: 0, gen: 0 };
  const newTtyState = () => ({ icanon: true, echo: true, isig: true, raw: false, onOutput: null });
  S.ttys[0] = newTtyState();
  const ttyOf = (t) => (t.tty === undefined ? (t.tty = K.x.k_proc_tty(t.pid)) : t.tty);
  const canBlock = (t) => !!t && !t.sync && t.asyncify;
  // Called first in every blockable import: finishes a pending rewind so the import can retry for real.
  function rewound(t) {
    if (t && t.asyncify && t.inst.exports.asyncify_get_state() === AS_REWIND) { t.inst.exports.asyncify_stop_rewind(); return true; }
    return false;
  }
  function block(t, cond, wakeAt) {
    const ex = t.inst.exports;
    if (!t.data) t.data = ex.memory.grow(ASYNC_BUF / 65536) * 65536;
    const dv = new DataView(ex.memory.buffer);
    dv.setUint32(t.data, t.data + 8, true); dv.setUint32(t.data + 4, t.data + ASYNC_BUF, true);
    t.waitCond = cond; t.wakeAt = wakeAt || 0;
    ex.asyncify_start_unwind(t.data);
    return 0;
  }
  function makeTask(pid, sync) { return { pid, sync, inst: null, asyncify: false, data: 0, blocked: false, rewinding: false, waitCond: null, wakeAt: 0, entry: null, done: true, onFinish: null, shell: false, killed: false, daemon: false, killCode: 130, cpu: 0, stopped: false, sigPending: 0, sigIgnore: 0 }; }
  function syncStats() {
    for (const t of S.tasks.values()) {
      let rss = 0; try { rss = Math.round(t.inst.exports.memory.buffer.byteLength / 1024); } catch (e) {}
      K.x.k_set_proc_stat(t.pid, t.cpu, rss, t.stopped ? 84 : t.blocked ? 83 : 82);
    }
    K.x.k_set_busy(S.busy);
  }
  function attach(t, inst) { t.inst = inst; t.asyncify = typeof inst.exports.asyncify_get_state === "function"; }
  function launch(t, entry, onFinish) { t.entry = entry; t.onFinish = onFinish; t.done = false; t.blocked = false; t.rewinding = false; S.tasks.set(t.pid, t); }
  function stepTask(t) {
    syncStats();
    const prev = S.cur; S.cur = t;
    const t0 = performance.now();
    let code = 0, finished = true;
    try {
      if (t.rewinding) { t.rewinding = false; t.inst.exports.asyncify_start_rewind(t.data); }
      t.entry();
    } catch (e) {
      if (e instanceof ExitSignal) code = e.code;
      else if (e instanceof ParkSignal) code = 0;
      else {
        code = 134;
        const [[p, n]] = packed("mavexos: program trapped: " + (e && e.message || e) + "\n"); K.x.k_fd_write(t.pid, 2, p, n);
      }
    } finally { S.cur = prev; const dt = performance.now() - t0; t.cpu += dt; S.busy += dt; }
    if (t.asyncify && t.inst.exports.asyncify_get_state() === AS_UNWIND) { t.inst.exports.asyncify_stop_unwind(); t.blocked = true; t.rewinding = true; finished = false; }
    if (finished) { t.done = true; t.blocked = false; S.tasks.delete(t.pid); S.gen++; if (t.onFinish) t.onFinish(code); }
  }
  function flushOutput() { S.ttys.forEach((ts, i) => { const n = K.x.k_tty_take(i); if (n) { const s = stagedStr(n); if (ts.onOutput) ts.onOutput(s); } }); }
  function pump() {
    if (S.pumping) return;
    S.pumping = true;
    const budget = performance.now() + 25;
    let more = false;
    try {
      let progressed = true;
      while (progressed) {
        progressed = false;
        for (const t of [...S.tasks.values()]) {
          if (t.done) continue;
          if (t.killed) { t.done = true; S.tasks.delete(t.pid); S.gen++; K.x.k_proc_exit(t.pid, t.killCode); if (t.onFinish) t.onFinish(t.killCode); progressed = true; continue; }
          if (t.stopped) continue;
          if (t.blocked && !t.waitCond()) continue;
          t.blocked = false; progressed = true; stepTask(t);
        }
        if (progressed && performance.now() > budget) { more = true; break; }
      }
    } finally { S.pumping = false; }
    flushOutput();
    clearTimeout(S.timer);
    if (more) { S.timer = setTimeout(pump, 0); return; }
    let next = Infinity;
    for (const t of S.tasks.values()) if (t.blocked && !t.stopped && t.wakeAt) next = Math.min(next, t.wakeAt);
    if (next < Infinity) S.timer = setTimeout(pump, Math.max(0, next - performance.now()));
    if (S.onIdle) S.onIdle();
  }
  /* ---- signals: Linux numbering; default actions are terminate / stop / continue / ignore ---- */
  const SIG = { HUP: 1, INT: 2, QUIT: 3, ABRT: 6, KILL: 9, USR1: 10, SEGV: 11, USR2: 12, PIPE: 13, ALRM: 14, TERM: 15, CHLD: 17, CONT: 18, STOP: 19, TSTP: 20, TTIN: 21, TTOU: 22, URG: 23, WINCH: 28 };
  const IGNORED_BY_DEFAULT = new Set([SIG.CHLD, SIG.URG, SIG.WINCH]);
  const STOPPING = new Set([SIG.STOP, SIG.TSTP, SIG.TTIN, SIG.TTOU]);
  function deliver(t, sig) {
    if (!sig || t.done) return;
    const bit = 1 << (sig - 1);
    if (sig === SIG.KILL) { t.killed = true; t.killCode = 137; t.stopped = false; S.gen++; return; }
    if (sig === SIG.CONT) { const was = t.stopped; t.stopped = false; t.sigPending |= bit; if (was) S.gen++; return; }
    if (sig === SIG.STOP) { t.stopped = true; S.gen++; return; }
    if (t.sigIgnore & bit) return;
    if (t.shell) { t.sigPending |= bit; if (STOPPING.has(sig) || IGNORED_BY_DEFAULT.has(sig) || sig === SIG.INT || sig === SIG.TERM || sig === SIG.HUP || sig === SIG.QUIT) return; t.killed = true; t.killCode = 128 + sig; S.gen++; return; }
    if (STOPPING.has(sig)) { t.stopped = true; S.gen++; return; }
    if (IGNORED_BY_DEFAULT.has(sig)) { t.sigPending |= bit; return; }
    t.killed = true; t.killCode = 128 + sig; S.gen++;
  }
  function signalTargets(target) { // >0 pid, <0 process group
    if (target > 0) { const t = S.tasks.get(target); return t ? [t] : []; }
    const out = [], n = K.x.k_pgrp_members(-target), b = n ? staged(n) : new Uint8Array(0), dv2 = new DataView(b.buffer);
    for (let i = 0; i < b.length; i += 4) { const t = S.tasks.get(dv2.getUint32(i, true)); if (t) out.push(t); }
    return out;
  }
  function sendSignal(target, sig) { for (const t of signalTargets(target)) deliver(t, sig); pump(); }
  function ttySignal(tt, sig) {
    const fg = K.x.k_tty_get_fg(tt);
    if (fg > 0 && signalTargets(-fg).some((t) => !t.shell)) { sendSignal(-fg, sig); return true; }
    if (sig === SIG.INT) { for (const t of S.tasks.values()) if (!t.shell && !t.daemon && ttyOf(t) === tt) deliver(t, SIG.INT); pump(); return true; }
    return false;
  }
  function ttySet(tt, m) { const t = S.ttys[tt]; t.icanon = !!(m & 1); t.echo = !!(m & 2); t.isig = !!(m & 4); t.raw = !t.icanon; }
  const ttyReset = (tt) => { const t = S.ttys[tt]; ttySet(tt, 1 | (t.echo ? 2 : 0) | (t.isig ? 4 : 0)); };
  const winch = () => S.ttys.forEach((_, i) => ttySignal(i, SIG.WINCH));
  const killForeground = (tt) => ttySignal(tt, SIG.INT);
  const stopForeground = (tt) => ttySignal(tt, SIG.TSTP);
  const foregroundBusy = (tt) => [...S.tasks.values()].some((t) => !t.shell && !t.daemon && !t.done && ttyOf(t) === tt);
  function newTty() { const id = K.x.k_tty_new(); S.ttys[id] = newTtyState(); return id; }

  /* ---- WASI for one process ---- */
  function makeWasi(pid, getMem, task) {
    const dv = () => new DataView(getMem().buffer);
    const m8 = () => new Uint8Array(getMem().buffer);
    const x = K.x;
    const readStr = (p, n) => dec.decode(m8().subarray(p, p + n));
    const iovTotal = (iovs, cnt) => { const d = dv(); let t = 0; for (let i = 0; i < cnt; i++) t += d.getUint32(iovs + i * 8 + 4, true); return t; };
    const scatter = (iovs, cnt, src) => {
      const d = dv(), m = m8(); let off = 0;
      for (let i = 0; i < cnt && off < src.length; i++) {
        const p = d.getUint32(iovs + i * 8, true), l = d.getUint32(iovs + i * 8 + 4, true);
        const n = Math.min(l, src.length - off); m.set(src.subarray(off, off + n), p); off += n;
      }
      return off;
    };
    const gather = (iovs, cnt) => {
      const total = iovTotal(iovs, cnt), p = scratch(total), d = dv(), m = m8(), kd = u8(); let off = 0;
      for (let i = 0; i < cnt; i++) {
        const ip = d.getUint32(iovs + i * 8, true), l = d.getUint32(iovs + i * 8 + 4, true);
        kd.set(m.subarray(ip, ip + l), p + off); off += l;
      }
      return [p, total];
    };
    const setU32 = (p, v) => dv().setUint32(p, v, true);
    const copyOut = (n, ptr, cap) => { // staged bytes -> app buffer, truncated to cap
      const b = staged(n), k = Math.min(b.length, cap); m8().set(b.subarray(0, k), ptr); return k;
    };
    let argv = null, envp = null;
    const lists = () => {
      if (!argv) {
        const s = (f) => { const n = f(pid); const t = stagedStr(n); return t === "" ? [] : t.split("\0").filter((v, i, a) => i < a.length - 1 || v !== ""); };
        argv = s(x.k_proc_args); envp = s(x.k_proc_env);
      }
    };
    const writeList = (list, ptrs, buf) => {
      const d = dv(), m = m8(); let off = buf;
      list.forEach((s, i) => { d.setUint32(ptrs + i * 4, off, true); const b = enc.encode(s); m.set(b, off); m[off + b.length] = 0; off += b.length + 1; });
    };
    const sizes = (list) => list.reduce((a, s) => a + enc.encode(s).length + 1, 0);
    const now = () => performance.timeOrigin + performance.now();

    const pathCall = (fn, dirfd, p, n, ...rest) => { const [[kp, kn]] = packed(readStr(p, n)); return fn(pid, dirfd, kp, kn, ...rest); };

    return {
      args_sizes_get(c, s) { lists(); setU32(c, argv.length); setU32(s, sizes(argv)); return 0; },
      args_get(p, b) { lists(); writeList(argv, p, b); return 0; },
      environ_sizes_get(c, s) { lists(); setU32(c, envp.length); setU32(s, sizes(envp)); return 0; },
      environ_get(p, b) { lists(); writeList(envp, p, b); return 0; },
      clock_res_get(id, out) { dv().setBigUint64(out, 1000n, true); return 0; },
      clock_time_get(id, prec, out) {
        const ns = id === 0 ? BigInt(Math.round(now() * 1e6)) : BigInt(Math.round(performance.now() * 1e6));
        dv().setBigUint64(out, ns, true); return 0;
      },
      random_get(p, n) { const m = m8(); for (let o = 0; o < n; o += 65536) crypto.getRandomValues(m.subarray(p + o, p + Math.min(n, o + 65536))); return 0; },
      sched_yield() {
        if (rewound(task) || !canBlock(task) || S.tasks.size < 2) return 0;
        return block(task, () => true);
      },
      proc_exit(code) { throw new ExitSignal(code); },
      proc_raise() { throw new ExitSignal(134); },
      poll_oneoff(inp, out, nsubs, nev) {
        const rw = rewound(task);
        const d = dv(); let end = 0; const fds = [];
        for (let i = 0; i < nsubs; i++) {
          const s = inp + i * 48, tag = d.getUint8(s + 8);
          if (tag === 0) {
            const t = Number(d.getBigUint64(s + 24, true)) / 1e6, abs = d.getUint16(s + 40, true) & 1;
            const e = (abs ? t : performance.timeOrigin + performance.now() + t) - performance.timeOrigin;
            end = end ? Math.min(end, e) : e;
          } else if (tag === 1 || tag === 2) fds.push({ fd: d.getUint32(s + 16, true), w: tag === 2 ? 1 : 0 });
        }
        if (rw && task.pollEnd) end = task.pollEnd;
        const anyReady = () => fds.some((f) => x.k_fd_ready(pid, f.fd, f.w) !== 0);
        if (!anyReady()) {
          if (end) {
            if (performance.now() < end) {
              if (canBlock(task)) { task.pollEnd = end; return block(task, () => anyReady() || performance.now() >= end, end); }
              while (performance.now() < end && !anyReady()) { /* sync context: busy wait */ }
            }
          } else if (fds.length && canBlock(task)) return block(task, anyReady);
        }
        task.pollEnd = 0;
        const expired = end && performance.now() >= end;
        let n = 0;
        for (let i = 0; i < nsubs; i++) {
          const s = inp + i * 48, e = out + n * 32, tag = d.getUint8(s + 8);
          const fire = tag === 0 ? expired : (tag === 1 || tag === 2) ? x.k_fd_ready(pid, d.getUint32(s + 16, true), tag === 2 ? 1 : 0) !== 0 : true;
          if (!fire) continue;
          dv().setBigUint64(e, d.getBigUint64(s, true), true); d.setUint16(e + 8, 0, true); d.setUint8(e + 10, tag);
          d.setBigUint64(e + 16, 0n, true); d.setUint16(e + 24, 0, true); n++;
        }
        if (n === 0) for (let i = 0; i < nsubs; i++) {
          const s = inp + i * 48, e = out + n * 32;
          dv().setBigUint64(e, d.getBigUint64(s, true), true); d.setUint16(e + 8, 0, true); d.setUint8(e + 10, d.getUint8(s + 8));
          d.setBigUint64(e + 16, 0n, true); d.setUint16(e + 24, 0, true); n++;
        }
        setU32(nev, n); return 0;
      },
      fd_close(fd) { return errOf(x.k_fd_close(pid, fd)); },
      fd_read(fd, iovs, cnt, nout) {
        rewound(task);
        const r = x.k_fd_read(pid, fd, iovTotal(iovs, cnt));
        if (r === -AGAIN) {
          if (canBlock(task)) return block(task, () => x.k_fd_ready(pid, fd, 0) !== 0);
          setU32(nout, 0); return 0;
        }
        if (r < 0) return -r;
        setU32(nout, scatter(iovs, cnt, staged(r))); return 0;
      },
      fd_pread(fd, iovs, cnt, off, nout) {
        const r = x.k_fd_pread(pid, fd, iovTotal(iovs, cnt), Number(off));
        if (r < 0) return -r;
        setU32(nout, scatter(iovs, cnt, staged(r))); return 0;
      },
      fd_write(fd, iovs, cnt, nout) {
        rewound(task);
        const [p, n] = gather(iovs, cnt), r = x.k_fd_write(pid, fd, p, n);
        if (r === -AGAIN) return canBlock(task) ? block(task, () => x.k_fd_ready(pid, fd, 1) !== 0) : EPIPE;
        if (r < 0) return -r;
        setU32(nout, r); return 0;
      },
      fd_pwrite(fd, iovs, cnt, off, nout) {
        const [p, n] = gather(iovs, cnt), r = x.k_fd_pwrite(pid, fd, p, n, Number(off));
        if (r < 0) return -r;
        setU32(nout, r); return 0;
      },
      fd_seek(fd, delta, whence, out) {
        const r = x.k_fd_seek(pid, fd, Number(delta), whence);
        if (r < 0) return -r;
        dv().setBigUint64(out, BigInt(r), true); return 0;
      },
      fd_tell(fd, out) { const r = x.k_fd_seek(pid, fd, 0, 1); if (r < 0) return -r; dv().setBigUint64(out, BigInt(r), true); return 0; },
      fd_sync() { return 0; }, fd_datasync() { return 0; }, fd_advise() { return 0; }, fd_allocate() { return 0; },
      fd_fdstat_get(fd, out) {
        const r = x.k_fd_fdstat(pid, fd);
        if (r < 0) return -r;
        const d = dv(), type = r & 255, append = (r >> 8) & 1;
        d.setUint8(out, type); d.setUint8(out + 1, 0); d.setUint16(out + 2, append, true);
        let rights = RIGHTS_ALL;
        if (type === 2 || type === 0) rights &= ~(R_SEEK | R_TELL); // tty and pipes are not seekable (isatty relies on this for CHAR)
        d.setBigUint64(out + 8, rights, true); d.setBigUint64(out + 16, RIGHTS_ALL, true); return 0;
      },
      fd_fdstat_set_flags() { return 0; }, fd_fdstat_set_rights() { return 0; },
      fd_filestat_get(fd, out) {
        const r = x.k_fd_filestat(pid, fd);
        if (r < 0) return -r;
        m8().set(staged(r), out); return 0;
      },
      fd_filestat_set_size(fd, size) { return errOf(x.k_fd_set_size(pid, fd, Number(size))); },
      fd_filestat_set_times() { return 0; },
      fd_prestat_get(fd, out) {
        if (fd !== 3) return EBADF; // only "/" is advertised; libc cwd comes from PWD (see mavex-rt)
        const r = x.k_fd_prestat(pid, fd);
        if (r < 0) return -r;
        const d = dv(); d.setUint8(out, 0); d.setUint32(out + 4, r, true); return 0;
      },
      fd_prestat_dir_name(fd, p, n) {
        const r = x.k_fd_prestat(pid, fd);
        if (r < 0) return -r;
        copyOut(r, p, n); return 0;
      },
      fd_readdir(fd, buf, bufLen, cookie, nout) {
        const r = x.k_fd_readdir(pid, fd, Number(cookie), bufLen);
        if (r < 0) return -r;
        setU32(nout, copyOut(r, buf, bufLen)); return 0;
      },
      fd_renumber() { return ENOSYS; },
      path_open(dirfd, _lookup, p, n, oflags, rights, _inh, fdflags, out) {
        const dir = (oflags & 2) !== 0;
        const read = dir || (rights & 0x4002n) !== 0n || (rights & 64n) === 0n;
        const write = (rights & 64n) !== 0n;
        const r = pathCall(x.k_path_open.bind(x), dirfd, p, n, oflags, fdflags, read ? 1 : 0, write ? 1 : 0);
        if (r < 0) return -r;
        setU32(out, r); return 0;
      },
      path_create_directory(dirfd, p, n) { return errOf(pathCall(x.k_path_mkdir, dirfd, p, n)); },
      path_remove_directory(dirfd, p, n) { return errOf(pathCall(x.k_path_rmdir, dirfd, p, n)); },
      path_unlink_file(dirfd, p, n) { return errOf(pathCall(x.k_path_unlink, dirfd, p, n)); },
      path_rename(d1, p1, n1, d2, p2, n2) {
        const [[a, an], [b, bn]] = packed(readStr(p1, n1), readStr(p2, n2));
        return errOf(x.k_path_rename(pid, d1, a, an, d2, b, bn));
      },
      path_symlink(tp, tn, dirfd, p, n) {
        const [[a, an], [b, bn]] = packed(readStr(tp, tn), readStr(p, n));
        return errOf(x.k_path_symlink(pid, a, an, dirfd, b, bn));
      },
      path_readlink(dirfd, p, n, buf, cap, nout) {
        const r = pathCall(x.k_path_readlink, dirfd, p, n);
        if (r < 0) return -r;
        setU32(nout, copyOut(r, buf, cap)); return 0;
      },
      path_filestat_get(dirfd, flags, p, n, out) {
        const r = pathCall(x.k_path_filestat, dirfd, p, n, flags & 1);
        if (r < 0) return -r;
        m8().set(staged(r), out); return 0;
      },
      path_filestat_set_times() { return 0; },
      path_link() { return ENOSYS; },
      sock_accept() { return ENOSYS; }, sock_recv() { return ENOSYS; }, sock_send() { return ENOSYS; }, sock_shutdown() { return ENOSYS; },
    };
  }

  /* ---- process runner (synchronous, run to completion) ---- */
  function resolveModule(pid) {
    const n = K.x.k_proc_image(pid);
    if (n < 0) return null;
    const img = staged(n);
    if (img.length > 4 && img[0] === 0 && img[1] === 0x61 && img[2] === 0x73 && img[3] === 0x6d) return new WebAssembly.Module(img);
    const head = dec.decode(img.subarray(0, 64));
    const m = head.match(/^#!mavex-module (\S+)\n/);
    return m ? K.modules[m[1]] || null : null;
  }

  class ParkSignal {}
  function makeMavex(pid, getMem, task) {
    const m8 = () => new Uint8Array(getMem().buffer), dv = () => new DataView(getMem().buffer);
    const str = (p, n) => dec.decode(m8().subarray(p, p + n));
    return {
      pipe(out) { const r = K.x.k_pipe(pid); if (r < 0) return r; dv().setUint32(out, r & 0xffff, true); dv().setUint32(out + 4, r >>> 16, true); return 0; },
      spawn(pp, pn, ap, an, ep, en, cp, cn, s0, s1, s2) {
        const [[a, al], [b, bl], [c, cl], [d, dl]] = packed(str(pp, pn), m8().slice(ap, ap + an), m8().slice(ep, ep + en), str(cp, cn));
        const r = K.x.k_spawn(pid, a, al, b, bl, c, cl, d, dl, s0, s1, s2);
        if (r > 0 && !task.sync) startProcess(r, task.daemon);
        return r;
      },
      ready: () => 1,
      run: (c) => { if (task.sync) runProcess(c); return 0; },
      wait(c) {
        rewound(task);
        if (!K.x.k_proc_exited(c) && canBlock(task)) return block(task, () => K.x.k_proc_exited(c) !== 0);
        return K.x.k_wait(c);
      },
      poll_child: (c) => (K.x.k_proc_exited(c) ? K.x.k_wait(c) : -1),
      kill(c, sig) {
        const target = c | 0, victims = [];
        if (target > 0) { if (K.x.k_proc_creds(target) < 0) return -71; victims.push(target); }
        else { const n = K.x.k_pgrp_members(-target), b = n ? staged(n) : new Uint8Array(0), d = new DataView(b.buffer); for (let i = 0; i < b.length; i += 4) victims.push(d.getUint32(i, true)); if (!victims.length) return -71; }
        const creds = (p) => (K.x.k_proc_creds(p) < 0 ? null : new Uint32Array(staged(K.x.k_proc_creds(p)).buffer));
        const mine = creds(pid);
        for (const v of victims) { const cr = creds(v); if (mine && cr && mine[2] !== 0 && mine[0] !== cr[0]) return -63; }
        if (sig) sendSignal(target, sig);
        return 0;
      },
      sig_take() { const v = task.sigPending; task.sigPending = 0; return v; },
      sig_ignore(mask) { task.sigIgnore = mask >>> 0; return 0; },
      child_state(c) {
        const t = S.tasks.get(c);
        if (t && !t.done) return t.stopped ? 1 : 0;
        return K.x.k_proc_exited(c) ? 2 : K.x.k_proc_creds(c) < 0 ? -1 : 0;
      },
      wait_change(ms) {
        if (rewound(task) || !canBlock(task)) return 0;
        const g = S.gen, end = performance.now() + (ms || 20);
        return block(task, () => S.gen !== g || performance.now() >= end, end);
      },
      tcsetpgrp(g) { K.x.k_tty_set_fg(ttyOf(task), g); return 0; },
      tcgetpgrp: () => K.x.k_tty_get_fg(ttyOf(task)),
      setpgid: (p, g) => K.x.k_setpgid(p || pid, g),
      getpgid: (p) => K.x.k_getpgid(p || pid),
      setsid: () => K.x.k_setsid(pid),
      tty_name() { return ttyOf(task) + 1; },
      tty_size(out) { const d = dv(); d.setUint32(out, S.cols, true); d.setUint32(out + 4, S.rows, true); return 0; },
      tty_raw(on) { ttySet(ttyOf(task), on ? 4 : 7); return 0; },
      tty_getattr() { const t = S.ttys[ttyOf(task)]; return (t.icanon ? 1 : 0) | (t.echo ? 2 : 0) | (t.isig ? 4 : 0); },
      tty_setattr(m) { ttySet(ttyOf(task), m); return 0; },
      dup: (fd) => K.x.k_fd_dup(pid, fd),
      park() { throw new ParkSignal(); },
      getcreds(out) { const r = K.x.k_proc_creds(pid); if (r < 0) return r; m8().set(staged(r), out); return 0; },
      setcreds: (u, g) => K.x.k_setcreds(pid, u, g),
      chmod(pp, pn, mode) { const [[a, al]] = packed(str(pp, pn)); return K.x.k_chmod_as(pid, a, al, mode); },
      chown(pp, pn, u, g) { const [[a, al]] = packed(str(pp, pn)); return K.x.k_chown_as(pid, a, al, u, g); },
      sysinfo(out) {
        const mem = Math.round(((performance.memory && performance.memory.usedJSHeapSize) || K.x.memory.buffer.byteLength) / 1048576);
        const total = Math.max(mem, Math.round((navigator.deviceMemory || 4) * 1024));
        K.x.k_set_hostinfo(total * 1024, mem * 1024, navigator.hardwareConcurrency || 1);
        const v = new Uint32Array(5);
        v.set([Math.floor(performance.now() / 1000), total, mem, navigator.hardwareConcurrency || 1, 0]);
        m8().set(new Uint8Array(v.buffer), out); return 0;
      },
      statx(pp, pn, out) { const [[a, al]] = packed(str(pp, pn)); const r = K.x.k_statx(pid, a, al); if (r < 0) return r; m8().set(staged(r), out); return 0; },
      net(rp, rn, out, cap) {
        const [[a, al]] = packed(str(rp, rn));
        const r = K.x.k_net(pid, a, al), data = staged(K.x.k_result_len());
        const n = Math.min(data.length, cap - 1);
        m8().set(data.subarray(0, n), out); m8()[out + n] = 0;
        return r < 0 ? r : n;
      },
      push_shell(cp, cn, ep, en) {
        const cwd = str(cp, cn), env = dec.decode(m8().subarray(ep, ep + en)).split("\0").filter(Boolean);
        startBrush(cwd, env, pid, ttyOf(task));
        return 0;
      },
    };
  }

  function instantiate(pid, mod, task) {
    let memory = null;
    const inst = new WebAssembly.Instance(mod, { wasi_snapshot_preview1: makeWasi(pid, () => memory, task), mavex: makeMavex(pid, () => memory, task) });
    memory = inst.exports.memory;
    attach(task, inst);
    return inst;
  }
  const errTo = (pid, s) => { const [[p, n]] = packed(s); K.x.k_fd_write(pid, 2, p, n); };

  // Synchronous run-to-completion (kernel-driven shell); blocking imports degrade to the old non-blocking behaviour.
  function runProcess(pid) {
    const x = K.x;
    let code = 0;
    try {
      const mod = resolveModule(pid);
      if (!mod) { errTo(pid, "mavexos: cannot execute: not a program\n"); x.k_proc_exit(pid, 126); return; }
      const task = makeTask(pid, true);
      instantiate(pid, mod, task).exports._start();
    } catch (e) {
      if (e instanceof ExitSignal) code = e.code;
      else { errTo(pid, "mavexos: program trapped: " + (e && e.message || e) + "\n"); code = 134; }
    }
    x.k_proc_exit(pid, code);
  }

  // Asynchronous process: scheduled by pump(), may suspend in blocking imports.
  function startProcess(pid, daemon) {
    const mod = resolveModule(pid);
    if (!mod) { errTo(pid, "mavexos: cannot execute: not a program\n"); K.x.k_proc_exit(pid, 126); return; }
    const task = makeTask(pid, false); task.daemon = !!daemon;
    let inst;
    try { inst = instantiate(pid, mod, task); } catch (e) { errTo(pid, "mavexos: cannot load program: " + (e && e.message || e) + "\n"); K.x.k_proc_exit(pid, 126); return; }
    launch(task, () => inst.exports._start(), (code) => K.x.k_proc_exit(pid, code));
  }

  function startInit() {
    const [[a, al], [b, bl], [c, cl], [d, dl]] = packed("/bin/init", "init\0", "PATH=/bin\0HOME=/root\0LANG=en_US.UTF-8\0", "/");
    const pid = K.x.k_spawn(0, a, al, b, bl, c, cl, d, dl, -2, -2, -2);
    if (pid < 0) throw new Error("init spawn failed " + pid);
    K.x.k_force_creds(pid, 0, 0);
    startProcess(pid, true); pump();
    return pid;
  }

  /* ---- persistent brush login shell ---- */
  const Bs = [];
  function startBrush(cwd, env, ppid, tt) {
    tt = tt || 0; const B = Bs[tt] || (Bs[tt] = { pid: 0, task: null, stack: [] });
    const x = K.x, mod = K.modules.brush;
    const [[a, al], [b, bl], [c, cl], [d, dl]] = packed("/bin/brush", "brush\0", env.join("\0") + "\0", cwd);
    const pid = x.k_spawn(ppid || 0, a, al, b, bl, c, cl, d, dl, -1, -1, -1);
    if (pid < 0) throw new Error("brush spawn failed " + pid);
    if (ppid === 1) x.k_force_creds(pid, 1000, 1000);
    x.k_set_tty(pid, tt); x.k_setsid(pid); x.k_tty_set_fg(tt, pid);
    const task = makeTask(pid, true); task.shell = true; task.tty = tt;
    const inst = instantiate(pid, mod, task);
    const prev = S.cur; S.cur = task;
    try { inst.exports._start(); } catch (e) { if (!(e instanceof ParkSignal)) throw e; } finally { S.cur = prev; }
    task.sync = false;
    if (B.task) B.stack.push({ pid: B.pid, task: B.task });
    B.pid = pid; B.task = task;
  }
  const brushStr = (tt, ptr) => { const ex = Bs[tt].task.inst.exports; return dec.decode(new Uint8Array(ex.memory.buffer, ptr, ex.brush_out_len())); };
  function takeTty(tt) { const n = K.x.k_tty_take(tt || 0); return n ? stagedStr(n) : ""; }
  function pushHostInfo() {
    const mem = Math.round(((performance.memory && performance.memory.usedJSHeapSize) || K.x.memory.buffer.byteLength) / 1048576);
    const total = Math.max(mem, Math.round((navigator.deviceMemory || 4) * 1024));
    K.x.k_set_hostinfo(total * 1024, mem * 1024, navigator.hardwareConcurrency || 1);
  }
  function brushRun(tt, line) {
    pushHostInfo(); const B = Bs[tt];
    const task = B.task, ex = task.inst.exports, bs = enc.encode(line), p = ex.brush_alloc(bs.length);
    new Uint8Array(ex.memory.buffer).set(bs, p);
    K.x.k_tty_flush_input(tt);
    return new Promise((resolve) => {
      launch(task, () => { task.result = ex.brush_run(p, bs.length); }, (trapCode) => {
        let code = trapCode === 134 ? 134 : task.result;
        let exited = code <= -1000;
        let out = takeTty(tt);
        if (trapCode === 134) out += "brush: trapped\n";
        if (exited && B.stack.length && B.task === task) {
          K.x.k_proc_exit(B.pid, -1000 - code); K.x.k_wait(B.pid);
          const prev = B.stack.pop(); B.pid = prev.pid; B.task = prev.task; exited = false;
        }
        resolve({ out, code: exited ? -1000 - code : code, exited });
      });
      pump();
    });
  }
  const brushPrompt = (tt) => brushStr(tt, Bs[tt].task.inst.exports.brush_prompt()).replace(/[\x01\x02]/g, "");
  function brushComplete(tt, text) { // text = line up to the cursor
    const B = Bs[tt]; if (!B || !B.task || !B.task.inst) return null;
    const ex = B.task.inst.exports, bs = enc.encode(text), p = ex.brush_alloc(bs.length);
    new Uint8Array(ex.memory.buffer).set(bs, p);
    const r = brushStr(tt, ex.brush_complete(p, bs.length, bs.length));
    if (!r) return null;
    const lines = r.split("\n"), [ins, del, files] = lines[0].split("\t");
    return { ins: +ins, del: +del, files: files === "1", cands: lines.slice(1).filter((x) => x.length) };
  }
  const brushCwd = (tt) => brushStr(tt, Bs[tt].task.inst.exports.brush_cwd());

  /* ---- public API ---- */
  async function boot(kernelB64, extraModules) {
    const bytes = await blobBytes(kernelB64);
    const imports = {
      host: {
        console_write(ptr, len) { K.consoleBuf += dec.decode(new Uint8Array(K.x.memory.buffer, ptr, len)); },
        run_process: runProcess,
        now_ns: () => (performance.timeOrigin + performance.now()) * 1e6,
      },
    };
    const { instance } = await WebAssembly.instantiate(bytes, imports);
    K.x = instance.exports;
    return K;
  }

  async function registerModule(name, b64) {
    K.modules[name] = await WebAssembly.compile(await blobBytes(b64));
  }

  function installSetuid(name, bins) {
    const x = K.x;
    const marker = enc.encode("#!mavex-module " + name + "\n");
    for (const b of bins) {
      const file = "/bin/" + b;
      const d = x.k_alloc(marker.length);
      u8().set(marker, d);
      const [[p, n]] = packed(file);
      x.k_unlink(p, n);
      x.k_write_file(p, n, d, marker.length);
      x.k_free(d, marker.length);
      x.k_chown(...packed(file)[0], 0, 0);
      x.k_chmod(...packed(file)[0], 0o4755);
    }
  }

  function install(name, utils) {
    const x = K.x, file = "/bin/" + name;
    const marker = enc.encode("#!mavex-module " + name + "\n");
    const d = x.k_alloc(marker.length);
    u8().set(marker, d);
    const [[p, n]] = packed(file);
    x.k_write_file(p, n, d, marker.length);
    x.k_free(d, marker.length);
    x.k_chmod(...packed(file)[0], 0o755);
    for (const u of utils) {
      const [[lp, ln]] = packed("/bin/" + u);
      x.k_unlink(lp, ln);
      const [[tp, tn], [lp2, ln2]] = packed(name, "/bin/" + u);
      x.k_symlink(tp, tn, lp2, ln2);
    }
  }

  window.MavexRuntime = { brushComplete, ttyReset, winch, startInit, startProcess, S, pump, killForeground, stopForeground, sendSignal, foregroundBusy, startBrush, newTty, brushRun, brushPrompt, brushCwd, takeTty, boot, registerModule, install, installSetuid, K, packed, staged, stagedStr, scratch };
})();

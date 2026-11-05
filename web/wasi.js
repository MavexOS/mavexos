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

  /* ---- WASI for one process ---- */
  function makeWasi(pid, getMem) {
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
      sched_yield() { return 0; },
      proc_exit(code) { throw new ExitSignal(code); },
      proc_raise() { throw new ExitSignal(134); },
      poll_oneoff(inp, out, nsubs, nev) {
        const d = dv(); let deadline = 0, n = 0;
        for (let i = 0; i < nsubs; i++) {
          const s = inp + i * 48, tag = d.getUint8(s + 8);
          if (tag === 0) { const t = Number(d.getBigUint64(s + 24, true)) / 1e6; const abs = d.getUint16(s + 40, true) & 1; deadline = Math.max(deadline, abs ? t : performance.timeOrigin + performance.now() + t); }
        }
        if (deadline) { const end = deadline - performance.timeOrigin; while (performance.now() < end) { /* run-to-completion: busy wait */ } }
        for (let i = 0; i < nsubs; i++) {
          const s = inp + i * 48, e = out + n * 32;
          dv().setBigUint64(e, d.getBigUint64(s, true), true); d.setUint16(e + 8, 0, true); d.setUint8(e + 10, d.getUint8(s + 8)); n++;
        }
        setU32(nev, n); return 0;
      },
      fd_close(fd) { return errOf(x.k_fd_close(pid, fd)); },
      fd_read(fd, iovs, cnt, nout) {
        const r = x.k_fd_read(pid, fd, iovTotal(iovs, cnt));
        if (r < 0) return -r;
        setU32(nout, scatter(iovs, cnt, staged(r))); return 0;
      },
      fd_pread(fd, iovs, cnt, off, nout) {
        const r = x.k_fd_pread(pid, fd, iovTotal(iovs, cnt), Number(off));
        if (r < 0) return -r;
        setU32(nout, scatter(iovs, cnt, staged(r))); return 0;
      },
      fd_write(fd, iovs, cnt, nout) {
        const [p, n] = gather(iovs, cnt), r = x.k_fd_write(pid, fd, p, n);
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
      fd_tell(fd, out) { return this.fd_seek(fd, 0n, 1, out); },
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

  function runProcess(pid) {
    const x = K.x, err = (s) => { const [[p, n]] = packed(s); x.k_fd_write(pid, 2, p, n); };
    let code = 0;
    try {
      const mod = resolveModule(pid);
      if (!mod) { err("mavexos: cannot execute: not a program\n"); x.k_proc_exit(pid, 126); return; }
      let memory = null;
      const wasi = makeWasi(pid, () => memory);
      const inst = new WebAssembly.Instance(mod, { wasi_snapshot_preview1: wasi });
      memory = inst.exports.memory;
      inst.exports._start();
    } catch (e) {
      if (e instanceof ExitSignal) code = e.code;
      else { err("mavexos: program trapped: " + (e && e.message || e) + "\n"); code = 134; }
    }
    x.k_proc_exit(pid, code);
  }

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

  window.MavexRuntime = { boot, registerModule, install, K, packed, staged, stagedStr, scratch };
})();

const $ = (id) => document.getElementById(id);
const FAST = location.hash.includes("fast");
const sleep = (ms) => new Promise(r => setTimeout(r, ms));

/* ---------- kernel ---------- */
const R = window.MavexRuntime;
let K, mem;
const enc = new TextEncoder(), dec = new TextDecoder();
const put = (bytes) => { const p = K.k_alloc(bytes.length); new Uint8Array(K.memory.buffer, p, bytes.length).set(bytes); return [p, bytes.length]; };
const withStr = (s, f) => { const [p, n] = put(enc.encode(s)); try { return f(p, n); } finally { K.k_free(p, n); } };
const staged = (n) => dec.decode(new Uint8Array(K.memory.buffer, K.k_result_ptr(), n));
const useBrush = !location.hash.includes("msh");
const LOGIN_ENV = ["PATH=/bin", "HOME=/home/user", "USER=user", "LOGNAME=user", "HOSTNAME=mavex", "SHELL=/bin/brush", "LANG=en_US.UTF-8", "TERM=xterm-256color", "TERM_PROGRAM=mavex-terminal", "PWD=/home/user",
  "PS1=\\[\\e[1;32m\\]\\u@mavex\\[\\e[0m\\]:\\[\\e[1;34m\\]\\w\\[\\e[0m\\]\\$ "];
const startLogin = (tt) => R.startBrush("/home/user", LOGIN_ENV, 1, tt);
const sys = {
  mkdir: (path) => withStr(path, (p, n) => K.k_mkdir(p, n)),
  unlink: (path) => withStr(path, (p, n) => K.k_unlink(p, n)),
  writeFile: (path, text) => withStr(path, (p, n) => { const [d, dn] = put(enc.encode(text)); try { return K.k_write_file(p, n, d, dn); } finally { K.k_free(d, dn); } }),
  readFile: (path) => withStr(path, (p, n) => { const r = K.k_read_file(p, n); return r < 0 ? r : staged(r); }),
  readdir: (path) => withStr(path, (p, n) => { const r = K.k_readdir(p, n); return r < 0 ? r : (r === 0 ? [] : staged(r).split("\n")); }),
  complete: (word, first) => withStr(word, (p, n) => { const r = K.k_shell_complete(p, n, first ? 1 : 0); return r === 0 ? [] : staged(r).split("\n"); }),
};

/* ---------- network policy (fetch import) ---------- */
const NET_CONF = "/etc/mavex/network";
const NET_DEFAULT = `# MavexOS network policy (read on every request)
# mode: direct-only | direct-first | relay-first | relay-only
#   direct-only  browser fetch(); needs the site to send CORS headers; uses the browser's own DNS
#   direct-first direct, then the relays below if the site cannot be reached
#   relay-first  relays in order (helps against blocked sites), then direct as a last resort
#   relay-only   never connect directly
# A relay can read everything that passes through it. Do not use it for private or financial data.
mode = direct-only
timeout = 30
# {url} is the percent-encoded target, {raw} the plain target. GET and HEAD only.
relay = https://r.jina.ai/{raw}
relay = https://api.allorigins.win/raw?url={url}
relay = https://api.codetabs.com/v1/proxy/?quest={url}
# DNS-over-HTTPS resolvers for MavexOS's own resolver (tried in order, first answer wins).
# This only answers DNS lookups; fetch() still uses the browser's DNS to connect.
doh = https://cloudflare-dns.com/dns-query
doh = https://dns.google/dns-query
doh = https://dns.quad9.net/dns-query
`;
function netPolicy() {
  const txt = typeof sys.readFile(NET_CONF) === "string" ? sys.readFile(NET_CONF) : NET_DEFAULT;
  const p = { mode: "direct-only", timeout: 30, relays: [], doh: [] };
  for (const l of txt.split("\n")) {
    const m = l.match(/^\s*(mode|timeout|relay|doh)\s*=\s*(\S+)\s*$/);
    if (!m) continue;
    if (m[1] === "relay") p.relays.push(m[2]); else if (m[1] === "doh") p.doh.push(m[2]); else if (m[1] === "timeout") p.timeout = Math.max(1, +m[2] || 30); else p.mode = m[2];
  }
  return p;
}
async function netAttempt(url, req, secs) {
  const ctl = new AbortController(), t = setTimeout(() => ctl.abort(), secs * 1000);
  const headers = new Headers();
  for (const l of req.headers.split("\n")) { const i = l.indexOf(":"); if (i > 0) try { headers.set(l.slice(0, i).trim(), l.slice(i + 1).trim()); } catch (_) {} }
  try {
    const r = await fetch(url, { method: req.method, headers, body: req.body, redirect: "follow", signal: ctl.signal, credentials: "omit", cache: "no-store" });
    let hd = "";
    r.headers.forEach((v, k) => { hd += k + ": " + v + "\n"; });
    return { kind: 0, status: r.status, headers: hd, body: new Uint8Array(await r.arrayBuffer()), note: r.url && r.url !== url ? "final url: " + r.url : "" };
  } catch (e) {
    if (ctl.signal.aborted) return { kind: 1, note: "timed out after " + secs + "s" };
    // The browser hides why fetch failed; an opaque no-cors probe tells "reachable but no CORS headers" from "unreachable".
    try { await fetch(url, { mode: "no-cors", method: "GET", signal: AbortSignal.timeout(secs * 1000), credentials: "omit" }); return { kind: 2, note: "blocked by CORS: the server answered but does not allow cross-origin reads" }; }
    catch (_) { return { kind: 1, note: "could not reach the server (DNS failure, blocked, offline, or an https certificate problem)" }; }
  }
}
globalThis.mavexFetch = async (req) => {
  if (req.method === "DNS") return globalThis.mavexDns(req.body);
  const pol = netPolicy();
  const direct = () => netAttempt(req.url, req, pol.timeout);
  const viaRelays = async () => {
    if (!/^(GET|HEAD)$/i.test(req.method)) return { kind: 1, note: "relays only support GET and HEAD" };
    let last = { kind: 1, note: "no relays configured" };
    for (const tpl of pol.relays) {
      const r = await netAttempt(tpl.replace("{url}", encodeURIComponent(req.url)).replace("{raw}", req.url), req, pol.timeout);
      if (r.kind === 0 && r.status < 500 && r.status !== 401 && r.status !== 403 && r.status !== 429) { r.note = "via relay " + new URL(tpl.replace(/\{(url|raw)\}/g, "x")).host + " (the relay can read this request and response)"; return r; }
      last = { kind: r.kind || 1, note: "relay " + new URL(tpl.replace(/\{(url|raw)\}/g, "x")).host + ": " + (r.note || "HTTP " + r.status) };
    }
    return last;
  };
  const order = { "direct-only": [direct], "direct-first": [direct, viaRelays], "relay-first": [viaRelays, direct], "relay-only": [viaRelays] }[pol.mode] || [direct];
  let r = null;
  for (const step of order) { r = await step(); if (r.kind === 0) return r; }
  return r;
};

// DNS wire message in, wire message out, over DoH (RFC 8484 POST). Queries are padded to a multiple of 128 bytes
// with an EDNS(0) padding option (RFC 7830/8467) so lengths leak less. Failover across the configured resolvers.
function dnsPad(q) {
  const u = new Uint8Array(q), dv = new DataView(u.buffer);
  if (u.length < 12 || dv.getUint16(10) > 0) return u;
  // Only add padding when the query has no OPT record: append OPT(+padding) and bump ARCOUNT.
  const hdr = 11, opt = 4 + 7;
  let ar = dv.getUint16(10);
  const base = u.length + hdr;
  const pad = (128 - ((base + 4) % 128)) % 128;
  const out = new Uint8Array(base + 4 + pad);
  out.set(u);
  const o = new DataView(out.buffer);
  let i = u.length;
  out[i++] = 0; o.setUint16(i, 41); i += 2; o.setUint16(i, 1232); i += 2; o.setUint32(i, 0); i += 4;
  o.setUint16(i, 4 + pad); i += 2; o.setUint16(i, 12); i += 2; o.setUint16(i, pad); i += 2;
  o.setUint16(10, ar + 1);
  return out;
}
globalThis.mavexDns = async (wire) => {
  const pol = netPolicy();
  const q = dnsPad(wire);
  let last = "no DoH resolvers configured";
  for (const ep of pol.doh) {
    try {
      const r = await fetch(ep, { method: "POST", headers: { "content-type": "application/dns-message", accept: "application/dns-message" }, body: q, credentials: "omit", cache: "no-store", referrerPolicy: "no-referrer", signal: AbortSignal.timeout(pol.timeout * 1000) });
      if (r.ok) return { kind: 0, body: new Uint8Array(await r.arrayBuffer()), note: new URL(ep).host };
      last = new URL(ep).host + ": HTTP " + r.status;
    } catch (e) { last = new URL(ep).host + ": " + (e && e.message || "failed"); }
  }
  return { kind: 1, note: last };
};

/* ---------- persistence (IndexedDB) ---------- */
const PERSIST = {
  db: null, known: new Map(), lastHash: -1, saving: false,
  open() { return new Promise((res) => { try { const rq = indexedDB.open("mavexos", 1); rq.onupgradeneeded = () => rq.result.createObjectStore("fs"); rq.onsuccess = () => res(rq.result); rq.onerror = () => res(null); rq.onblocked = () => res(null); } catch (e) { res(null); } }); },
  tx(mode, f) { return new Promise((res, rej) => { const t = this.db.transaction("fs", mode); const r = f(t.objectStore("fs")); t.oncomplete = () => res(r && r.result); t.onerror = t.onabort = () => rej(t.error); }); },
  split(blob) {
    const out = []; const dv = new DataView(blob.buffer, blob.byteOffset, blob.byteLength); let o = 0;
    while (o + 21 <= blob.length) {
      const st = o; const pl = dv.getUint32(o + 21, true); const path = dec.decode(blob.subarray(o + 25, o + 25 + pl));
      const dl = dv.getUint32(o + 25 + pl, true); o += 29 + pl + dl;
      out.push({ path, bytes: blob.slice(st, o), fp: Array.from(blob.subarray(st, st + 21)).join(",") + ":" + path + ":" + dl });
    }
    return out;
  },
  async init() {
    this.db = await this.open();
    if (!this.db) return -1;
    if (location.hash.includes("reset")) await this.tx("readwrite", (s) => s.clear());
    const rows = await this.tx("readonly", (s) => s.getAll());
    let count = 0;
    if (rows && rows.length) {
      const total = rows.reduce((a, b) => a + b.length, 0); const blob = new Uint8Array(total); let o = 0;
      for (const r of rows) { blob.set(r, o); o += r.length; }
      const [p, n] = put(blob); try { count = K.k_persist_restore(p, n); } finally { K.k_free(p, n); }
      for (const e of this.split(blob)) this.known.set(e.path, e.fp);
    }
    this.lastHash = K.k_persist_hash();
    setInterval(() => this.save(), 2000);
    addEventListener("pagehide", () => this.save(true));
    return count;
  },
  async save(force) {
    if (!this.db || this.saving) return;
    const h = K.k_persist_hash();
    if (h === this.lastHash && !force) return;
    this.saving = true;
    try {
      const n = K.k_persist_dump(); const blob = new Uint8Array(K.memory.buffer, K.k_result_ptr(), n).slice();
      const cur = this.split(blob); const seen = new Set();
      const put_ = [], del = [];
      for (const e of cur) { seen.add(e.path); if (this.known.get(e.path) !== e.fp) put_.push(e); }
      for (const k of this.known.keys()) if (!seen.has(k)) del.push(k);
      if (put_.length || del.length) await this.tx("readwrite", (s) => { for (const e of put_) s.put(e.bytes, e.path); for (const k of del) s.delete(k); });
      for (const e of put_) this.known.set(e.path, e.fp);
      for (const k of del) this.known.delete(k);
      this.lastHash = h;
    } catch (e) { console.warn("persist save failed", e); }
    this.saving = false;
  },
};

globalThis.mavexPower = async (kind) => {
  try { await PERSIST.save(true); } catch (e) {}
  if (kind === 1) { location.hash = location.hash.replace(/,?reset/, ""); location.reload(); return; }
  const o = document.createElement("div");
  o.style.cssText = "position:fixed;inset:0;background:#000;color:#aaa;display:flex;align-items:center;justify-content:center;font:16px monospace;z-index:99;cursor:pointer;white-space:pre;text-align:center";
  o.textContent = (kind === 2 ? "System halted." : "Powered off.") + "\n\nClick or press any key to boot again.";
  document.body.appendChild(o);
  const again = () => { location.hash = location.hash.replace(/,?reset/, ""); location.reload(); };
  o.addEventListener("click", again); addEventListener("keydown", again, { once: true });
};

const write = (s) => tabs.cur.write(s);
function makeTab(tt, els) {
const { screenEl, out, lineEl, promptEl, beforeEl, curEl, afterEl, altEl } = els;
const vt = window.MavexVT.create(altEl, screenEl);
const ts = R.S.ttys[tt];
const isCur = () => tabs.cur === self;
let self = null;
let sgr = { cls: "" };
function ansiToNodes(text) {
  const re = /\x1b\[([0-9;]*)([A-Za-z])/g;
  let last = 0, m;
  const emit = (s) => { if (!s) return; const sp = document.createElement("span"); if (sgr.cls) sp.className = sgr.cls; sp.textContent = s; out.appendChild(sp); };
  while ((m = re.exec(text))) {
    emit(text.slice(last, m.index));
    last = re.lastIndex;
    if (m[2] === "J") { out.textContent = ""; nlines = 0; }
    else if (m[2] === "m") {
      const codes = m[1] === "" ? [0] : m[1].split(";").map(Number);
      let bold = sgr.cls.includes("b"), fg = (sgr.cls.match(/c\d+/) || [""])[0], bg = (sgr.cls.match(/g\d+/) || [""])[0];
      for (const c of codes) {
        if (c === 0) { bold = false; fg = ""; bg = ""; }
        else if (c === 1) bold = true;
        else if (c === 22) bold = false;
        else if (c === 39) fg = "";
        else if (c === 49) bg = "";
        else if ((c >= 30 && c <= 37) || (c >= 90 && c <= 97)) fg = "c" + c;
        else if ((c >= 40 && c <= 47) || (c >= 100 && c <= 107)) bg = "g" + c;
      }
      sgr.cls = [bold ? "b" : "", fg, bg].filter(Boolean).join(" ");
    }
  }
  emit(text.slice(last));
}
const MAX_LINES = 5000;
let nlines = 0;
function trimScrollback() {
  while (nlines > MAX_LINES && out.firstChild) { const n = (out.firstChild.textContent.match(/\n/g) || []).length; nlines -= n; out.removeChild(out.firstChild); }
}
function write(text) {
  text = vt.process(text);
  if (!text) return;
  nlines += (text.match(/\n/g) || []).length;
  ansiToNodes(text); trimScrollback(); scrollDown();
}
function scrollDown() { if (isCur()) screenEl.scrollTop = screenEl.scrollHeight; else pendingScroll = true; }

let brushExited = false;
const shellRun = (line) => (useBrush ? R.brushRun(tt, line).then((r) => { brushExited = r.exited; return r.out; }) : withStr(line, (p, n) => staged(K.k_shell_run(p, n))));
const prompt = () => (useBrush ? R.brushPrompt(tt) : staged(K.k_shell_prompt()));
const exited = () => (useBrush ? brushExited : K.k_shell_exited() === 1);
const reset = () => { if (useBrush) { brushExited = false; startLogin(tt); } else K.k_shell_reset(); };
let pendingScroll = false;
const HIST = "/home/user/.bash_history";
let histLoaded = false;
let buf = "", pos = 0, history = [], hidx = 0, draft = "", promptText = "", live = false, dead = false;
const stripAnsi = (s) => s.replace(/\x1b\[[0-9;]*[A-Za-z]/g, "");
function renderPrompt() {
  const el = promptEl; el.textContent = "";
  const re = /\x1b\[([0-9;]*)m/g; let last = 0, m, cls = "";
  const add = (s) => { if (!s) return; const sp = document.createElement("span"); if (cls) sp.className = cls; sp.textContent = s; el.appendChild(sp); };
  while ((m = re.exec(promptText))) {
    add(promptText.slice(last, m.index)); last = re.lastIndex;
    const codes = m[1] === "" ? [0] : m[1].split(";").map(Number);
    let bold = cls.includes("b"), fg = (cls.match(/c\d+/) || [""])[0];
    for (const c of codes) { if (c === 0) { bold = false; fg = ""; } else if (c === 1) bold = true; else if (c >= 30 && c <= 37) fg = "c" + c; }
    cls = [bold ? "b" : "", fg].filter(Boolean).join(" ");
  }
  add(promptText.slice(last));
}
function renderLine() {
  const hide = running && !ts.echo;
  beforeEl.textContent = hide ? "" : buf.slice(0, pos);
  curEl.textContent = !hide && pos < buf.length ? buf[pos] : " ";
  afterEl.textContent = hide ? "" : buf.slice(pos + 1);
  scrollDown();
}
function showLine(show) { lineEl.style.display = show ? "" : "none"; }
function newPrompt() { if (!histLoaded) { histLoaded = true; const h = sys.readFile(HIST); if (typeof h === "string") history = h.split("\n").filter(Boolean).slice(-1000); } promptText = prompt(); renderPrompt(); buf = ""; pos = 0; hidx = history.length; renderLine(); showLine(true); live = true; }

function commitInput(extra) {
  write(promptText); // colored prompt echo
  write(buf + (extra || "") + "\n");
}
let running = false;
vt.onReply = (s) => feed(s);
const feed = (str) => { const b = enc.encode(str), [p, n] = put(b); try { K.k_tty_input(tt, p, n); } finally { K.k_free(p, n); } R.pump(); };
const RAW_KEYS = { Enter: "\r", Backspace: "\x7f", Tab: "\t", Escape: "\x1b", ArrowUp: "\x1b[A", ArrowDown: "\x1b[B", ArrowRight: "\x1b[C", ArrowLeft: "\x1b[D", Home: "\x1b[H", End: "\x1b[F", Delete: "\x1b[3~", PageUp: "\x1b[5~", PageDown: "\x1b[6~", Insert: "\x1b[2~", F1: "\x1bOP", F2: "\x1bOQ", F3: "\x1bOR", F4: "\x1bOS", F5: "\x1b[15~", F6: "\x1b[17~", F7: "\x1b[18~", F8: "\x1b[19~", F9: "\x1b[20~", F10: "\x1b[21~", F11: "\x1b[23~", F12: "\x1b[24~" };
function rawKey(e) {
  const k = e.key;
  if (e.ctrlKey && !e.altKey && !e.metaKey && k.length === 1) {
    const l = k.toLowerCase();
    if (l === "c" && ts.isig) { e.preventDefault(); R.killForeground(tt); return; }
    if (l === "z" && ts.isig) { e.preventDefault(); R.stopForeground(tt); return; }
    if (l >= "a" && l <= "z") { e.preventDefault(); feed(String.fromCharCode(l.charCodeAt(0) - 96)); return; }
    return;
  }
  if (e.metaKey) return;
  if (vt.appCursor && /^Arrow|^Home$|^End$/.test(k)) { e.preventDefault(); feed("\x1bO" + { ArrowUp: "A", ArrowDown: "B", ArrowRight: "C", ArrowLeft: "D", Home: "H", End: "F" }[k]); return; }
  if (RAW_KEYS[k]) { e.preventDefault(); feed(RAW_KEYS[k]); if (ts.echo && k === "Enter") write("\n"); return; }
  if (k.length === 1) { e.preventDefault(); feed(k); if (ts.echo) write(k); }
}
async function runLine(line) {
  running = true; promptText = ""; renderPrompt(); buf = ""; pos = 0; renderLine(); showLine(true); live = true;
  try { write(await shellRun(line)); } finally { running = false; R.ttyReset(tt); vt.leave(); vt.resetModes(); }
}
let pend = null;
function needMore(text) {
  const lines = text.split("\n");
  let tag = null, dash = false;
  for (const ln of lines) {
    if (tag !== null) { if ((dash ? ln.replace(/^\t+/, "") : ln) === tag) tag = null; continue; }
    const m = ln.match(/<<(-?)\s*(?:'([^']+)'|"([^"]+)"|\\?([A-Za-z_][\w]*))/);
    if (m && !/<<</.test(ln)) { tag = m[2] || m[3] || m[4]; dash = m[1] === "-"; }
  }
  if (tag !== null) return true;
  let q = null;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (q) { if (c === q) q = null; else if (q === '"' && c === "\\") i++; }
    else if (c === "\\") i++;
    else if (c === "'" || c === '"') q = c;
  }
  return q !== null || /\\$/.test(text);
}
async function enter() {
  const line = buf;
  if (running) { write((ts.echo ? buf : "") + "\n"); buf = ""; pos = 0; hidx = history.length; renderLine(); feed(line + "\n"); return; }
  live = false;
  if (dead && tabs.list.length > 1) { closeTab(self); return; }
  if (dead) { write(promptText + "\n"); dead = false; reset(); write("\x1b[0m"); newPrompt(); return; }
  commitInput();
  if (useBrush) {
    const full = pend === null ? line : pend + "\n" + line;
    if (needMore(full)) { pend = full; promptText = "> "; buf = ""; pos = 0; hidx = history.length; renderPrompt(); renderLine(); live = true; return; }
    if (pend !== null) { pend = null; if (full.trim()) { history.push(full.replace(/\n/g, " ")); sys.writeFile(HIST, history.join("\n") + "\n"); await runLine(full); if (exited() && tabs.list.length > 1) { closeTab(self); return; } newPrompt(); return; } }
  }
  if (line.trim() && history[history.length - 1] !== line) { history.push(line); if (history.length > 1000) history.shift(); sys.writeFile(HIST, history.join("\n") + "\n"); }
  if (line.trim()) await runLine(line);
  if (exited() && tabs.list.length > 1) { closeTab(self); return; }
  if (exited()) { dead = true; write("\n[process completed - press Enter to restart]\n"); promptText = ""; buf = ""; pos = 0; renderPrompt(); renderLine(); live = true; return; }
  newPrompt();
}
function setBuf(s) { buf = s; pos = s.length; renderLine(); }
function prevWordStart() { let i = pos; while (i > 0 && buf[i - 1] === " ") i--; while (i > 0 && buf[i - 1] !== " ") i--; return i; }
function commonPrefix(a) { let p = a[0]; for (const s of a) while (!s.startsWith(p)) p = p.slice(0, -1); return p; }
const esc = (c) => c.replace(/([ \t'"\\$&;()<>|*?\[\]#!{}`])/g, "\\$1");
function tabBrush() {
  const left = buf.slice(0, pos), lb = enc.encode(left);
  const r = R.brushComplete(tt, left);
  if (!r || !r.cands.length) return;
  const start = dec.decode(lb.slice(0, r.ins)).length, end = dec.decode(lb.slice(0, r.ins + r.del)).length;
  const typed = left.slice(start, end);
  const fix = (c) => (r.files ? esc(c) : c);
  const apply = (text) => { buf = buf.slice(0, start) + text + buf.slice(pos); pos = start + text.length; renderLine(); };
  if (r.cands.length === 1) { const c = r.cands[0]; apply(fix(c) + (c.endsWith("/") ? "" : " ")); return; }
  const pre = fix(commonPrefix(r.cands));
  if (pre.length > typed.length) { apply(pre); return; }
  write(promptText); write(buf + "\n" + r.cands.join("  ") + "\n"); renderLine();
}
function tab() {
  if (useBrush && !running) return tabBrush();
  const left = buf.slice(0, pos);
  const start = left.lastIndexOf(" ") + 1;
  const word = left.slice(start);
  const isFirst = left.slice(0, start).trim() === "";
  const c = sys.complete(word, isFirst);
  if (!c.length) return;
  if (c.length === 1) {
    const rep = c[0] + (c[0].endsWith("/") ? "" : " ");
    buf = buf.slice(0, start) + rep + buf.slice(pos); pos = start + rep.length; renderLine(); return;
  }
  const pre = commonPrefix(c);
  if (pre.length > word.length) { buf = buf.slice(0, start) + pre + buf.slice(pos); pos = start + pre.length; renderLine(); return; }
  write(promptText); write(buf + "\n" + c.join("  ") + "\n"); renderLine();
}
let killed = "", rs = null;
function nextWordEnd() { let i = pos; while (i < buf.length && buf[i] === " ") i++; while (i < buf.length && buf[i] !== " ") i++; return i; }
function kill(a, b) { killed = buf.slice(a, b); buf = buf.slice(0, a) + buf.slice(b); pos = a; renderLine(); }
function rsFind(from) {
  for (let i = from; i >= 0; i--) if (history[i].includes(rs.q)) { rs.idx = i; return; }
  rs.idx = -1;
}
function rsRender() {
  const m = rs.idx >= 0 ? history[rs.idx] : "";
  beforeEl.textContent = "(reverse-i-search)`" + rs.q + "': " + m; curEl.textContent = " "; afterEl.textContent = ""; scrollDown();
}
function rsKey(e) {
  const k = e.key, l = k.toLowerCase();
  if (e.ctrlKey && l === "r") { e.preventDefault(); rsFind((rs.idx >= 0 ? rs.idx : history.length) - 1); rsRender(); return; }
  if ((e.ctrlKey && (l === "g" || l === "c")) || k === "Escape") { e.preventDefault(); rs = null; renderLine(); return; }
  if (k === "Backspace") { e.preventDefault(); rs.q = rs.q.slice(0, -1); rsFind(history.length - 1); rsRender(); return; }
  if (!e.ctrlKey && !e.altKey && !e.metaKey && k.length === 1) { e.preventDefault(); rs.q += k; rsFind(rs.idx >= 0 ? rs.idx : history.length - 1); rsRender(); return; }
  e.preventDefault();
  const m = rs.idx >= 0 ? history[rs.idx] : buf; rs = null; buf = m; pos = m.length; renderLine();
  if (k === "Enter") enter();
}
function insert(s) { buf = buf.slice(0, pos) + s + buf.slice(pos); pos += s.length; renderLine(); }

function onKey(e) {
  if (!live) return;
  if (e.shiftKey && !e.ctrlKey && !e.altKey && (e.key === "PageUp" || e.key === "PageDown") && !vt.active) { e.preventDefault(); screenEl.scrollBy(0, (e.key === "PageUp" ? -1 : 1) * screenEl.clientHeight * 0.9); return; }
  if (running && ts.raw) { rawKey(e); return; }
  const k = e.key;
  if (rs) { rsKey(e); return; }
  if (e.altKey && !e.ctrlKey && !e.metaKey && !running) {
    if (k === "b") { e.preventDefault(); pos = prevWordStart(); renderLine(); return; }
    if (k === "f") { e.preventDefault(); pos = nextWordEnd(); renderLine(); return; }
    if (k === "d") { e.preventDefault(); kill(pos, nextWordEnd()); return; }
    if (k === "Backspace") { e.preventDefault(); kill(prevWordStart(), pos); return; }
  }
  if (e.ctrlKey && !e.altKey && !e.metaKey) {
    const l = k.toLowerCase();
    if (l === "r" && !running) { e.preventDefault(); rs = { q: "", idx: -1 }; rsRender(); return; }
    if (!running && l === "b") { e.preventDefault(); if (pos > 0) { pos--; renderLine(); } return; }
    if (!running && l === "f") { e.preventDefault(); if (pos < buf.length) { pos++; renderLine(); } return; }
    if (!running && l === "p") { e.preventDefault(); if (hidx > 0) { if (hidx === history.length) draft = buf; hidx--; setBuf(history[hidx]); } return; }
    if (!running && l === "n") { e.preventDefault(); if (hidx < history.length) { hidx++; setBuf(hidx === history.length ? draft : history[hidx]); } return; }
    if (!running && l === "h") { e.preventDefault(); if (pos > 0) { buf = buf.slice(0, pos - 1) + buf.slice(pos); pos--; renderLine(); } return; }
    if (!running && l === "y") { e.preventDefault(); insert(killed); return; }
    if (!running && l === "t") { e.preventDefault(); if (pos > 0 && buf.length > 1) { const p = pos === buf.length ? pos - 1 : pos; buf = buf.slice(0, p - 1) + buf[p] + buf[p - 1] + buf.slice(p + 1); pos = p + 1; renderLine(); } return; }
    if (running && !ts.isig && (l === "c" || l === "z")) { e.preventDefault(); return; }
    if (l === "c" && running && !(window.getSelection().toString())) { e.preventDefault(); write(buf + "^C\n"); buf = ""; pos = 0; renderLine(); R.killForeground(tt); return; }
    if (l === "z" && running) { e.preventDefault(); write("^Z\n"); buf = ""; pos = 0; renderLine(); R.stopForeground(tt); return; }
    if (l === "d" && running) { e.preventDefault(); if (!buf.length) { K.k_tty_eof(tt); R.pump(); } return; }
    if (l === "c" && !(window.getSelection().toString())) { e.preventDefault(); write(promptText); write(buf + "^C\n"); newPrompt(); return; }
    if (l === "c") return;
    if (l === "v") return;
    if (l === "l") { e.preventDefault(); out.textContent = ""; renderLine(); return; }
    if (l === "u") { e.preventDefault(); kill(0, pos); return; }
    if (l === "k") { e.preventDefault(); kill(pos, buf.length); return; }
    if (l === "a") { e.preventDefault(); pos = 0; renderLine(); return; }
    if (l === "e") { e.preventDefault(); pos = buf.length; renderLine(); return; }
    if (l === "w") { e.preventDefault(); kill(prevWordStart(), pos); return; }
    if (l === "d") { e.preventDefault(); if (!buf.length) { buf = "exit"; pos = 4; enter(); } return; }
    return;
  }
  if (e.metaKey || e.altKey) return;
  switch (k) {
    case "Enter": e.preventDefault(); enter(); return;
    case "Backspace": e.preventDefault(); if (pos > 0) { buf = buf.slice(0, pos - 1) + buf.slice(pos); pos--; renderLine(); } return;
    case "Delete": e.preventDefault(); if (pos < buf.length) { buf = buf.slice(0, pos) + buf.slice(pos + 1); renderLine(); } return;
    case "ArrowLeft": e.preventDefault(); if (pos > 0) { pos--; renderLine(); } return;
    case "ArrowRight": e.preventDefault(); if (pos < buf.length) { pos++; renderLine(); } return;
    case "Home": e.preventDefault(); pos = 0; renderLine(); return;
    case "End": e.preventDefault(); pos = buf.length; renderLine(); return;
    case "ArrowUp": e.preventDefault(); if (hidx > 0) { if (hidx === history.length) draft = buf; hidx--; setBuf(history[hidx]); } return;
    case "ArrowDown": e.preventDefault(); if (hidx < history.length) { hidx++; setBuf(hidx === history.length ? draft : history[hidx]); } return;
    case "Tab": e.preventDefault(); tab(); return;
  }
  if (k.length === 1) { e.preventDefault(); insert(k); }
}
function onPaste(e) {
  if (!live) return;
  e.preventDefault();
  const text = (e.clipboardData || window.clipboardData).getData("text");
  if (running && !ts.icanon) { feed(vt.paste ? "\x1b[200~" + text + "\x1b[201~" : text.replace(/\n/g, "\r")); return; }
  const parts = text.replace(/\r/g, "").split("\n");
  parts.forEach((p, i) => { insert(p); if (i < parts.length - 1) enter(); });
}

  let down = -1, last = "";
  const cell = (e) => {
    const r = altEl.getBoundingClientRect(), cs = getComputedStyle(altEl);
    const x = Math.floor((e.clientX - r.left - (parseFloat(cs.paddingLeft) || 0)) / (vt.cw || 8)) + 1;
    const y = Math.floor((e.clientY - r.top - (parseFloat(cs.paddingTop) || 0)) / (vt.lh || 19)) + 1;
    return [Math.max(1, Math.min(vt.cols, x)), Math.max(1, Math.min(vt.rows, y))];
  };
  const report = (b, x, y, release, e) => {
    b |= (e.shiftKey ? 4 : 0) | (e.altKey ? 8 : 0) | (e.ctrlKey ? 16 : 0);
    if (vt.mouseSgr) feed(`\x1b[<${b};${x};${y}${release ? "m" : "M"}`);
    else feed("\x1b[M" + String.fromCharCode(32 + (release ? 3 : b), 32 + Math.min(x, 223), 32 + Math.min(y, 223)));
  };
  const on = () => isCur() && vt.active && vt.mouse > 0 && running;
  altEl.addEventListener("mousedown", (e) => { if (!on() || e.button > 2) return; e.preventDefault(); down = e.button; const [x, y] = cell(e); report(e.button === 1 ? 1 : e.button === 2 ? 2 : 0, x, y, false, e); });
  window.addEventListener("mouseup", (e) => { if (down < 0 || !on()) { down = -1; return; } const [x, y] = cell(e); report(down === 1 ? 1 : down === 2 ? 2 : 0, x, y, true, e); down = -1; });
  window.addEventListener("mousemove", (e) => {
    if (!on() || vt.mouseMotion === 0 || (vt.mouseMotion === 1 && down < 0)) return;
    const [x, y] = cell(e), key = x + "," + y; if (key === last) return; last = key;
    report(32 + (down < 0 ? 3 : down === 1 ? 1 : down === 2 ? 2 : 0), x, y, false, e);
  });
  altEl.addEventListener("wheel", (e) => { if (!on()) return; e.preventDefault(); const [x, y] = cell(e); report(e.deltaY < 0 ? 64 : 65, x, y, false, e); }, { passive: false });

ts.onOutput = write;
vt.cw = 8; vt.lh = 19;
self = { tt, els, vt, write, onKey, onPaste, newPrompt, showLine, renderLine, startUp: () => { newPrompt(); }, flushScroll: () => { if (pendingScroll) { pendingScroll = false; scrollDown(); } }, isDead: () => dead, isLive: () => live, input: (v) => { if (live && v) insert(v.replace(/\n/g, "")); }, root: els.root };
return self;
}

/* ---------- terminal tabs ---------- */
const tabs = { list: [], cur: null };
const termsEl = $("terms"), tabbar = $("tabs");
function makeEls(root) {
  const q = (c) => root.querySelector("." + c);
  return { root, screenEl: q("screen"), out: q("out"), lineEl: q("line"), promptEl: q("prompt"), beforeEl: q("before"), curEl: q("cursor"), afterEl: q("after"), altEl: q("alt") };
}
function buildTerm() {
  const root = document.createElement("div"); root.className = "term";
  root.innerHTML = '<div class="screen"><pre class="out"></pre><pre class="line"><span class="prompt"></span><span class="before"></span><span class="cursor"> </span><span class="after"></span></pre></div><pre class="alt"></pre>';
  termsEl.appendChild(root);
  return root;
}
function attachTab(t) {
  for (const el of [t.els.screenEl, t.els.altEl]) el.addEventListener("click", () => { if (!window.getSelection().toString()) $("sink").focus(); });
  tabs.list.push(t);
}
function renderBar() {
  tabbar.style.display = tabs.list.length > 1 ? "flex" : "none";
  tabbar.textContent = "";
  tabs.list.forEach((t, i) => {
    const b = document.createElement("button"); b.className = "tab" + (t === tabs.cur ? " on" : ""); b.textContent = (i + 1) + ": tty" + (t.tt + 1);
    b.addEventListener("click", () => switchTab(t)); tabbar.appendChild(b);
  });
  const plus = document.createElement("button"); plus.className = "tab plus"; plus.textContent = "+"; plus.addEventListener("click", newTab); tabbar.appendChild(plus);
}
function switchTab(t) {
  tabs.cur = t;
  for (const x of tabs.list) x.root.classList.toggle("on", x === t);
  renderBar(); fitCols(); t.flushScroll(); $("sink").focus();
}
function newTab() {
  if (!useBrush || !K || tabs.list.length >= 9) return;
  const tt = R.newTty();
  const t = makeTab(tt, makeEls(buildTerm()));
  attachTab(t);
  startLogin(tt);
  switchTab(t);
  t.write("MavexOS tty" + (tt + 1) + "\n\n");
  t.write(sys.readFile("/etc/motd") + "\n");
  t.newPrompt();
}
function closeTab(t) {
  const i = tabs.list.indexOf(t);
  if (i < 0 || tabs.list.length < 2) return;
  tabs.list.splice(i, 1); t.root.remove();
  switchTab(tabs.list[Math.min(i, tabs.list.length - 1)]);
}
document.addEventListener("keydown", (e) => {
  if (e.ctrlKey && e.shiftKey && !e.altKey && !e.metaKey && e.key.toLowerCase() === "c") {
    e.preventDefault();
    const t = String(window.getSelection());
    if (t) (navigator.clipboard ? navigator.clipboard.writeText(t) : Promise.reject()).catch(() => document.execCommand("copy"));
    return;
  }
  if (e.ctrlKey && e.altKey && !e.metaKey && e.key.toLowerCase() === "t") { e.preventDefault(); newTab(); return; }
  if (e.altKey && !e.ctrlKey && !e.metaKey && tabs.list.length > 1) {
    const d = /^[1-9]$/.test(e.key) ? +e.key - 1 : e.key === "PageUp" ? tabs.list.indexOf(tabs.cur) - 1 : e.key === "PageDown" ? tabs.list.indexOf(tabs.cur) + 1 : -1;
    if (d >= 0 && d < tabs.list.length) { e.preventDefault(); switchTab(tabs.list[d]); return; }
  }
  if (tabs.cur) tabs.cur.onKey(e);
});
document.addEventListener("paste", (e) => { if (tabs.cur) tabs.cur.onPaste(e); });
$("sink").addEventListener("input", (e) => { const v = e.target.value; e.target.value = ""; if (tabs.cur) tabs.cur.input(v); });

/* ---------- terminal size ---------- */
function fitCols() {
  const t = tabs.cur; if (!t) return;
  const { screenEl, out } = t.els;
  const probe = document.createElement("span");
  probe.style.cssText = "position:absolute;visibility:hidden;white-space:pre";
  probe.textContent = "x".repeat(100); out.appendChild(probe);
  const cw = probe.getBoundingClientRect().width / 100; out.removeChild(probe);
  const w = screenEl.clientWidth - 24;
  if (cw > 0 && K) K.k_tty_resize(Math.max(20, Math.floor(w / cw)));
  const lh = parseFloat(getComputedStyle(screenEl).lineHeight) || 19;
  const cols = Math.max(20, Math.floor(w / cw)), rows = Math.max(5, Math.floor((termsEl.clientHeight - 24) / lh));
  const changed = R.S.cols !== cols || R.S.rows !== rows;
  R.S.cols = cols; R.S.rows = rows;
  for (const x of tabs.list) { x.vt.cw = cw; x.vt.lh = lh; x.vt.resize(cols, rows); }
  if (changed && R.winch) R.winch();
}
window.addEventListener("resize", fitCols);


{
  const t0 = makeTab(0, makeEls($("term0")));
  attachTab(t0); tabs.cur = t0; t0.root.classList.add("on");
}
/* ---------- boot ---------- */
async function detectGpu() {
  const lim = (p) => Promise.race([p, new Promise((r) => setTimeout(() => r(null), 1500))]);
  try {
    if (navigator.gpu) {
      const a = await lim(navigator.gpu.requestAdapter());
      if (a) {
        const i = a.info || (a.requestAdapterInfo ? await lim(a.requestAdapterInfo()) : null) || {};
        const m = [i.vendor, i.description || i.architecture || i.device].filter(Boolean).join(" ");
        return "WebGPU\n" + (m || "adapter (details hidden by browser)");
      }
    }
  } catch (e) {}
  try {
    const gl = document.createElement("canvas").getContext("webgl") || document.createElement("canvas").getContext("experimental-webgl");
    if (gl) {
      const x = gl.getExtension("WEBGL_debug_renderer_info");
      return "WebGL\n" + ((x && gl.getParameter(x.UNMASKED_RENDERER_WEBGL)) || gl.getParameter(gl.RENDERER) || "renderer");
    }
  } catch (e) {}
  return "Software\nno GPU API available (CPU rendering)";
}

function sysfile(path, text) {
  const [pp, pn] = put(enc.encode(path)), [dp, dn] = put(enc.encode(text));
  try { K.k_sysfile(pp, pn, dp, dn); } finally { K.k_free(pp, pn); K.k_free(dp, dn); }
}
async function publishSysfs() {
  const ud = navigator.userAgentData;
  const ua = navigator.userAgent;
  let brand = (ud && ud.brands.filter((b) => !/Not.?A.?Brand/i.test(b.brand)).pop()) || null;
  let name = brand ? brand.brand : /Firefox\/([\d.]+)/.test(ua) ? "Firefox" : /Safari/.test(ua) ? "Safari" : "Browser";
  let ver = brand ? brand.version : (/(?:Firefox|Version)\/([\d.]+)/.exec(ua) || [])[1] || "";
  let plat = (ud && ud.platform) || navigator.platform || "unknown";
  if (ud && ud.getHighEntropyValues) { try { const h = await ud.getHighEntropyValues(["architecture", "bitness"]); if (h.architecture) plat += " " + (h.architecture === "arm" ? (h.bitness === "32" ? "armv7" : "aarch64") : h.architecture === "x86" ? (h.bitness === "64" ? "x86_64" : "i686") : h.architecture); } catch (_) {} }
  const dmi = "/sys/devices/virtual/dmi/id/";
  sysfile(dmi + "product_name", `${name}${ver ? " " + ver : ""} on ${plat}\n`);
  sysfile(dmi + "sys_vendor", `${location.protocol === "file:" ? "local file" : location.host}\n`);
  sysfile(dmi + "chassis_type", (ud && ud.mobile) || /Mobi/.test(ua) ? "30\n" : "3\n");
  const dpr = window.devicePixelRatio || 1;
  sysfile("/sys/devices/virtual/mavex/display", `${Math.round(screen.width * dpr)} ${Math.round(screen.height * dpr)} ${Math.round(dpr)} 0\n`);
  if (navigator.getBattery) {
    try {
      const b = await navigator.getBattery();
      const upd = () => {
        const base = "/sys/class/power_supply/BAT0/";
        sysfile(base + "type", "Battery\n");
        sysfile(base + "capacity", `${Math.round(b.level * 100)}\n`);
        sysfile(base + "status", b.charging ? (b.level >= 1 ? "Full\n" : "Charging\n") : "Discharging\n");
      };
      upd(); b.addEventListener("levelchange", upd); b.addEventListener("chargingchange", upd);
    } catch (e) { /* no battery info */ }
  }
}

async function boot() {
  tabs.cur.showLine(false);
  const rt = await R.boot(window.MAVEX_KERNEL_B64);
  K = rt.x;
  const coreutils = R.registerModule("coreutils", window.MAVEX_COREUTILS_B64);
  const brushMod = R.registerModule("brush", window.MAVEX_BRUSH_B64);
  const mxu = R.registerModule("mavexutils", window.MAVEX_MAVEXUTILS_B64);
  const rgMod = R.registerModule("rg", window.MAVEX_EXT_RG_B64);
  const sedMod = R.registerModule("sed", window.MAVEX_EXT_SED_B64);
  const dfMod = R.registerModule("diffutils", window.MAVEX_EXT_DIFFUTILS_B64);
  const exMods = ["awk", "kibi", "mwget", "find", "xargs", "tar"].map((n) => R.registerModule(n, window["MAVEX_EXT_" + n.toUpperCase() + "_B64"]));
  const cfMod = R.registerModule("crabfetch", window.MAVEX_EXT_CRABFETCH_B64);
  const initMod = R.registerModule("init", window.MAVEX_EXT_INIT_B64);
  write("\x1b[1;36mMavexOS\x1b[0m bootloader: loading kernel...\n");
  if (!FAST) await sleep(350);
  await coreutils; await brushMod; await mxu; await rgMod; await sedMod; await Promise.all(exMods); await dfMod; await cfMod; await initMod;
  K.kmain();
  const restored = await PERSIST.init();
  write(restored < 0 ? "\x1b[1;33m[warn]\x1b[0m storage: IndexedDB unavailable, running RAM-only\n" : restored > 0 ? `[ \x1b[1;32m OK \x1b[0m ] storage: restored ${restored} entries from IndexedDB\n` : "[ \x1b[1;32m OK \x1b[0m ] storage: IndexedDB mounted (first boot on this device)\n");
  withStr(await detectGpu(), (p, n) => K.k_set_gpu(p, n));
  await publishSysfs();
  R.install("coreutils", window.MAVEX_COREUTILS_UTILS);
  R.install("brush", ["sh", "bash"]);
  R.install("mavexutils", ["id", "whoami", "groups", "chmod", "chown", "chgrp", "stat", "ip", "uname", "hostname", "clear", "uptime", "free", "nproc", "which", "top", "htop", "wg", "wg-quick", "stty", "tty", "ps", "pgrep", "pkill", "nohup", "realpath", "readlink", "less", "more", "man", "edit"]);
  R.install("rg", []);
  R.install("sed", []);
  R.install("awk", []);
  R.install("find", []);
  R.install("xargs", []);
  R.install("tar", []);
  R.install("kibi", ["nano"]);
  R.install("mwget", ["wget"]);
  R.install("diffutils", ["diff", "cmp", "diff3", "sdiff"]);
  R.install("crabfetch", []);
  R.install("init", ["systemctl", "journalctl", "reboot", "poweroff", "halt", "shutdown"]);
  R.installSetuid("mavexutils", ["sudo", "su", "passwd"]);
  R.startInit();
  for (let i = 0; i < 60 && typeof sys.readFile("/run/mavex-init/booted") !== "string"; i++) await sleep(50);
  if (useBrush) startLogin(0);
  fitCols();
  window.mavex = sys;
  for (const l of R.K.consoleBuf.split("\n").filter(Boolean)) {
    const m = l.match(/^\[ ok \](.*)$/);
    write(m ? "[ \x1b[1;32m OK \x1b[0m ]" + m[1] + "\n" : l + "\n");
    if (!FAST) await sleep(120 + Math.random() * 180);
  }
  if (!FAST) await sleep(300);
  const ver = (R.K.consoleBuf.match(/MavexOS ([0-9.]+)/) || [0, "?"])[1];
  write("\nMavexOS " + ver + " tty1\n\nmavex login: user (automatic login)\n");
  write(sys.readFile("/etc/motd") + "\n");
  tabs.cur.newPrompt();
  $("sink").focus();
  window.mavexReady = true;
  window.dispatchEvent(new Event("mavex-ready"));
}
boot().catch((e) => { write("\n\x1b[1;31mkernel panic: " + e + "\x1b[0m\n"); throw e; });


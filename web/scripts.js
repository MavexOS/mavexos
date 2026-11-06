const $ = (id) => document.getElementById(id);
const screenEl = $("screen"), out = $("out"), lineEl = $("line");
const FAST = location.hash.includes("fast");
const sleep = (ms) => new Promise(r => setTimeout(r, ms));

/* ---------- ANSI output ---------- */
let sgr = { cls: "" };
function ansiToNodes(text) {
  const re = /\x1b\[([0-9;]*)([A-Za-z])/g;
  let last = 0, m;
  const emit = (s) => { if (!s) return; const sp = document.createElement("span"); if (sgr.cls) sp.className = sgr.cls; sp.textContent = s; out.appendChild(sp); };
  while ((m = re.exec(text))) {
    emit(text.slice(last, m.index));
    last = re.lastIndex;
    if (m[2] === "J") { out.textContent = ""; }
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
function write(text) { ansiToNodes(text); scrollDown(); }
function scrollDown() { screenEl.scrollTop = screenEl.scrollHeight; }

/* ---------- kernel ---------- */
const R = window.MavexRuntime;
let K, mem;
const enc = new TextEncoder(), dec = new TextDecoder();
const put = (bytes) => { const p = K.k_alloc(bytes.length); new Uint8Array(K.memory.buffer, p, bytes.length).set(bytes); return [p, bytes.length]; };
const withStr = (s, f) => { const [p, n] = put(enc.encode(s)); try { return f(p, n); } finally { K.k_free(p, n); } };
const staged = (n) => dec.decode(new Uint8Array(K.memory.buffer, K.k_result_ptr(), n));
const useBrush = !location.hash.includes("msh");
let brushExited = false;
const LOGIN_ENV = ["PATH=/bin", "HOME=/home/user", "USER=user", "LOGNAME=user", "HOSTNAME=mavex", "SHELL=/bin/brush", "TERM=xterm-256color", "PWD=/home/user",
  "PS1=\\[\\e[1;32m\\]\\u@mavex\\[\\e[0m\\]:\\[\\e[1;34m\\]\\w\\[\\e[0m\\]\\$ "];
function startLogin() { R.startBrush("/home/user", LOGIN_ENV); }
const sys = {
  mkdir: (path) => withStr(path, (p, n) => K.k_mkdir(p, n)),
  unlink: (path) => withStr(path, (p, n) => K.k_unlink(p, n)),
  writeFile: (path, text) => withStr(path, (p, n) => { const [d, dn] = put(enc.encode(text)); try { return K.k_write_file(p, n, d, dn); } finally { K.k_free(d, dn); } }),
  readFile: (path) => withStr(path, (p, n) => { const r = K.k_read_file(p, n); return r < 0 ? r : staged(r); }),
  readdir: (path) => withStr(path, (p, n) => { const r = K.k_readdir(p, n); return r < 0 ? r : (r === 0 ? [] : staged(r).split("\n")); }),
  shellRun: (line) => {
    if (!useBrush) return withStr(line, (p, n) => staged(K.k_shell_run(p, n)));
    return R.brushRun(line).then((r) => { brushExited = r.exited; return r.out; });
  },
  prompt: () => (useBrush ? R.brushPrompt() : staged(K.k_shell_prompt())),
  complete: (word, first) => withStr(word, (p, n) => { const r = K.k_shell_complete(p, n, first ? 1 : 0); return r === 0 ? [] : staged(r).split("\n"); }),
  exited: () => (useBrush ? brushExited : K.k_shell_exited() === 1),
  reset: () => { if (useBrush) { brushExited = false; startLogin(); } else K.k_shell_reset(); },
};

/* ---------- line editor ---------- */
let buf = "", pos = 0, history = [], hidx = 0, draft = "", promptText = "", live = false, dead = false;
const stripAnsi = (s) => s.replace(/\x1b\[[0-9;]*[A-Za-z]/g, "");
function renderPrompt() {
  const el = $("prompt"); el.textContent = "";
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
  $("before").textContent = buf.slice(0, pos);
  $("cur").textContent = pos < buf.length ? buf[pos] : " ";
  $("after").textContent = buf.slice(pos + 1);
  scrollDown();
}
function showLine(show) { lineEl.style.display = show ? "" : "none"; }
function newPrompt() { promptText = sys.prompt(); renderPrompt(); buf = ""; pos = 0; hidx = history.length; renderLine(); showLine(true); live = true; }

function commitInput(extra) {
  write(promptText); // colored prompt echo
  write(buf + (extra || "") + "\n");
}
let running = false;
const feed = (str) => { const b = enc.encode(str), [p, n] = put(b); try { K.k_tty_input(p, n); } finally { K.k_free(p, n); } R.pump(); };
const RAW_KEYS = { Enter: "\r", Backspace: "\x7f", Tab: "\t", Escape: "\x1b", ArrowUp: "\x1b[A", ArrowDown: "\x1b[B", ArrowRight: "\x1b[C", ArrowLeft: "\x1b[D", Home: "\x1b[H", End: "\x1b[F", Delete: "\x1b[3~", PageUp: "\x1b[5~", PageDown: "\x1b[6~" };
function rawKey(e) {
  const k = e.key;
  if (e.ctrlKey && !e.altKey && !e.metaKey && k.length === 1) {
    const l = k.toLowerCase();
    if (l === "c") { e.preventDefault(); R.killForeground(); return; }
    if (l >= "a" && l <= "z") { e.preventDefault(); feed(String.fromCharCode(l.charCodeAt(0) - 96)); return; }
    return;
  }
  if (e.metaKey) return;
  if (RAW_KEYS[k]) { e.preventDefault(); feed(RAW_KEYS[k]); return; }
  if (k.length === 1) { e.preventDefault(); feed(k); }
}
async function runLine(line) {
  running = true; promptText = ""; renderPrompt(); buf = ""; pos = 0; renderLine(); showLine(true); live = true;
  try { write(await sys.shellRun(line)); } finally { running = false; R.S.rawTty = false; }
}
async function enter() {
  const line = buf;
  if (running) { write(buf + "\n"); buf = ""; pos = 0; hidx = history.length; renderLine(); feed(line + "\n"); return; }
  live = false;
  if (dead) { write(promptText + "\n"); dead = false; sys.reset(); write("\x1b[0m"); newPrompt(); return; }
  commitInput();
  if (line.trim() && history[history.length - 1] !== line) history.push(line);
  if (line.trim()) await runLine(line);
  if (sys.exited()) { dead = true; write("\n[process completed - press Enter to restart]\n"); promptText = ""; buf = ""; pos = 0; renderPrompt(); renderLine(); live = true; return; }
  newPrompt();
}
function setBuf(s) { buf = s; pos = s.length; renderLine(); }
function prevWordStart() { let i = pos; while (i > 0 && buf[i - 1] === " ") i--; while (i > 0 && buf[i - 1] !== " ") i--; return i; }
function commonPrefix(a) { let p = a[0]; for (const s of a) while (!s.startsWith(p)) p = p.slice(0, -1); return p; }
function tab() {
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
function insert(s) { buf = buf.slice(0, pos) + s + buf.slice(pos); pos += s.length; renderLine(); }

document.addEventListener("keydown", (e) => {
  if (!live) return;
  if (running && R.S.rawTty) { rawKey(e); return; }
  const k = e.key;
  if (e.ctrlKey && !e.altKey && !e.metaKey) {
    const l = k.toLowerCase();
    if (l === "c" && running && !(window.getSelection().toString())) { e.preventDefault(); write(buf + "^C\n"); buf = ""; pos = 0; renderLine(); R.killForeground(); return; }
    if (l === "d" && running) { e.preventDefault(); if (!buf.length) { K.k_tty_eof(); R.pump(); } return; }
    if (l === "c" && !(window.getSelection().toString())) { e.preventDefault(); write(promptText); write(buf + "^C\n"); newPrompt(); return; }
    if (l === "c") return;
    if (l === "v") return;
    if (l === "l") { e.preventDefault(); out.textContent = ""; renderLine(); return; }
    if (l === "u") { e.preventDefault(); buf = buf.slice(pos); pos = 0; renderLine(); return; }
    if (l === "k") { e.preventDefault(); buf = buf.slice(0, pos); renderLine(); return; }
    if (l === "a") { e.preventDefault(); pos = 0; renderLine(); return; }
    if (l === "e") { e.preventDefault(); pos = buf.length; renderLine(); return; }
    if (l === "w") { e.preventDefault(); const s = prevWordStart(); buf = buf.slice(0, s) + buf.slice(pos); pos = s; renderLine(); return; }
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
});
document.addEventListener("paste", (e) => {
  if (!live) return;
  e.preventDefault();
  const text = (e.clipboardData || window.clipboardData).getData("text");
  const parts = text.replace(/\r/g, "").split("\n");
  parts.forEach((p, i) => { insert(p); if (i < parts.length - 1) enter(); });
});
screenEl.addEventListener("click", () => { if (!window.getSelection().toString()) $("sink").focus(); });
$("sink").addEventListener("input", (e) => { const v = e.target.value; e.target.value = ""; if (live && v) insert(v.replace(/\n/g, "")); });

/* ---------- terminal size ---------- */
function fitCols() {
  const probe = document.createElement("span");
  probe.style.cssText = "position:absolute;visibility:hidden;white-space:pre";
  probe.textContent = "x".repeat(100); out.appendChild(probe);
  const cw = probe.getBoundingClientRect().width / 100; out.removeChild(probe);
  const w = screenEl.clientWidth - 24;
  if (cw > 0 && K) K.k_tty_resize(Math.max(20, Math.floor(w / cw)));
}
window.addEventListener("resize", fitCols);

/* ---------- boot ---------- */
async function boot() {
  showLine(false);
  const rt = await R.boot(window.MAVEX_KERNEL_B64);
  K = rt.x;
  R.S.onOutput = write;
  const coreutils = R.registerModule("coreutils", window.MAVEX_COREUTILS_B64);
  const brushMod = R.registerModule("brush", window.MAVEX_BRUSH_B64);
  const mxu = R.registerModule("mavexutils", window.MAVEX_MAVEXUTILS_B64);
  const rgMod = R.registerModule("rg", window.MAVEX_EXT_RG_B64);
  const cfMod = R.registerModule("crabfetch", window.MAVEX_EXT_CRABFETCH_B64);
  write("\x1b[1;36mMavexOS\x1b[0m bootloader: loading kernel...\n");
  if (!FAST) await sleep(350);
  await coreutils; await brushMod; await mxu; await rgMod; await cfMod;
  K.kmain();
  R.install("coreutils", window.MAVEX_COREUTILS_UTILS);
  R.install("brush", ["sh", "bash"]);
  R.install("mavexutils", ["id", "whoami", "groups", "chmod", "chown", "chgrp", "stat", "ip", "uname", "hostname", "clear", "uptime", "free", "nproc", "which"]);
  R.install("rg", []);
  R.install("crabfetch", ["crab-fetch"]);
  R.installSetuid("mavexutils", ["sudo", "su"]);
  if (useBrush) startLogin();
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
  newPrompt();
  $("sink").focus();
  window.mavexReady = true;
  window.dispatchEvent(new Event("mavex-ready"));
}
boot().catch((e) => { write("\n\x1b[1;31mkernel panic: " + e + "\x1b[0m\n"); throw e; });

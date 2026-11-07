/* Alternate-screen terminal: a cell grid with a VT100/xterm subset, used while a full-screen program (top, htop, ...) runs. */
(() => {
  const PAL16 = ["#4c566a", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#c8d3e0",
    "#6b7689", "#ff7b86", "#b5e890", "#f0d08c", "#7cc0ff", "#d68fee", "#6fd0dc", "#ffffff"];
  const color = (n) => {
    if (typeof n === "string") return n;
    if (n < 16) return PAL16[n];
    if (n < 232) { n -= 16; const c = (v) => (v ? 55 + v * 40 : 0); return `rgb(${c(Math.floor(n / 36))},${c(Math.floor(n / 6) % 6)},${c(n % 6)})`; }
    const g = 8 + (n - 232) * 10; return `rgb(${g},${g},${g})`;
  };
  const DEF = { fg: null, bg: null, b: 0, d: 0, u: 0, r: 0 };
  const blank = (a) => ({ ch: " ", a });
  // wcwidth-style: 0 for combining marks, 2 for East Asian wide / emoji, else 1.
  const cw = (cp) => {
    if (cp < 0x300) return 1;
    if ((cp >= 0x300 && cp <= 0x36f) || (cp >= 0x1ab0 && cp <= 0x1aff) || (cp >= 0x1dc0 && cp <= 0x1dff) || (cp >= 0x20d0 && cp <= 0x20ff) || (cp >= 0xfe00 && cp <= 0xfe0f) || (cp >= 0xfe20 && cp <= 0xfe2f) || cp === 0x200b || cp === 0x200d) return 0;
    if ((cp >= 0x1100 && cp <= 0x115f) || (cp >= 0x2e80 && cp <= 0xa4cf && cp !== 0x303f) || (cp >= 0xac00 && cp <= 0xd7a3) || (cp >= 0xf900 && cp <= 0xfaff) || (cp >= 0xfe30 && cp <= 0xfe6f) || (cp >= 0xff00 && cp <= 0xff60) || (cp >= 0xffe0 && cp <= 0xffe6) || (cp >= 0x1f300 && cp <= 0x1f64f) || (cp >= 0x1f900 && cp <= 0x1f9ff) || (cp >= 0x20000 && cp <= 0x3fffd)) return 2;
    return 1;
  };
  const MODES = /\x1b\[\?([0-9;]+)([hl])/g;
  const ENTER = /\x1b\[\?(1049|1047|47)h/, LEAVE = /\x1b\[\?(1049|1047|47)l/;

  const vt = {
    active: false, rows: 24, cols: 80, g: [], cx: 0, cy: 0, a: DEF, saved: null, top: 0, bot: 23, curVis: true, state: 0, buf: "", dirty: false, onReply: null, el: null, esc: "", mouse: 0, mouseSgr: false, mouseMotion: 0, paste: false, appCursor: false,
    resetModes() { this.mouse = 0; this.mouseSgr = false; this.mouseMotion = 0; this.paste = false; this.appCursor = false; },
    setMode(list, on) {
      for (const m of list.split(";")) {
        const n = +m;
        if (n === 1000 || n === 1002 || n === 1003) { this.mouse = on ? n : 0; this.mouseMotion = on ? (n === 1000 ? 0 : n === 1002 ? 1 : 2) : 0; }
        else if (n === 1006) this.mouseSgr = on;
        else if (n === 2004) this.paste = on;
        else if (n === 1) this.appCursor = on;
      }
    },
    init(el) { this.el = el; },
    resize(cols, rows) {
      if (cols === this.cols && rows === this.rows && this.g.length) return;
      const g = [];
      for (let y = 0; y < rows; y++) { const row = []; for (let x = 0; x < cols; x++) row.push((this.g[y] && this.g[y][x]) || blank(DEF)); g.push(row); }
      this.g = g; this.rows = rows; this.cols = cols; this.top = 0; this.bot = rows - 1;
      this.cx = Math.min(this.cx, cols - 1); this.cy = Math.min(this.cy, rows - 1); this.paint();
    },
    enter() {
      this.active = true; this.g = []; this.cx = this.cy = 0; this.a = DEF; this.top = 0; this.bot = this.rows - 1; this.curVis = true; this.state = 0; this.esc = "";
      const rows = this.rows, cols = this.cols; this.rows = this.cols = 0; this.resize(cols, rows);
      this.el.style.display = "block"; document.getElementById("screen").style.display = "none";
    },
    leave() {
      if (!this.active) return;
      this.active = false; this.el.style.display = "none"; document.getElementById("screen").style.display = "";
    },
    /* Splits `text` into the part for the normal scrollback and the part consumed by the grid. Returns the normal part. */
    process(text) {
      text.replace(MODES, (_, l, f) => { this.setMode(l, f === "h"); return ""; });
      let normal = "";
      while (text) {
        if (!this.active) {
          const m = ENTER.exec(text);
          if (!m) { normal += text; break; }
          normal += text.slice(0, m.index); text = text.slice(m.index + m[0].length); this.enter();
        } else {
          const m = LEAVE.exec(text);
          if (!m) { this.put(text); break; }
          this.put(text.slice(0, m.index)); text = text.slice(m.index + m[0].length); this.leave();
        }
      }
      return normal;
    },
    scroll(n, top = this.top, bot = this.bot) {
      for (let i = 0; i < Math.abs(n); i++) {
        const row = []; for (let x = 0; x < this.cols; x++) row.push(blank(this.a));
        if (n > 0) { this.g.splice(top, 1); this.g.splice(bot, 0, row); } else { this.g.splice(bot, 1); this.g.splice(top, 0, row); }
      }
    },
    lf() { if (this.cy === this.bot) this.scroll(1); else if (this.cy < this.rows - 1) this.cy++; },
    put(text) {
      for (const ch of text) {
        if (this.state === 0) {
          if (ch === "\x1b") { this.state = 1; this.esc = ""; continue; }
          if (ch === "\n") { this.lf(); this.cx = 0; continue; }
          if (ch === "\r") { this.cx = 0; continue; }
          if (ch === "\b") { if (this.cx > 0) this.cx--; continue; }
          if (ch === "\t") { this.cx = Math.min(this.cols - 1, (this.cx + 8) & ~7); continue; }
          if (ch < " " || ch === "\x7f") continue;
          const w = cw(ch.codePointAt(0));
          if (w === 0) { const px = this.cx > 0 ? this.cx - 1 : 0; const pc = this.g[this.cy][px]; if (pc && pc.ch) this.g[this.cy][px] = { ch: pc.ch + ch, a: pc.a, w: pc.w }; continue; }
          if (this.cx + w > this.cols) { this.cx = 0; this.lf(); }
          this.g[this.cy][this.cx++] = { ch, a: this.a, w };
          if (w === 2) this.g[this.cy][this.cx++] = { ch: "", a: this.a, w: 0 };
        } else if (this.state === 1) { // after ESC
          if (ch === "[") { this.state = 2; this.esc = ""; }
          else if (ch === "]") { this.state = 3; this.esc = ""; }
          else if (ch === "7") { this.saved = [this.cx, this.cy, this.a]; this.state = 0; }
          else if (ch === "8") { if (this.saved) [this.cx, this.cy, this.a] = this.saved; this.state = 0; }
          else if (ch === "M") { if (this.cy === this.top) this.scroll(-1); else if (this.cy > 0) this.cy--; this.state = 0; }
          else if (ch === "c") { this.a = DEF; this.clear(2); this.cx = this.cy = 0; this.state = 0; }
          else if (ch === "(" || ch === ")") this.state = 4;
          else this.state = 0;
        } else if (this.state === 2) { // CSI
          if (ch >= "@" && ch <= "~") { this.csi(this.esc, ch); this.state = 0; } else this.esc += ch;
        } else if (this.state === 3) { // OSC: swallow until BEL or ST
          if (ch === "\x07") this.state = 0;
          else if (ch === "\x1b") this.state = 1;
        } else this.state = 0; // charset designation byte
      }
      this.paint();
    },
    clear(mode) { // ED
      const e = (y, x0, x1) => { for (let x = x0; x < x1; x++) this.g[y][x] = blank(this.a); };
      if (mode === 0) { e(this.cy, this.cx, this.cols); for (let y = this.cy + 1; y < this.rows; y++) e(y, 0, this.cols); }
      else if (mode === 1) { for (let y = 0; y < this.cy; y++) e(y, 0, this.cols); e(this.cy, 0, this.cx + 1); }
      else for (let y = 0; y < this.rows; y++) e(y, 0, this.cols);
    },
    csi(raw, f) {
      const priv = raw.startsWith("?");
      const p = raw.replace(/^[?>=]/, "").split(";").map((v) => (v === "" ? 0 : parseInt(v, 10) || 0));
      const n = (i, d = 1) => (p[i] ? p[i] : d);
      const clampX = () => { this.cx = Math.max(0, Math.min(this.cols - 1, this.cx)); };
      switch (f) {
        case "H": case "f": this.cy = Math.min(this.rows, n(0)) - 1; this.cx = Math.min(this.cols, n(1)) - 1; break;
        case "A": this.cy = Math.max(this.top, this.cy - n(0)); break;
        case "B": this.cy = Math.min(this.bot, this.cy + n(0)); break;
        case "C": this.cx = Math.min(this.cols - 1, this.cx + n(0)); break;
        case "D": this.cx = Math.max(0, this.cx - n(0)); break;
        case "E": this.cy = Math.min(this.bot, this.cy + n(0)); this.cx = 0; break;
        case "F": this.cy = Math.max(this.top, this.cy - n(0)); this.cx = 0; break;
        case "G": this.cx = Math.min(this.cols, n(0)) - 1; break;
        case "d": this.cy = Math.min(this.rows, n(0)) - 1; break;
        case "J": this.clear(p[0] || 0); break;
        case "K": { clampX(); const m = p[0] || 0, y = this.cy; const [a, b] = m === 0 ? [this.cx, this.cols] : m === 1 ? [0, this.cx + 1] : [0, this.cols]; for (let x = a; x < b; x++) this.g[y][x] = blank(this.a); break; }
        case "L": if (this.cy >= this.top && this.cy <= this.bot) this.scroll(-n(0), this.cy, this.bot); break;
        case "M": if (this.cy >= this.top && this.cy <= this.bot) this.scroll(n(0), this.cy, this.bot); break;
        case "S": this.scroll(n(0)); break;
        case "T": this.scroll(-n(0)); break;
        case "@": { clampX(); const r = this.g[this.cy]; for (let i = 0; i < n(0); i++) { r.splice(this.cx, 0, blank(this.a)); r.pop(); } break; }
        case "P": { clampX(); const r = this.g[this.cy]; for (let i = 0; i < n(0); i++) { r.splice(this.cx, 1); r.push(blank(this.a)); } break; }
        case "X": { clampX(); for (let i = 0; i < n(0) && this.cx + i < this.cols; i++) this.g[this.cy][this.cx + i] = blank(this.a); break; }
        case "r": this.top = Math.max(0, n(0) - 1); this.bot = Math.min(this.rows - 1, (p[1] || this.rows) - 1); this.cx = 0; this.cy = this.top; break;
        case "s": this.saved = [this.cx, this.cy, this.a]; break;
        case "u": if (this.saved) [this.cx, this.cy, this.a] = this.saved; break;
        case "h": case "l": if (priv && p.includes(25)) this.curVis = f === "h"; break;
        case "n": if (p[0] === 6 && this.onReply) this.onReply(`\x1b[${this.cy + 1};${this.cx + 1}R`); break;
        case "m": this.sgr(p.length ? p : [0]); break;
      }
    },
    sgr(p) {
      const a = { ...this.a };
      for (let i = 0; i < p.length; i++) {
        const c = p[i];
        if (c === 0) Object.assign(a, DEF);
        else if (c === 1) a.b = 1; else if (c === 2) a.d = 1; else if (c === 4) a.u = 1; else if (c === 7) a.r = 1;
        else if (c === 22) { a.b = 0; a.d = 0; } else if (c === 24) a.u = 0; else if (c === 27) a.r = 0;
        else if (c >= 30 && c <= 37) a.fg = c - 30; else if (c === 39) a.fg = null;
        else if (c >= 40 && c <= 47) a.bg = c - 40; else if (c === 49) a.bg = null;
        else if (c >= 90 && c <= 97) a.fg = c - 90 + 8; else if (c >= 100 && c <= 107) a.bg = c - 100 + 8;
        else if (c === 38 || c === 48) {
          let v = null;
          if (p[i + 1] === 5) { v = p[i + 2]; i += 2; }
          else if (p[i + 1] === 2) { v = `rgb(${p[i + 2]},${p[i + 3]},${p[i + 4]})`; i += 4; }
          if (v !== null) { if (c === 38) a.fg = v; else a.bg = v; }
        }
      }
      this.a = a;
    },
    paint() {
      if (this.dirty) return;
      this.dirty = true;
      requestAnimationFrame(() => { this.dirty = false; this.render(); });
    },
    render() {
      if (!this.active) return;
      const esc = (s) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;");
      const style = (a, cursor) => {
        let fg = a.fg === null ? null : color(a.fg), bg = a.bg === null ? null : color(a.bg);
        if (a.r !== 0 !== cursor) { const t = fg; fg = bg || "#0b0e14"; bg = t || "#c8d3e0"; }
        return (fg ? `color:${fg};` : "") + (bg ? `background:${bg};` : "") + (a.b ? "font-weight:bold;" : "") + (a.d ? "opacity:.65;" : "") + (a.u ? "text-decoration:underline;" : "");
      };
      let html = "";
      for (let y = 0; y < this.rows; y++) {
        const row = this.g[y]; let run = "", rs = null;
        const flush = () => { if (run) html += rs ? `<span style="${rs}">${esc(run)}</span>` : esc(run); run = ""; };
        for (let x = 0; x < this.cols; x++) {
          const c = row[x], cur = this.curVis && y === this.cy && x === Math.min(this.cx, this.cols - 1);
          const s = style(c.a, cur);
          if (s !== rs) { flush(); rs = s; }
          run += c.ch;
        }
        flush(); html += "\n";
      }
      this.el.innerHTML = html;
    },
  };
  window.MavexVT = vt;
})();

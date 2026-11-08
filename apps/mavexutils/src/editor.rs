//! `edit`: a small nano-style terminal editor (MavexOS' own; not ox).
use crate::top::{size, split_keys, wait_stdin};
use std::io::{Read, Write};

#[link(wasm_import_module = "mavex")]
unsafe extern "C" {
    fn tty_raw(on: i32) -> i32;
}

struct Ed { lines: Vec<Vec<char>>, cy: usize, cx: usize, top: usize, left: usize, path: String, dirty: bool, msg: String, quit_armed: bool, find: String, prompt: Option<(String, String)>, clip: Option<Vec<char>> }

impl Ed {
    fn line_len(&self) -> usize { self.lines[self.cy].len() }
    fn clamp(&mut self) { self.cx = self.cx.min(self.line_len()); }
    fn save(&mut self) {
        let mut s = String::new();
        for (i, l) in self.lines.iter().enumerate() { if i > 0 { s.push('\n'); } s.extend(l.iter()); }
        if !(self.lines.len() == 1 && self.lines[0].is_empty()) { s.push('\n'); }
        match std::fs::write(&self.path, s) { Ok(_) => { self.dirty = false; self.msg = format!("Wrote {} lines to {}", self.lines.len(), self.path); } Err(e) => self.msg = format!("Error writing {}: {}", self.path, e.to_string().split(" (os").next().unwrap_or("")) }
    }
    fn do_find(&mut self) {
        if self.find.is_empty() { return; }
        let pat: Vec<char> = self.find.chars().collect();
        let n = self.lines.len();
        for off in 0..=n {
            let y = (self.cy + off) % n;
            let start = if off == 0 { self.cx + 1 } else { 0 };
            let l = &self.lines[y];
            if l.len() >= pat.len() { for x in start..=l.len() - pat.len() { if l[x..x + pat.len()] == pat[..] { self.cy = y; self.cx = x; return; } } }
        }
        self.msg = format!("\"{}\" not found", self.find);
    }
    fn scroll(&mut self, cols: usize, rows: usize) {
        if self.cy < self.top { self.top = self.cy; }
        if self.cy >= self.top + rows { self.top = self.cy + 1 - rows; }
        if self.cx < self.left { self.left = self.cx; }
        if self.cx >= self.left + cols { self.left = self.cx + 1 - cols; }
    }
    fn render(&mut self, cols: usize, rows: usize) -> String {
        let body = rows.saturating_sub(2);
        self.scroll(cols, body);
        let mut f = String::from("\x1b[?25l\x1b[H");
        f.push_str(&format!("\x1b[7m{:<w$}\x1b[27m\r\n", format!(" edit  {}{}", if self.path.is_empty() { "(new)" } else { &self.path }, if self.dirty { " [modified]" } else { "" }), w = cols));
        for i in 0..body {
            if let Some(l) = self.lines.get(self.top + i) { let s: String = l.iter().skip(self.left).take(cols).collect(); f.push_str(&s.replace('\t', " ")); }
            f.push_str("\x1b[K\r\n");
        }
        let status = match &self.prompt { Some((q, a)) => format!("{q}{a}"), None if !self.msg.is_empty() => self.msg.clone(), None => "^S Save  ^Q Quit  ^F Find  ^K Cut line  ^U Paste".into() };
        f.push_str(&format!("\x1b[7m{:<w$}\x1b[27m", status.chars().take(cols).collect::<String>(), w = cols));
        match &self.prompt {
            Some((q, a)) => f.push_str(&format!("\x1b[{};{}H", rows, q.len() + a.len() + 1)),
            None => f.push_str(&format!("\x1b[{};{}H", self.cy - self.top + 2, self.cx - self.left + 1)),
        }
        f.push_str("\x1b[?25h");
        f
    }
    fn key(&mut self, k: &[u8], rows: usize) -> bool {
        if let Some((_, a)) = self.prompt.as_mut() {
            match k {
                b"\r" | b"\n" => { let (_, a) = self.prompt.take().unwrap(); self.find = a; self.do_find(); }
                b"\x1b" | b"\x03" | b"\x07" => self.prompt = None,
                b"\x7f" => { a.pop(); }
                c if c.len() == 1 && c[0] >= 32 => a.push(c[0] as char),
                c if c.first().is_some_and(|b| *b >= 0xc0) => a.push_str(&String::from_utf8_lossy(c)),
                _ => {}
            }
            return true;
        }
        self.msg.clear();
        if k != b"\x11" { self.quit_armed = false; }
        match k {
            b"\x11" => { if self.dirty && !self.quit_armed { self.quit_armed = true; self.msg = "Unsaved changes: ^Q again to discard, ^S to save".into(); } else { return false; } }
            b"\x13" => { if self.path.is_empty() { self.msg = "No file name".into(); } else { self.save(); } }
            b"\x06" => self.prompt = Some(("Find: ".into(), String::new())),
            b"\x0b" => { self.clip = Some(self.lines.remove(self.cy)); if self.lines.is_empty() { self.lines.push(vec![]); } self.cy = self.cy.min(self.lines.len() - 1); self.clamp(); self.dirty = true; }
            b"\x15" => { if let Some(c) = self.clip.clone() { self.lines.insert(self.cy, c); self.cy += 1; self.dirty = true; } }
            b"\x1b[A" => { self.cy = self.cy.saturating_sub(1); self.clamp(); }
            b"\x1b[B" => { if self.cy + 1 < self.lines.len() { self.cy += 1; } self.clamp(); }
            b"\x1b[D" => { if self.cx > 0 { self.cx -= 1; } else if self.cy > 0 { self.cy -= 1; self.cx = self.line_len(); } }
            b"\x1b[C" => { if self.cx < self.line_len() { self.cx += 1; } else if self.cy + 1 < self.lines.len() { self.cy += 1; self.cx = 0; } }
            b"\x1b[H" | b"\x01" => self.cx = 0,
            b"\x1b[F" | b"\x05" => self.cx = self.line_len(),
            b"\x1b[5~" => { self.cy = self.cy.saturating_sub(rows.saturating_sub(3)); self.clamp(); }
            b"\x1b[6~" => { self.cy = (self.cy + rows.saturating_sub(3)).min(self.lines.len() - 1); self.clamp(); }
            b"\x1b[3~" => {
                if self.cx < self.line_len() { self.lines[self.cy].remove(self.cx); self.dirty = true; }
                else if self.cy + 1 < self.lines.len() { let n = self.lines.remove(self.cy + 1); self.lines[self.cy].extend(n); self.dirty = true; }
            }
            b"\x7f" | b"\x08" => {
                if self.cx > 0 { self.cx -= 1; self.lines[self.cy].remove(self.cx); self.dirty = true; }
                else if self.cy > 0 { let l = self.lines.remove(self.cy); self.cy -= 1; self.cx = self.line_len(); self.lines[self.cy].extend(l); self.dirty = true; }
            }
            b"\r" | b"\n" => { let rest = self.lines[self.cy].split_off(self.cx); self.cy += 1; self.lines.insert(self.cy, rest); self.cx = 0; self.dirty = true; }
            b"\t" => { for _ in 0..4 { self.lines[self.cy].insert(self.cx, ' '); self.cx += 1; } self.dirty = true; }
            c if !c.starts_with(b"\x1b") && c.iter().all(|b| *b >= 32 || *b >= 0x80) => {
                for ch in String::from_utf8_lossy(c).chars() { self.lines[self.cy].insert(self.cx, ch); self.cx += 1; }
                self.dirty = true;
            }
            _ => {}
        }
        true
    }
}

pub fn edit(args: &[String]) -> i32 {
    let path = args.iter().find(|a| !a.starts_with('-') && !a.starts_with('+')).cloned().unwrap_or_default();
    let mut lines: Vec<Vec<char>> = Vec::new();
    let mut msg = String::new();
    if !path.is_empty() {
        match std::fs::read(&path) {
            Ok(b) => { let t = String::from_utf8_lossy(&b); lines = t.lines().map(|l| l.chars().collect()).collect(); }
            Err(_) => msg = "New file".into(),
        }
    }
    if lines.is_empty() { lines.push(vec![]); }
    use std::io::IsTerminal;
    if !std::io::stdout().is_terminal() { eprintln!("edit: stdout is not a terminal"); return 1; }
    let mut ed = Ed { lines, cy: 0, cx: 0, top: 0, left: 0, path, dirty: false, msg, quit_armed: false, find: String::new(), prompt: None, clip: None };
    unsafe { tty_raw(1) };
    let mut so = std::io::stdout();
    let _ = write!(so, "\x1b[?1049h");
    let mut stdin = std::io::stdin();
    'main: loop {
        let (cols, rows) = size();
        let f = ed.render(cols, rows);
        let _ = so.write_all(f.as_bytes());
        let _ = so.flush();
        while !wait_stdin(1000) {}
        let mut b = [0u8; 256];
        let n = match stdin.read(&mut b) { Ok(0) | Err(_) => break, Ok(n) => n };
        for k in split_keys(&b[..n]) { if !ed.key(&k, rows) { break 'main; } }
    }
    let _ = write!(so, "\x1b[?25h\x1b[?1049l");
    let _ = so.flush();
    unsafe { tty_raw(0) };
    0
}

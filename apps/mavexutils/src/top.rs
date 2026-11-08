//! `top` and `htop`: interactive process viewers over /proc (MavexOS' own implementations, not procps/htop).
use std::collections::HashMap;
use std::io::{Read, Write};
use std::time::{Duration, Instant};

#[link(wasm_import_module = "mavex")]
unsafe extern "C" {
    fn tty_raw(on: i32) -> i32;
    fn tty_size(out: *mut u32) -> i32;
    fn kill(pid: u32, sig: i32) -> i32;
}
#[link(wasm_import_module = "wasi_snapshot_preview1")]
unsafe extern "C" {
    fn poll_oneoff(inp: *const u8, out: *mut u8, nsubs: usize, nevents: *mut usize) -> u16;
}

pub fn size() -> (usize, usize) {
    let mut o = [0u32; 2];
    unsafe { tty_size(o.as_mut_ptr()) };
    (o[0].max(20) as usize, o[1].max(5) as usize)
}

/// Waits until stdin is readable or `ms` passes; true when input is ready.
pub fn wait_stdin(ms: u64) -> bool {
    let mut subs = [0u8; 96];
    subs[8] = 1; // fd_read on fd 0
    subs[48 + 8] = 0; // clock
    subs[48..56].copy_from_slice(&1u64.to_le_bytes());
    subs[48 + 16..48 + 20].copy_from_slice(&1u32.to_le_bytes());
    subs[48 + 24..48 + 32].copy_from_slice(&(ms * 1_000_000).to_le_bytes());
    let mut out = [0u8; 64];
    let mut n = 0usize;
    let r = unsafe { poll_oneoff(subs.as_ptr(), out.as_mut_ptr(), 2, &mut n) };
    if r != 0 {
        std::thread::sleep(Duration::from_millis(ms));
        return false;
    }
    (0..n).any(|k| u64::from_le_bytes(out[k * 32..k * 32 + 8].try_into().unwrap()) == 0)
}

#[derive(Clone)]
struct P {
    pid: u32,
    ppid: u32,
    user: String,
    state: char,
    ticks: u64,
    vsz_kib: u64,
    rss_kib: u64,
    cpu: f64,
    cmd: String,
    comm: String,
}

struct Mem { total: u64, free: u64 }

fn users() -> HashMap<u32, String> {
    std::fs::read_to_string("/etc/passwd").unwrap_or_default().lines().filter_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        Some((f.get(2)?.parse().ok()?, f[0].to_string()))
    }).collect()
}

fn read_procs(us: &HashMap<u32, String>) -> Vec<P> {
    let mut v = Vec::new();
    let Ok(rd) = std::fs::read_dir("/proc") else { return v };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Ok(pid) = name.parse::<u32>() else { continue };
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else { continue };
        let (Some(a), Some(b)) = (stat.find('('), stat.rfind(')')) else { continue };
        let comm = stat[a + 1..b].to_string();
        let f: Vec<&str> = stat[b + 1..].split_whitespace().collect();
        if f.len() < 22 { continue; }
        let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
        let uid = status.lines().find_map(|l| l.strip_prefix("Uid:")).and_then(|r| r.split_whitespace().next()?.parse().ok()).unwrap_or(0u32);
        let cmd = std::fs::read(format!("/proc/{pid}/cmdline")).map(|b| String::from_utf8_lossy(&b).split('\0').filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ")).unwrap_or_default();
        v.push(P {
            pid, ppid: f[1].parse().unwrap_or(0), user: us.get(&uid).cloned().unwrap_or_else(|| uid.to_string()),
            state: f[0].chars().next().unwrap_or('?'), ticks: f[11].parse().unwrap_or(0),
            vsz_kib: f[20].parse::<u64>().unwrap_or(0) / 1024, rss_kib: f[21].parse::<u64>().unwrap_or(0) * 4,
            cpu: 0.0, cmd: if cmd.is_empty() { comm.clone() } else { cmd }, comm,
        });
    }
    v
}

fn meminfo() -> Mem {
    let s = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let get = |k: &str| s.lines().find_map(|l| l.strip_prefix(k)?.trim_start_matches(':').split_whitespace().next()?.parse::<u64>().ok()).unwrap_or(0);
    Mem { total: get("MemTotal"), free: get("MemAvailable") }
}

fn cpu_times() -> (u64, u64) {
    let s = std::fs::read_to_string("/proc/stat").unwrap_or_default();
    let f: Vec<u64> = s.lines().next().unwrap_or("").split_whitespace().skip(1).filter_map(|x| x.parse().ok()).collect();
    (f.first().copied().unwrap_or(0), f.get(3).copied().unwrap_or(0))
}

fn uptime() -> f64 {
    std::fs::read_to_string("/proc/uptime").ok().and_then(|s| s.split_whitespace().next()?.parse().ok()).unwrap_or(0.0)
}

fn loadavg() -> String {
    let s = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
    s.split_whitespace().take(3).collect::<Vec<_>>().join(" ")
}

fn kib(v: u64) -> String {
    if v < 100_000 { v.to_string() } else if v < 100 * 1024 * 1024 { format!("{:.1}m", v as f64 / 1024.0) } else { format!("{:.1}g", v as f64 / 1048576.0) }
}

fn dur_hms(secs: u64) -> String {
    let (d, h, m) = (secs / 86400, secs / 3600 % 24, secs / 60 % 60);
    if d > 0 { format!("{d} days, {h}:{m:02}") } else if h > 0 { format!("{h}:{m:02}") } else { format!("{m} min") }
}

fn clock() -> String {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    format!("{:02}:{:02}:{:02}", t / 3600 % 24, t / 60 % 60, t % 60)
}

fn cut(s: &str, w: usize) -> String { s.chars().take(w).collect() }

#[derive(Clone, Copy, PartialEq)]
enum Sort { Cpu, Mem, Time, Pid }

struct App {
    htop: bool,
    sort: Sort,
    rev: bool,
    tree: bool,
    sel: usize,
    scroll: usize,
    procs: Vec<P>,
    prev: HashMap<u32, u64>,
    prev_cpu: (u64, u64),
    cpu_pct: f64,
    mem: Mem,
    msg: String,
    prompt: Option<String>,
}

impl App {
    fn sample(&mut self, dt: f64, us: &HashMap<u32, String>) {
        let mut v = read_procs(us);
        for p in &mut v {
            if let Some(o) = self.prev.get(&p.pid) { p.cpu = (p.ticks.saturating_sub(*o)) as f64 / dt.max(0.001); }
        }
        self.prev = v.iter().map(|p| (p.pid, p.ticks)).collect();
        let c = cpu_times();
        let (db, di) = (c.0.saturating_sub(self.prev_cpu.0), c.1.saturating_sub(self.prev_cpu.1));
        self.cpu_pct = if db + di > 0 { db as f64 * 100.0 / (db + di) as f64 } else { 0.0 };
        self.prev_cpu = c;
        self.mem = meminfo();
        self.procs = v;
        self.order();
        if self.sel >= self.procs.len() { self.sel = self.procs.len().saturating_sub(1); }
    }

    fn order(&mut self) {
        let key = self.sort;
        let rev = self.rev;
        self.procs.sort_by(|a, b| {
            let o = match key {
                Sort::Cpu => b.cpu.partial_cmp(&a.cpu).unwrap_or(std::cmp::Ordering::Equal),
                Sort::Mem => b.rss_kib.cmp(&a.rss_kib),
                Sort::Time => b.ticks.cmp(&a.ticks),
                Sort::Pid => a.pid.cmp(&b.pid),
            }
            .then(a.pid.cmp(&b.pid));
            if rev { o.reverse() } else { o }
        });
        if self.tree {
            let pids: Vec<u32> = self.procs.iter().map(|p| p.pid).collect();
            let mut out: Vec<P> = Vec::new();
            fn walk(all: &[P], parent: u32, depth: usize, out: &mut Vec<P>, seen: &mut Vec<u32>) {
                for p in all.iter().filter(|p| p.ppid == parent && p.pid != parent) {
                    if seen.contains(&p.pid) { continue; }
                    seen.push(p.pid);
                    let mut q = p.clone();
                    if depth > 0 { q.cmd = format!("{}└─ {}", "   ".repeat(depth - 1), q.cmd); }
                    out.push(q);
                    walk(all, p.pid, depth + 1, out, seen);
                }
            }
            let mut seen = Vec::new();
            let roots: Vec<u32> = self.procs.iter().filter(|p| !pids.contains(&p.ppid) || p.ppid == p.pid).map(|p| p.ppid).collect();
            let mut done = Vec::new();
            for r in roots { if !done.contains(&r) { done.push(r); walk(&self.procs, r, 0, &mut out, &mut seen); } }
            for p in &self.procs { if !seen.contains(&p.pid) { out.push(p.clone()); } }
            self.procs = out;
        }
    }

    fn counts(&self) -> (usize, usize, usize, usize) {
        let run = self.procs.iter().filter(|p| p.state == 'R').count();
        let zom = self.procs.iter().filter(|p| p.state == 'Z').count();
        let stop = self.procs.iter().filter(|p| p.state == 'T').count();
        (self.procs.len(), run, self.procs.len() - run - zom - stop, zom)
    }

    fn header_top(&self, cols: usize) -> Vec<String> {
        let (tot, run, slp, zom) = self.counts();
        let m = &self.mem;
        let us = self.cpu_pct;
        let mib = |k: u64| k as f64 / 1024.0;
        let mut v = vec![
            format!("top - {} up {},  1 user,  load average: {}", clock(), dur_hms(uptime() as u64), loadavg().replace(' ', ", ")),
            format!("Tasks: {tot:>4} total, {run:>3} running, {slp:>3} sleeping,   0 stopped, {zom:>3} zombie"),
            format!("%Cpu(s): {us:>5.1} us, {:>5.1} sy, {:>5.1} id", 0.0, 100.0 - us),
            format!("MiB Mem : {:>8.1} total, {:>8.1} free, {:>8.1} used", mib(m.total), mib(m.free), mib(m.total - m.free)),
            format!("MiB Swap: {:>8.1} total, {:>8.1} free, {:>8.1} used.", 0.0, 0.0, 0.0),
            String::new(),
        ];
        v.iter_mut().for_each(|l| *l = cut(l, cols));
        v
    }

    fn table_top(&self, p: &P) -> String {
        let t = p.ticks;
        let mem = if self.mem.total > 0 { p.rss_kib as f64 * 100.0 / self.mem.total as f64 } else { 0.0 };
        format!("{:>7} {:<9.9} {:>3} {:>3} {:>7} {:>6} {:>6} {} {:>5.1} {:>5.1} {:>4}:{:02}.{:02} {}", p.pid, p.user, 20, 0, kib(p.vsz_kib), kib(p.rss_kib), 0, p.state, p.cpu, mem, t / 6000, t / 100 % 60, t % 100, p.cmd)
    }
    const TOP_HDR: &'static str = "    PID USER      PR  NI    VIRT    RES    SHR S  %CPU  %MEM     TIME+ COMMAND";

    fn bar(&self, label: &str, frac: f64, text: &str, width: usize, color: &str) -> String {
        let inner = width.saturating_sub(label.len() + 2);
        let filled = (frac.clamp(0.0, 1.0) * inner as f64) as usize;
        let tl = text.chars().count();
        let mut cells: Vec<char> = vec![' '; inner];
        for c in cells.iter_mut().take(filled) { *c = '|'; }
        for (i, ch) in text.chars().enumerate() { if inner >= tl { cells[inner - tl + i] = ch; } }
        let (a, b): (String, String) = (cells[..filled.min(inner.saturating_sub(tl))].iter().collect(), cells[filled.min(inner.saturating_sub(tl))..].iter().collect());
        format!("\x1b[1;36m{label}\x1b[0;1m[\x1b[0m{color}{a}\x1b[0;90m{b}\x1b[0;1m]\x1b[0m")
    }

    fn render(&mut self, cols: usize, rows: usize) -> String {
        let mut lines: Vec<String> = Vec::new();
        let hdr_rows;
        if self.htop {
            let (tot, run, _, _) = self.counts();
            let half = (cols.saturating_sub(2) / 2).max(20);
            let m = &self.mem;
            let used = m.total - m.free;
            let l1 = self.bar("CPU", self.cpu_pct / 100.0, &format!("{:.1}%", self.cpu_pct), half, "\x1b[32m");
            let l2 = self.bar("Mem", used as f64 / m.total.max(1) as f64, &format!("{:.0}M/{:.0}M", used as f64 / 1024.0, m.total as f64 / 1024.0), half, "\x1b[32m");
            let l3 = self.bar("Swp", 0.0, "0K/0K", half, "\x1b[31m");
            let r1 = format!("\x1b[1;36mTasks:\x1b[0m \x1b[1m{tot}\x1b[0m, \x1b[1;32m{run}\x1b[0m running");
            let r2 = format!("\x1b[1;36mLoad average:\x1b[0m {}", loadavg());
            let up = uptime() as u64;
            let r3 = format!("\x1b[1;36mUptime:\x1b[0m \x1b[1m{:02}:{:02}:{:02}\x1b[0m", up / 3600, up / 60 % 60, up % 60);
            for (l, r) in [(l1, r1), (l2, r2), (l3, r3)] { lines.push(format!("{l}  {r}")); }
            lines.push(String::new());
            hdr_rows = lines.len();
            let h = format!("{:>7} {:<9} {:>3} {:>3} {:>6} {:>6} {} {:>5} {:>5} {:>9}  Command", "PID", "USER", "PRI", "NI", "VIRT", "RES", "S", "CPU%", "MEM%", "TIME+");
            lines.push(format!("\x1b[38;2;11;14;20;42m{:<w$}\x1b[0m", cut(&h, cols), w = cols));
        } else {
            lines = self.header_top(cols);
            hdr_rows = lines.len();
            lines.push(format!("\x1b[7;1m{:<w$}\x1b[0m", cut(Self::TOP_HDR, cols), w = cols));
        }
        let body_rows = rows.saturating_sub(lines.len() + 1);
        if self.sel < self.scroll { self.scroll = self.sel; }
        if body_rows > 0 && self.sel >= self.scroll + body_rows { self.scroll = self.sel + 1 - body_rows; }
        for (i, p) in self.procs.iter().enumerate().skip(self.scroll).take(body_rows) {
            let text = if self.htop {
                let t = p.ticks;
                let mem = if self.mem.total > 0 { p.rss_kib as f64 * 100.0 / self.mem.total as f64 } else { 0.0 };
                format!("{:>7} {:<9.9} {:>3} {:>3} {:>6} {:>6} {} {:>5.1} {:>5.1} {:>4}:{:02}.{:02}  {}", p.pid, p.user, 20, 0, kib(p.vsz_kib), kib(p.rss_kib), p.state, p.cpu, mem, t / 6000, t / 100 % 60, t % 100, p.cmd)
            } else {
                self.table_top(p)
            };
            let t = format!("{:<w$}", cut(&text, cols), w = cols);
            if self.htop && i == self.sel { lines.push(format!("\x1b[38;2;11;14;20;46m{t}\x1b[0m")); } else { lines.push(t); }
        }
        while lines.len() < rows.saturating_sub(1) { lines.push(String::new()); }
        let foot = if let Some(p) = &self.prompt {
            format!("\x1b[1mPID to kill: {p}\x1b[0m")
        } else if !self.msg.is_empty() {
            format!("\x1b[1;33m{}\x1b[0m", self.msg)
        } else if self.htop {
            let keys = [("F1", "Help  "), ("F3", "Search"), ("F5", "Tree  "), ("F6", "SortBy"), ("F9", "Kill  "), ("F10", "Quit  ")];
            keys.iter().map(|(k, l)| format!("{k}\x1b[38;2;11;14;20;46m{l}\x1b[0m")).collect::<Vec<_>>().join("")
        } else {
            "q quit  P cpu  M mem  T time  N pid  R reverse  V tree  k kill".to_string()
        };
        lines.push(foot);
        let _ = hdr_rows;
        let mut s = String::from("\x1b[H");
        let n = lines.len().min(rows);
        for (i, l) in lines.iter().take(n).enumerate() {
            s.push_str(l);
            s.push_str("\x1b[K");
            if i + 1 < n { s.push_str("\r\n"); }
        }
        s.push_str("\x1b[J");
        s
    }

    /// Returns false to quit.
    fn key(&mut self, k: &[u8], page: usize) -> bool {
        if let Some(p) = self.prompt.as_mut() {
            match k {
                b"\r" | b"\n" => {
                    let t = self.prompt.take().unwrap_or_default();
                    match t.trim().parse::<u32>() {
                        Ok(pid) => self.msg = self.kill_pid(pid),
                        Err(_) => self.msg = "invalid pid".into(),
                    }
                }
                b"\x1b" | b"\x03" => self.prompt = None,
                b"\x7f" => { p.pop(); }
                [c] if c.is_ascii_digit() => p.push(*c as char),
                _ => {}
            }
            return true;
        }
        self.msg.clear();
        let last = self.procs.len().saturating_sub(1);
        match k {
            b"q" | b"\x1b[21~" | b"\x03" => return false,
            b"\x1b[A" => self.sel = self.sel.saturating_sub(1),
            b"\x1b[B" => self.sel = (self.sel + 1).min(last),
            b"\x1b[5~" => self.sel = self.sel.saturating_sub(page),
            b"\x1b[6~" => self.sel = (self.sel + page).min(last),
            b"\x1b[H" => self.sel = 0,
            b"\x1b[F" => self.sel = last,
            b"P" => self.sort = Sort::Cpu,
            b"M" => self.sort = Sort::Mem,
            b"T" => self.sort = Sort::Time,
            b"N" => self.sort = Sort::Pid,
            b"R" | b"I" => self.rev = !self.rev,
            b"t" | b"V" | b"\x1b[15~" => self.tree = !self.tree,
            b"\x1b[17~" => { self.sort = match self.sort { Sort::Cpu => Sort::Mem, Sort::Mem => Sort::Time, Sort::Time => Sort::Pid, Sort::Pid => Sort::Cpu }; }
            b"\x1b[20~" | b"k" => {
                if self.htop {
                    if let Some(p) = self.procs.get(self.sel) { let pid = p.pid; self.msg = self.kill_pid(pid); }
                } else { self.prompt = Some(String::new()); }
            }
            _ => {}
        }
        true
    }

    fn kill_pid(&self, pid: u32) -> String {
        if pid == 1 { return "refusing to signal init".into(); }
        match unsafe { kill(pid, 15) } {
            0 => format!("sent SIGTERM to {pid}"),
            r => format!("kill {pid}: error {r}"),
        }
    }
}

pub fn split_keys(buf: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < buf.len() {
        if buf[i] == 0x1b && i + 1 < buf.len() && (buf[i + 1] == b'[' || buf[i + 1] == b'O') {
            let mut j = i + 2;
            while j < buf.len() && !(buf[j] >= 0x40 && buf[j] <= 0x7e) { j += 1; }
            let end = (j + 1).min(buf.len());
            let mut seq = buf[i..end].to_vec();
            if seq.len() > 2 && seq[1] == b'O' { seq = [b"\x1b[".as_slice(), &seq[2..]].concat(); }
            out.push(seq);
            i = end;
        } else { out.push(vec![buf[i]]); i += 1; }
    }
    out
}

pub fn run(name: &str, args: &[String]) -> i32 {
    let htop = name == "htop";
    let (mut batch, mut iters, mut delay) = (false, 0u32, 1.5f64);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-b" | "--batch" => batch = true,
            "-n" => iters = it.next().and_then(|v| v.parse().ok()).unwrap_or(1),
            "-d" => delay = it.next().and_then(|v| v.parse().ok()).unwrap_or(1.5),
            "-h" | "--help" => { println!("usage: {name} [-b] [-n iterations] [-d seconds]"); return 0; }
            "-V" | "--version" => { println!("{name} (MavexOS mavexutils {})", env!("CARGO_PKG_VERSION")); return 0; }
            x => { eprintln!("{name}: unknown option '{x}'"); return 1; }
        }
    }
    let us = users();
    let mut app = App { htop, sort: Sort::Cpu, rev: false, tree: false, sel: 0, scroll: 0, procs: Vec::new(), prev: HashMap::new(), prev_cpu: cpu_times(), cpu_pct: 0.0, mem: meminfo(), msg: String::new(), prompt: None };
    let mut last = Instant::now();
    app.sample(1.0, &us);
    if batch {
        let n = if iters == 0 { 1 } else { iters };
        for i in 0..n {
            if i > 0 {
                std::thread::sleep(Duration::from_secs_f64(delay));
                let dt = last.elapsed().as_secs_f64(); last = Instant::now();
                app.sample(dt, &us);
            }
            for l in app.header_top(200) { println!("{l}"); }
            println!("{}", App::TOP_HDR);
            for p in &app.procs { println!("{}", app.table_top(p)); }
            println!();
        }
        return 0;
    }
    unsafe { tty_raw(1) };
    let mut so = std::io::stdout();
    let _ = write!(so, "\x1b[?1049h\x1b[?25l");
    let mut stdin = std::io::stdin();
    let mut frames = 0u32;
    'main: loop {
        let (cols, rows) = size();
        let frame = app.render(cols, rows);
        let _ = so.write_all(frame.as_bytes());
        let _ = so.flush();
        frames += 1;
        if iters > 0 && frames >= iters { break; }
        let deadline = Instant::now() + Duration::from_secs_f64(delay);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() { break; }
            if wait_stdin(left.as_millis().max(1) as u64) {
                let mut b = [0u8; 64];
                let n = match stdin.read(&mut b) { Ok(0) | Err(_) => break 'main, Ok(n) => n };
                let page = rows.saturating_sub(10).max(1);
                for k in split_keys(&b[..n]) { if !app.key(&k, page) { break 'main; } }
                let frame = app.render(cols, rows);
                let _ = so.write_all(frame.as_bytes());
                let _ = so.flush();
            }
        }
        let dt = last.elapsed().as_secs_f64(); last = Instant::now();
        app.sample(dt, &us);
    }
    let _ = write!(so, "\x1b[?25h\x1b[?1049l");
    let _ = so.flush();
    unsafe { tty_raw(0) };
    0
}

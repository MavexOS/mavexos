//! `ps`, `pgrep`, `pkill`, `nohup` over /proc (own implementations, procps-style output).
use std::fs;

#[link(wasm_import_module = "mavex")]
unsafe extern "C" {
    fn kill(pid: u32, sig: i32) -> i32;
    fn sig_ignore(mask: u32) -> i32;
}

pub struct P {
    pub pid: u32,
    pub ppid: u32,
    pub pgid: u32,
    pub sid: u32,
    pub tty: u32,
    pub tpgid: i64,
    pub state: char,
    pub ticks: u64,
    pub start: u64,
    pub rss_kib: u64,
    pub vsz_kib: u64,
    pub uid: u32,
    pub comm: String,
    pub args: String,
}

fn read_proc(pid: u32) -> Option<P> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    let comm = stat[open + 1..close].to_string();
    let f: Vec<&str> = stat[close + 2..].split_whitespace().collect();
    let n = |i: usize| f.get(i).and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
    let status = fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
    let uid = status.lines().find_map(|l| l.strip_prefix("Uid:")).and_then(|v| v.split_whitespace().next().and_then(|x| x.parse().ok())).unwrap_or(0);
    let cmdline = fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
    let args = cmdline.split(|b| *b == 0).filter(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).into_owned()).collect::<Vec<_>>().join(" ");
    let tty_nr = n(4);
    Some(P {
        pid,
        ppid: n(1) as u32,
        pgid: n(2) as u32,
        sid: n(3) as u32,
        tty: if tty_nr > 1024 { (tty_nr - 1024) as u32 } else { 0 },
        tpgid: n(5),
        state: f.first().and_then(|s| s.chars().next()).unwrap_or('?'),
        ticks: n(11) as u64,
        start: n(19) as u64,
        rss_kib: n(21) as u64 * 4,
        vsz_kib: n(20) as u64 / 1024,
        uid,
        args: if args.is_empty() { format!("[{comm}]") } else { args },
        comm,
    })
}

pub fn all() -> Vec<P> {
    let mut v: Vec<P> = fs::read_dir("/proc").map(|d| d.flatten().filter_map(|e| e.file_name().to_string_lossy().parse::<u32>().ok()).filter_map(read_proc).collect()).unwrap_or_default();
    v.sort_by_key(|p| p.pid);
    v
}

fn me() -> u32 {
    fs::read_to_string("/proc/self/stat").ok().and_then(|s| s.split_whitespace().next().and_then(|x| x.parse().ok())).unwrap_or(0)
}

fn tty_name(p: &P) -> String { if p.pid == 1 || p.sid == 1 || p.tty == 0 { "?".into() } else { format!("tty{}", p.tty) } }
fn hms(ticks: u64) -> String { let s = ticks / 100; format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) }
fn stat_str(p: &P) -> String {
    let mut s = p.state.to_string();
    if p.pid == p.sid { s.push('s'); }
    if p.tpgid == p.pgid as i64 { s.push('+'); }
    s
}
fn uptime_ticks() -> u64 { fs::read_to_string("/proc/uptime").ok().and_then(|s| s.split_whitespace().next().and_then(|x| x.parse::<f64>().ok())).map_or(0, |u| (u * 100.0) as u64) }
fn total_kib() -> u64 { fs::read_to_string("/proc/meminfo").ok().and_then(|s| s.lines().find_map(|l| l.strip_prefix("MemTotal:").and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok()))).unwrap_or(1) }
fn start_str(p: &P) -> String {
    let up = uptime_ticks();
    let ago = up.saturating_sub(p.start) / 100;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let t = now.saturating_sub(ago);
    format!("{:02}:{:02}", t / 3600 % 24, t / 60 % 60)
}

fn field(p: &P, name: &str) -> Option<(String, String, bool)> { // (header, value, left-aligned)
    let up = uptime_ticks().max(1);
    Some(match name {
        "pid" => ("PID".into(), p.pid.to_string(), false),
        "ppid" => ("PPID".into(), p.ppid.to_string(), false),
        "pgid" | "pgrp" => ("PGID".into(), p.pgid.to_string(), false),
        "sid" | "sess" => ("SID".into(), p.sid.to_string(), false),
        "uid" => ("UID".into(), p.uid.to_string(), false),
        "user" | "euser" | "uname" => ("USER".into(), crate::user_name(p.uid), true),
        "tty" | "tt" => ("TTY".into(), tty_name(p), true),
        "stat" | "state" | "s" => ("STAT".into(), stat_str(p), true),
        "time" | "cputime" => ("TIME".into(), hms(p.ticks), false),
        "comm" | "ucmd" | "cmd" => ("COMMAND".into(), p.comm.clone(), true),
        "args" | "command" => ("COMMAND".into(), p.args.clone(), true),
        "rss" | "rsz" => ("RSS".into(), p.rss_kib.to_string(), false),
        "vsz" | "vsize" => ("VSZ".into(), p.vsz_kib.to_string(), false),
        "%cpu" | "pcpu" => ("%CPU".into(), format!("{:.1}", p.ticks as f64 * 100.0 / (up.saturating_sub(p.start).max(1)) as f64), false),
        "%mem" | "pmem" => ("%MEM".into(), format!("{:.1}", p.rss_kib as f64 * 100.0 / total_kib() as f64), false),
        "start" | "stime" | "start_time" => ("START".into(), start_str(p), false),
        "c" => ("C".into(), "0".into(), false),
        "nice" | "ni" => ("NI".into(), "0".into(), false),
        _ => return None,
    })
}

fn table(procs: &[P], cols: &[(String, String)]) { // (field, header override)
    let rows: Vec<Vec<(String, String, bool)>> = procs.iter().map(|p| cols.iter().map(|(c, _)| field(p, c).unwrap()).collect()).collect();
    let heads: Vec<String> = cols.iter().enumerate().map(|(i, (c, h))| if h.is_empty() { field(&procs.first().map_or_else(|| read_proc(1).unwrap(), |p| P { comm: String::new(), args: String::new(), ..*p_clone(p) }), c).map_or_else(|| c.to_uppercase(), |x| x.0) } else { let _ = i; h.clone() }).collect();
    let mut w: Vec<usize> = heads.iter().map(|h| h.len()).collect();
    for r in &rows { for (i, c) in r.iter().enumerate() { w[i] = w[i].max(c.1.len()); } }
    let line = |cells: Vec<(String, bool)>| {
        let n = cells.len();
        let mut s = String::new();
        for (i, (c, left)) in cells.into_iter().enumerate() {
            if i > 0 { s.push(' '); }
            if i == n - 1 && left { s.push_str(&c); } else if left { s.push_str(&format!("{c:<w$}", w = w[i])); } else { s.push_str(&format!("{c:>w$}", w = w[i])); }
        }
        println!("{s}");
    };
    let lefts: Vec<bool> = cols.iter().map(|(c, _)| field(&read_proc(me()).unwrap_or_else(|| read_proc(1).unwrap()), c).is_some_and(|x| x.2)).collect();
    line(heads.into_iter().zip(&lefts).map(|(h, l)| (h, *l)).collect());
    for r in rows { line(r.into_iter().map(|c| (c.1, c.2)).collect()); }
}

fn p_clone(p: &P) -> Box<P> { Box::new(P { pid: p.pid, ppid: p.ppid, pgid: p.pgid, sid: p.sid, tty: p.tty, tpgid: p.tpgid, state: p.state, ticks: p.ticks, start: p.start, rss_kib: p.rss_kib, vsz_kib: p.vsz_kib, uid: p.uid, comm: p.comm.clone(), args: p.args.clone() }) }

pub fn ps(args: &[String]) -> i32 {
    let mut everything = false;
    let mut format = "default";
    let mut custom: Vec<String> = Vec::new();
    let mut i = 0;
    let mut sel_pids: Vec<u32> = Vec::new();
    let mut sel_user: Option<String> = None;
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "-e" | "-A" | "ax" | "-ax" | "-aux" | "aux" | "axu" | "-axu" => { everything = true; if a.contains('u') { format = "aux"; } }
            "-f" => format = "full",
            "-ef" | "-Af" | "-fe" => { everything = true; format = "full"; }
            "-l" => format = "long",
            "u" | "-u" if i + 1 >= args.len() || a == "u" => format = "aux",
            "-o" | "o" | "-O" => { i += 1; for f in args.get(i).map(|s| s.as_str()).unwrap_or("").split(',') { let (n, h) = f.split_once('=').unwrap_or((f, "")); custom.push(format!("{n}\t{h}")); } format = "custom"; }
            "-p" | "p" | "--pid" => { i += 1; sel_pids.extend(args.get(i).map(|s| s.as_str()).unwrap_or("").split(',').filter_map(|x| x.parse::<u32>().ok())); }
            "-u" | "-U" | "--user" => { i += 1; sel_user = args.get(i).cloned(); }
            "--help" => { println!("Usage: ps [-e|-A|ax] [-f|-l|u] [-o fmt] [-p pid,..] [-u user]"); return 0; }
            "--version" | "-V" => { println!("ps (MavexOS) 0.1"); return 0; }
            _ if a.starts_with("-o") && a.len() > 2 => { for f in a[2..].split(',') { let (n, h) = f.split_once('=').unwrap_or((f, "")); custom.push(format!("{n}\t{h}")); } format = "custom"; }
            _ if a.starts_with('-') || a.chars().all(|c| "auxeAfl".contains(c)) => {
                let s = a.trim_start_matches('-');
                for c in s.chars() { match c { 'e' | 'A' | 'a' | 'x' => everything = true, 'f' => format = "full", 'u' => format = "aux", 'l' => format = "long", _ => { eprintln!("ps: unsupported option '{a}'"); return 1; } } }
            }
            _ => { eprintln!("ps: error: garbage option '{a}'"); return 1; }
        }
        i += 1;
    }
    let mine = read_proc(me());
    let uid_sel = sel_user.as_ref().and_then(|u| u.parse::<u32>().ok().or_else(|| fs::read_to_string("/etc/passwd").ok().and_then(|p| p.lines().find_map(|l| { let f: Vec<&str> = l.split(':').collect(); (f.first() == Some(&u.as_str())).then(|| f.get(2).and_then(|x| x.parse().ok())).flatten() }))));
    if sel_user.is_some() && uid_sel.is_none() { eprintln!("ps: user {} does not exist", sel_user.unwrap()); return 1; }
    let procs: Vec<P> = all().into_iter().filter(|p| {
        if !sel_pids.is_empty() { return sel_pids.contains(&p.pid); }
        if let Some(u) = uid_sel { return p.uid == u; }
        everything || mine.as_ref().is_some_and(|m| p.sid == m.sid)
    }).collect();
    let cols: Vec<(&str, &str)> = match format {
        "aux" => vec![("user", ""), ("pid", ""), ("%cpu", ""), ("%mem", ""), ("vsz", ""), ("rss", ""), ("tty", ""), ("stat", ""), ("start", ""), ("time", ""), ("args", "")],
        "full" => vec![("user", "UID"), ("pid", ""), ("ppid", ""), ("c", ""), ("start", "STIME"), ("tty", ""), ("time", ""), ("args", "CMD")],
        "long" => vec![("stat", "S"), ("uid", ""), ("pid", ""), ("ppid", ""), ("c", ""), ("nice", ""), ("vsz", "SZ"), ("tty", ""), ("time", ""), ("comm", "CMD")],
        "custom" => custom.iter().map(|c| { let (n, h) = c.split_once('\t').unwrap(); (n, h) }).collect(),
        _ => vec![("pid", ""), ("tty", ""), ("time", ""), ("comm", "CMD")],
    };
    for (c, _) in &cols { if field(&P { pid: 0, ppid: 0, pgid: 0, sid: 0, tty: 0, tpgid: 0, state: 'R', ticks: 0, start: 0, rss_kib: 0, vsz_kib: 0, uid: 0, comm: String::new(), args: String::new() }, c).is_none() { eprintln!("ps: error: unknown user-defined format specifier \"{c}\""); return 1; } }
    let cols: Vec<(String, String)> = cols.into_iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
    table(&procs, &cols);
    0
}

pub const SIGNALS: &[(&str, i32)] = &[("HUP", 1), ("INT", 2), ("QUIT", 3), ("ABRT", 6), ("KILL", 9), ("USR1", 10), ("SEGV", 11), ("USR2", 12), ("PIPE", 13), ("ALRM", 14), ("TERM", 15), ("CHLD", 17), ("CONT", 18), ("STOP", 19), ("TSTP", 20), ("TTIN", 21), ("TTOU", 22), ("WINCH", 28)];

fn parse_sig(s: &str) -> Option<i32> {
    let t = s.trim_start_matches("SIG");
    if let Ok(n) = t.parse::<i32>() { return Some(n); }
    SIGNALS.iter().find(|(n, _)| *n == t.to_uppercase()).map(|(_, v)| *v)
}

struct Sel { pat: Option<regex_lite::Regex>, full: bool, exact: bool, newest: bool, oldest: bool, users: Vec<u32>, list: bool, count: bool, sig: i32, parent: Option<u32> }

fn parse_sel(tool: &str, args: &[String]) -> Result<Sel, i32> {
    let mut s = Sel { pat: None, full: false, exact: false, newest: false, oldest: false, users: vec![], list: false, count: false, sig: 15, parent: None };
    let mut pat: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "-f" | "--full" => s.full = true,
            "-x" | "--exact" => s.exact = true,
            "-n" | "--newest" => s.newest = true,
            "-o" | "--oldest" => s.oldest = true,
            "-l" | "--list-name" if tool == "pgrep" => s.list = true,
            "-a" | "--list-full" if tool == "pgrep" => { s.list = true; s.full = true; }
            "-c" | "--count" => s.count = true,
            "-i" | "--ignore-case" => {}
            "-u" | "--euid" | "-U" | "--uid" => { i += 1; let u = args.get(i).cloned().unwrap_or_default(); match u.parse::<u32>().ok().or_else(|| fs::read_to_string("/etc/passwd").ok().and_then(|p| p.lines().find_map(|l| { let f: Vec<&str> = l.split(':').collect(); (f.first() == Some(&u.as_str())).then(|| f.get(2).and_then(|x| x.parse().ok())).flatten() }))) { Some(n) => s.users.push(n), None => { eprintln!("{tool}: invalid user name: {u}"); return Err(2); } } }
            "-P" | "--parent" => { i += 1; s.parent = args.get(i).and_then(|x| x.parse().ok()); }
            "-s" | "--signal" if tool == "pkill" => { i += 1; match args.get(i).and_then(|x| parse_sig(x)) { Some(n) => s.sig = n, None => { eprintln!("pkill: unknown signal"); return Err(2); } } }
            "--" => { i += 1; if pat.is_none() { pat = args.get(i).cloned(); } break; }
            "--help" | "-h" => { println!("Usage: {tool} [options] <pattern>\n  -f full command line  -x exact match  -n newest  -o oldest  -u user  -P ppid  -c count{}", if tool == "pgrep" { "  -l list names  -a list full" } else { "  -SIGNAL / -s SIGNAL" }); return Err(0); }
            _ if a.starts_with('-') && a.len() > 1 && tool == "pkill" && parse_sig(&a[1..]).is_some() => s.sig = parse_sig(&a[1..]).unwrap(),
            _ if a.starts_with('-') && a.len() > 1 => { eprintln!("{tool}: unrecognized option '{a}'"); return Err(2); }
            _ => if pat.is_none() { pat = Some(a.to_string()); } else { eprintln!("{tool}: only one pattern can be provided"); return Err(2); },
        }
        i += 1;
    }
    if pat.is_none() && s.users.is_empty() && s.parent.is_none() { eprintln!("{tool}: no matching criteria specified\nTry `{tool} --help' for more information."); return Err(2); }
    if let Some(p) = pat {
        let p = if s.exact { format!("^(?:{p})$") } else { p };
        match regex_lite::Regex::new(&p) { Ok(r) => s.pat = Some(r), Err(e) => { eprintln!("{tool}: invalid pattern: {e}"); return Err(2); } }
    }
    Ok(s)
}

fn select(s: &Sel) -> Vec<P> {
    let self_pid = me();
    let mut v: Vec<P> = all().into_iter().filter(|p| {
        p.pid != self_pid && p.pid != 1
            && s.pat.as_ref().is_none_or(|r| r.is_match(if s.full { &p.args } else { &p.comm }))
            && (s.users.is_empty() || s.users.contains(&p.uid))
            && s.parent.is_none_or(|pp| p.ppid == pp)
    }).collect();
    if s.newest { v.sort_by_key(|p| (p.start, p.pid)); if let Some(l) = v.pop() { v = vec![l]; } }
    else if s.oldest { v.sort_by_key(|p| (p.start, p.pid)); v.truncate(1); }
    v
}

pub fn pgrep(args: &[String]) -> i32 {
    let s = match parse_sel("pgrep", args) { Ok(s) => s, Err(c) => return c };
    let v = select(&s);
    if s.count { println!("{}", v.len()); return if v.is_empty() { 1 } else { 0 }; }
    for p in &v { if s.list { println!("{} {}", p.pid, if s.full { &p.args } else { &p.comm }); } else { println!("{}", p.pid); } }
    if v.is_empty() { 1 } else { 0 }
}

pub fn pkill(args: &[String]) -> i32 {
    let s = match parse_sel("pkill", args) { Ok(s) => s, Err(c) => return c };
    let v = select(&s);
    let mut rc = if v.is_empty() { 1 } else { 0 };
    for p in &v {
        if unsafe { kill(p.pid, s.sig) } < 0 { eprintln!("pkill: killing pid {} failed: Operation not permitted", p.pid); rc = 1; }
    }
    if s.count { println!("{}", v.len()); }
    rc
}

pub fn nohup(args: &[String]) -> i32 {
    if args.is_empty() { eprintln!("nohup: missing operand\nTry 'nohup --help' for more information."); return 125; }
    use std::io::IsTerminal;
    use std::os::fd::AsRawFd;
    unsafe { sig_ignore(1) }; // SIGHUP
    let (mut s0, mut s1, mut s2) = (0i32, 1i32, 2i32);
    let mut keep = Vec::new();
    let tin = std::io::stdin().is_terminal();
    let tout = std::io::stdout().is_terminal();
    let terr = std::io::stderr().is_terminal();
    if tin { s0 = -2; }
    if tout {
        let path = match std::env::var("HOME") { Ok(h) if fs::OpenOptions::new().create(true).append(true).open("nohup.out").is_err() => format!("{h}/nohup.out"), _ => "nohup.out".into() };
        match fs::OpenOptions::new().create(true).append(true).open(&path) {
            Ok(f) => { eprintln!("nohup: {}ignoring input and appending output to '{path}'", if tin { "" } else { "" }); s1 = f.as_raw_fd(); if terr { s2 = s1; } keep.push(f); }
            Err(e) => { eprintln!("nohup: cannot open '{path}': {e}"); return 125; }
        }
    } else if terr {
        s2 = s1;
    }
    let j = |v: &[String]| { let mut b = Vec::new(); for s in v { b.extend_from_slice(s.as_bytes()); b.push(0); } b };
    let env: Vec<String> = std::env::vars().map(|(k, v)| format!("{k}={v}")).collect();
    let cwd = std::env::var("PWD").unwrap_or_else(|_| "/".into());
    let (a, e) = (j(args), j(&env));
    let prog = &args[0];
    let pid = unsafe { crate::spawn(prog.as_ptr(), prog.len(), a.as_ptr(), a.len(), e.as_ptr(), e.len(), cwd.as_ptr(), cwd.len(), s0, s1, s2) };
    if pid < 0 { eprintln!("nohup: failed to run command '{prog}': No such file or directory"); return 127; }
    let rc = unsafe { crate::run(pid as u32); crate::wait(pid as u32) };
    drop(keep);
    rc
}

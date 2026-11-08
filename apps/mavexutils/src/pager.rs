//! `less` and `man` (own implementations) sharing one full-screen pager.
use crate::top::{size, split_keys, wait_stdin};
use std::io::{Read, Write};

#[link(wasm_import_module = "mavex")]
unsafe extern "C" {
    fn tty_raw(on: i32) -> i32;
}

fn is_tty(fd: i32) -> bool {
    use std::io::IsTerminal;
    match fd { 0 => std::io::stdin().is_terminal(), _ => std::io::stdout().is_terminal() }
}

fn vis_len(s: &str) -> usize {
    let mut n = 0;
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\x1b' { while let Some(&d) = it.peek() { it.next(); if d.is_ascii_alphabetic() { break; } } } else if c != '\x08' { n += 1; }
    }
    n
}

fn chop(s: &str, w: usize) -> String {
    let mut out = String::new();
    let mut n = 0;
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\x1b' { out.push(c); while let Some(&d) = it.peek() { out.push(d); it.next(); if d.is_ascii_alphabetic() { break; } } continue; }
        if n >= w { continue; }
        out.push(c);
        n += 1;
    }
    out
}

fn wrap(lines: &[String], w: usize) -> Vec<String> {
    let mut v = Vec::new();
    for l in lines {
        let l = l.replace('\t', "        ");
        if vis_len(&l) <= w { v.push(l); continue; }
        let cs: Vec<char> = l.chars().collect();
        for c in cs.chunks(w.max(1)) { v.push(c.iter().collect()); }
    }
    v
}

fn highlight(line: &str, pat: &str) -> String {
    if pat.is_empty() { return line.to_string(); }
    line.replace(pat, &format!("\x1b[7m{pat}\x1b[27m"))
}

/// Full-screen pager. `keys` is where key presses are read from (a tty fd).
pub fn page(raw: Vec<String>, title: &str, numbers: bool, chop_long: bool, quit_if_one: bool, no_init: bool) -> i32 {
    let (mut cols, mut rows) = size();
    if quit_if_one && raw.len() < rows { for l in &raw { println!("{l}"); } return 0; }
    let mut keys = if is_tty(0) { None } else { std::fs::File::open("/dev/tty").ok() };
    unsafe { tty_raw(1) };
    let mut so = std::io::stdout();
    if !no_init { let _ = write!(so, "\x1b[?1049h"); }
    let mut top = 0usize;
    let mut pat = String::new();
    let mut prompt: Option<String> = None;
    let mut msg = String::new();
    let mut help = false;
    let mut stdin = std::io::stdin();
    loop {
        let (c, r) = size(); cols = c; rows = r;
        let body_rows = rows - 1;
        let lines: Vec<String> = if help { HELP.lines().map(String::from).collect() } else if numbers { raw.iter().enumerate().map(|(i, l)| format!("{:>6}  {l}", i + 1)).collect() } else { raw.clone() };
        let disp = if chop_long { lines.iter().map(|l| chop(&l.replace('\t', "        "), cols)).collect::<Vec<_>>() } else { wrap(&lines, cols) };
        let max_top = disp.len().saturating_sub(body_rows);
        top = top.min(max_top);
        let mut f = String::from("\x1b[H");
        for i in 0..body_rows {
            match disp.get(top + i) { Some(l) => f.push_str(&highlight(l, &pat)), None => f.push('~') }
            f.push_str("\x1b[K\r\n");
        }
        let status = if let Some(p) = &prompt { format!("/{p}") } else if !msg.is_empty() { msg.clone() } else if top >= max_top { "(END)".to_string() } else { format!("{title} lines {}-{}/{} {}%", top + 1, (top + body_rows).min(disp.len()), disp.len(), (top + body_rows).min(disp.len()) * 100 / disp.len().max(1)) };
        f.push_str(&format!("\x1b[7m{}\x1b[27m\x1b[K", chop(&status, cols)));
        let _ = so.write_all(f.as_bytes());
        let _ = so.flush();
        msg.clear();
        wait_stdin_any(keys.is_some());
        let mut b = [0u8; 64];
        let n = match &mut keys { Some(k) => k.read(&mut b), None => stdin.read(&mut b) };
        let n = match n { Ok(0) | Err(_) => break, Ok(n) => n };
        for k in split_keys(&b[..n]) {
            if let Some(p) = prompt.as_mut() {
                match k.as_slice() {
                    b"\r" | b"\n" => {
                        pat = prompt.take().unwrap_or_default();
                        match disp.iter().enumerate().skip(top + 1).find(|(_, l)| l.contains(&pat)).or_else(|| if pat.is_empty() { None } else { disp.iter().enumerate().find(|(_, l)| l.contains(&pat)) }) {
                            Some((i, _)) => top = i,
                            None => msg = format!("Pattern not found: {pat}"),
                        }
                    }
                    b"\x1b" | b"\x03" => prompt = None,
                    b"\x7f" => { if p.pop().is_none() { prompt = None; } }
                    c if c.len() == 1 && c[0] >= 32 => p.push(c[0] as char),
                    _ => {}
                }
                continue;
            }
            if help { help = false; continue; }
            let page = body_rows.saturating_sub(1).max(1);
            match k.as_slice() {
                b"q" | b"Q" | b"ZZ" | b"\x03" => { cleanup(&mut so, no_init); return 0; }
                b" " | b"f" | b"\x1b[6~" | b"\x06" => { if top >= max_top && b" f".contains(&k[0]) { cleanup(&mut so, no_init); return 0; } top += page }
                b"b" | b"\x1b[5~" | b"\x02" => top = top.saturating_sub(page),
                b"d" | b"\x04" => top += body_rows / 2,
                b"u" | b"\x15" => top = top.saturating_sub(body_rows / 2),
                b"j" | b"e" | b"\r" | b"\n" | b"\x1b[B" => top += 1,
                b"k" | b"y" | b"\x1b[A" => top = top.saturating_sub(1),
                b"g" | b"<" | b"\x1b[H" => top = 0,
                b"G" | b">" | b"\x1b[F" => top = max_top,
                b"/" => prompt = Some(String::new()),
                b"n" => if let Some((i, _)) = disp.iter().enumerate().skip(top + 1).find(|(_, l)| !pat.is_empty() && l.contains(&pat)) { top = i } else { msg = "Pattern not found".into() },
                b"N" => if let Some((i, _)) = disp.iter().enumerate().take(top).rev().find(|(_, l)| !pat.is_empty() && l.contains(&pat)) { top = i } else { msg = "Pattern not found".into() },
                b"h" | b"H" => help = true,
                _ => {}
            }
        }
    }
    cleanup(&mut so, no_init);
    0
}

fn cleanup(so: &mut std::io::Stdout, no_init: bool) {
    if !no_init { let _ = write!(so, "\x1b[?1049l"); }
    let _ = so.flush();
    unsafe { tty_raw(0) };
}

fn wait_stdin_any(from_dev_tty: bool) {
    // /dev/tty shares the tty input queue, so polling fd 0 only works when fd 0 is that tty; otherwise block in read.
    if !from_dev_tty { while !wait_stdin(1000) {} }
}

const HELP: &str = "\n  SUMMARY OF LESS COMMANDS\n\n  q          Quit\n  SPACE, f   Forward one page        b          Backward one page\n  j, ENTER   Forward one line        k          Backward one line\n  d / u      Half page down / up     g / G      Top / bottom\n  /pattern   Search forward          n / N      Next / previous match\n  h          This help\n\n  (press any key to return)";

pub fn less(args: &[String]) -> i32 {
    let (mut numbers, mut chop_l, mut f, mut x) = (false, false, false, false);
    let mut files: Vec<&String> = Vec::new();
    for a in args {
        if a.starts_with('-') && a.len() > 1 && !a.starts_with("--") {
            for c in a[1..].chars() { match c { 'N' => numbers = true, 'S' => chop_l = true, 'F' => f = true, 'X' => x = true, 'R' | 'r' | 'i' | 'I' | 'M' | 'm' | 'e' | 'E' | 'K' => {}, _ => { eprintln!("less: invalid option -- '{c}'"); return 1; } } }
        } else if a == "--help" { println!("Usage: less [-NSFX] [file...]"); return 0; } else if a.starts_with("--") { } else { files.push(a); }
    }
    let mut text = String::new();
    if files.is_empty() {
        if is_tty(0) { eprintln!("Missing filename (\"less --help\" for help)"); return 1; }
        let mut b = Vec::new();
        let _ = std::io::stdin().read_to_end(&mut b);
        text = String::from_utf8_lossy(&b).into_owned();
    } else {
        for f in &files {
            match std::fs::read(f) { Ok(b) => text.push_str(&String::from_utf8_lossy(&b)), Err(e) => { eprintln!("{f}: {}", e.to_string().split(" (os").next().unwrap_or("error")); return 1; } }
        }
    }
    let lines: Vec<String> = text.lines().map(|l| l.trim_end_matches('\r').to_string()).collect();
    if !is_tty(1) { for l in &lines { println!("{l}"); } return 0; }
    page(lines, files.first().map_or("(stdin)", |s| s.as_str()), numbers, chop_l, f, x)
}

fn roff(src: &str) -> Vec<String> {
    let esc = |s: &str| s.replace("\\-", "-").replace("\\&", "").replace("\\(em", "--").replace("\\e", "\\").replace("\\fB", "").replace("\\fI", "").replace("\\fR", "").replace("\\fP", "").replace("\\ ", " ");
    let mut out = Vec::new();
    for l in src.lines() {
        if l.starts_with(".\\\"") || l.starts_with("'\\\"") { continue; }
        if let Some(rest) = l.strip_prefix('.') {
            let (m, arg) = rest.split_once(' ').unwrap_or((rest, ""));
            let arg = esc(arg.trim().trim_matches('"'));
            match m {
                "TH" => out.push(arg.split_whitespace().take(2).collect::<Vec<_>>().join("(") + ")"),
                "SH" | "SS" => { out.push(String::new()); out.push(arg.to_uppercase()); }
                "B" | "I" | "SM" | "BR" | "IR" | "RB" | "BI" | "IB" | "RI" => out.push(format!("       {}", arg.replace('"', ""))),
                "TP" | "PP" | "P" | "LP" | "IP" => out.push(String::new()),
                "br" | "sp" => out.push(String::new()),
                _ => {}
            }
        } else { out.push(format!("       {}", esc(l))); }
    }
    out
}

pub fn man(args: &[String]) -> i32 {
    let names: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
    let sect = names.first().filter(|n| n.len() == 1 && n.chars().all(|c| c.is_ascii_digit())).map(|s| s.to_string());
    let name = match if sect.is_some() { names.get(1) } else { names.first() } { Some(n) => n.as_str(), None => { eprintln!("What manual page do you want?\nFor example, try 'man ls'."); return 1; } };
    let mp = std::env::var("MANPATH").unwrap_or_else(|_| "/usr/share/man".into());
    let mut lines: Option<Vec<String>> = None;
    'o: for base in mp.split(':') {
        for s in sect.iter().map(|s| s.as_str()).chain("12345678".split("").filter(|s| !s.is_empty()).filter(|_| sect.is_none())) {
            for ext in ["", ".gz"] {
                let _ = ext;
                let p = format!("{base}/man{s}/{name}.{s}");
                if let Ok(t) = std::fs::read_to_string(&p) { lines = Some(if t.contains(".TH") { roff(&t) } else { t.lines().map(String::from).collect() }); break 'o; }
            }
        }
    }
    let lines = match lines { Some(l) => l, None => {
        let tmp = format!("/tmp/.man-{name}");
        let f = match std::fs::File::create(&tmp) { Ok(f) => f, Err(_) => { eprintln!("No manual entry for {name}"); return 16; } };
        use std::os::fd::AsRawFd;
        let argv = vec![name.to_string(), "--help".to_string()];
        let env: Vec<String> = std::env::vars().map(|(k, v)| format!("{k}={v}")).collect();
        let cwd = std::env::var("PWD").unwrap_or_else(|_| "/".into());
        let j = |v: &[String]| { let mut b = Vec::new(); for s in v { b.extend_from_slice(s.as_bytes()); b.push(0); } b };
        let (a, e) = (j(&argv), j(&env));
        let fd = f.as_raw_fd();
        let pid = unsafe { crate::spawn(name.as_ptr(), name.len(), a.as_ptr(), a.len(), e.as_ptr(), e.len(), cwd.as_ptr(), cwd.len(), -2, fd, fd) };
        if pid < 0 { let _ = std::fs::remove_file(&tmp); eprintln!("No manual entry for {name}"); return 16; }
        unsafe { crate::run(pid as u32); crate::wait(pid as u32); }
        drop(f);
        let t = std::fs::read_to_string(&tmp).unwrap_or_default();
        let _ = std::fs::remove_file(&tmp);
        if t.trim().is_empty() { eprintln!("No manual entry for {name}"); return 16; }
        let mut v = vec![format!("{} (no man page installed: showing `{name} --help`)", name.to_uppercase()), String::new()];
        v.extend(t.lines().map(String::from));
        v
    } };
    if !is_tty(1) { for l in &lines { println!("{l}"); } return 0; }
    page(lines, &format!("Manual page {name}"), false, false, false, false)
}

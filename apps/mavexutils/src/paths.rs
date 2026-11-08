//! `realpath` and `readlink` (own implementations: std's canonicalize is unsupported on wasip1).
use std::path::{Path, PathBuf};

fn cwd() -> String { std::env::var("PWD").unwrap_or_else(|_| "/".into()) }

/// Resolves symlinks component by component. `must_exist`: every component must exist (-e); `last_may_miss`: only the final component may be missing (-f).
fn resolve(p: &str, must_exist: bool, last_may_miss: bool) -> Result<String, String> {
    let mut pending: Vec<String> = if p.starts_with('/') { p.split('/').filter(|s| !s.is_empty()).rev().map(String::from).collect() } else { format!("{}/{p}", cwd()).split('/').filter(|s| !s.is_empty()).rev().map(String::from).collect() };
    let mut cur: Vec<String> = Vec::new();
    let mut links = 0;
    while let Some(c) = pending.pop() {
        match c.as_str() {
            "." => continue,
            ".." => { cur.pop(); continue; }
            _ => {}
        }
        let here = format!("/{}", cur.iter().chain(std::iter::once(&c)).cloned().collect::<Vec<_>>().join("/"));
        match std::fs::symlink_metadata(&here) {
            Ok(m) if m.file_type().is_symlink() => {
                links += 1;
                if links > 40 { return Err("Too many levels of symbolic links".into()); }
                let t = std::fs::read_link(&here).map_err(|e| e.to_string())?;
                let t = t.to_string_lossy().into_owned();
                if t.starts_with('/') { cur.clear(); }
                for s in t.split('/').filter(|s| !s.is_empty()).rev() { pending.push(s.to_string()); }
            }
            Ok(_) => cur.push(c),
            Err(_) => {
                let last = pending.iter().all(|s| s == "." || s == "..") ;
                if must_exist || !last_may_miss || !last { return Err("No such file or directory".into()); }
                cur.push(c);
            }
        }
    }
    Ok(format!("/{}", cur.join("/")))
}

fn relative(from: &str, to: &str) -> String {
    let f: Vec<&str> = from.split('/').filter(|s| !s.is_empty()).collect();
    let t: Vec<&str> = to.split('/').filter(|s| !s.is_empty()).collect();
    let n = f.iter().zip(&t).take_while(|(a, b)| a == b).count();
    let mut v: Vec<&str> = vec![".."; f.len() - n];
    v.extend(&t[n..]);
    if v.is_empty() { ".".into() } else { v.join("/") }
}

pub fn realpath(args: &[String]) -> i32 {
    let (mut e, mut m, mut s, mut q, mut zero) = (false, false, false, false, false);
    let mut rel: Option<String> = None;
    let mut files = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-e" | "--canonicalize-existing" => e = true,
            "-m" | "--canonicalize-missing" => m = true,
            "-s" | "--strip" | "--no-symlinks" => s = true,
            "-q" | "--quiet" => q = true,
            "-z" | "--zero" => zero = true,
            "-L" | "-P" => {}
            "--relative-to" => { i += 1; rel = args.get(i).cloned(); }
            a if a.starts_with("--relative-to=") => rel = Some(a[14..].to_string()),
            "--help" => { println!("Usage: realpath [-emsqz] [--relative-to=DIR] FILE..."); return 0; }
            "--" => { files.extend(args[i + 1..].iter().cloned()); break; }
            a if a.starts_with('-') && a.len() > 1 => { eprintln!("realpath: invalid option -- '{a}'"); return 1; }
            a => files.push(a.to_string()),
        }
        i += 1;
    }
    if files.is_empty() { eprintln!("realpath: missing operand"); return 1; }
    let mut rc = 0;
    for f in &files {
        let r = if s { resolve_lexical(f) } else if m { resolve_missing(f) } else { resolve(f, e, !e) };
        match r {
            Ok(mut p) => {
                if let Some(base) = &rel { if let Ok(b) = resolve(base, false, true) { p = relative(&b, &p); } }
                if zero { print!("{p}\0"); } else { println!("{p}"); }
            }
            Err(msg) => { if !q { eprintln!("realpath: {f}: {msg}"); } rc = 1; }
        }
    }
    rc
}

fn resolve_lexical(p: &str) -> Result<String, String> {
    let mut cur: Vec<String> = Vec::new();
    let full = if p.starts_with('/') { p.to_string() } else { format!("{}/{p}", cwd()) };
    for c in full.split('/') { match c { "" | "." => {} ".." => { cur.pop(); } _ => cur.push(c.to_string()) } }
    Ok(format!("/{}", cur.join("/")))
}

fn resolve_missing(p: &str) -> Result<String, String> {
    let lex = resolve_lexical(p)?;
    let mut base = PathBuf::from(&lex);
    let mut tail: Vec<String> = Vec::new();
    while !base.as_os_str().is_empty() && base != Path::new("/") && std::fs::symlink_metadata(&base).is_err() {
        tail.push(base.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
        base.pop();
    }
    let mut r = resolve(&base.to_string_lossy(), false, true)?;
    for t in tail.iter().rev() { if r != "/" { r.push('/'); } r.push_str(t); }
    Ok(r)
}

pub fn readlink(args: &[String]) -> i32 {
    let (mut f, mut e, mut m, mut n, mut q) = (false, false, false, false, false);
    let mut files = Vec::new();
    for a in args {
        match a.as_str() {
            "-f" | "--canonicalize" => f = true,
            "-e" | "--canonicalize-existing" => e = true,
            "-m" | "--canonicalize-missing" => m = true,
            "-n" | "--no-newline" => n = true,
            "-q" | "-s" | "--quiet" | "--silent" | "-v" | "--verbose" | "-z" | "--zero" => q = q || a == "-q" || a == "-s",
            "--help" => { println!("Usage: readlink [-femnq] FILE..."); return 0; }
            a if a.starts_with('-') && a.len() > 1 => { eprintln!("readlink: invalid option -- '{a}'"); return 1; }
            a => files.push(a.to_string()),
        }
    }
    if files.is_empty() { eprintln!("readlink: missing operand"); return 1; }
    let mut rc = 0;
    for p in &files {
        let r: Result<String, String> = if m { resolve_missing(p) } else if f { resolve(p, false, true) } else if e { resolve(p, true, false) } else {
            std::fs::read_link(p).map(|t| t.to_string_lossy().into_owned()).map_err(|_| String::new())
        };
        match r {
            Ok(s) => { if n && files.len() == 1 { print!("{s}"); } else { println!("{s}"); } }
            Err(msg) => { if !q && !msg.is_empty() { eprintln!("readlink: {p}: {msg}"); } rc = 1; }
        }
    }
    rc
}

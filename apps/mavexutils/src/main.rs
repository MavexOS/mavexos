//! MavexOS utilities multicall: id, whoami, groups, chmod, chown, stat, sudo, su, ip.
//! `sudo`/`su` are MavexOS' own minimal implementations (not sudo-rs); `ip` is a reimplementation
//! of an iproute2-compatible subset over the kernel's virtual netns (netlink-like requests).
mod ip;
mod sysutils;
mod top;
mod wg;

use std::process::exit;

#[link(wasm_import_module = "mavex")]
unsafe extern "C" {
    fn getcreds(out: *mut u32) -> i32;
    fn setcreds(uid: u32, gid: u32) -> i32;
    fn chmod(p: *const u8, n: usize, mode: u32) -> i32;
    fn chown(p: *const u8, n: usize, uid: u32, gid: u32) -> i32;
    fn statx(p: *const u8, n: usize, out: *mut u32) -> i32;
    fn net(rp: *const u8, rn: usize, out: *mut u8, cap: usize) -> i32;
    fn push_shell(cp: *const u8, cn: usize, ep: *const u8, en: usize) -> i32;
    fn spawn(prog: *const u8, prog_n: usize, argv: *const u8, argv_n: usize, env: *const u8, env_n: usize, cwd: *const u8, cwd_n: usize, s0: i32, s1: i32, s2: i32) -> i32;
    fn run(pid: u32) -> i32;
    fn wait(pid: u32) -> i32;
}

pub fn creds() -> (u32, u32, u32, u32) {
    let mut o = [0u32; 4];
    unsafe { getcreds(o.as_mut_ptr()) };
    (o[0], o[1], o[2], o[3])
}

pub fn net_request(req: &str) -> Result<String, (i32, String)> {
    let mut buf = vec![0u8; 1 << 16];
    let r = unsafe { net(req.as_ptr(), req.len(), buf.as_mut_ptr(), buf.len()) };
    let end = buf.iter().position(|b| *b == 0).unwrap_or(0);
    let s = String::from_utf8_lossy(&buf[..end]).into_owned();
    if r < 0 { Err((r, s)) } else { Ok(s) }
}

struct User {
    name: String,
    uid: u32,
    gid: u32,
    home: String,
    shell: String,
}

fn users() -> Vec<User> {
    std::fs::read_to_string("/etc/passwd")
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            if f.len() < 7 { return None; }
            Some(User { name: f[0].into(), uid: f[2].parse().ok()?, gid: f[3].parse().ok()?, home: f[5].into(), shell: f[6].into() })
        })
        .collect()
}

struct Group {
    name: String,
    gid: u32,
    members: Vec<String>,
}

fn groups_db() -> Vec<Group> {
    std::fs::read_to_string("/etc/group")
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            if f.len() < 4 { return None; }
            Some(Group { name: f[0].into(), gid: f[2].parse().ok()?, members: f[3].split(',').filter(|s| !s.is_empty()).map(String::from).collect() })
        })
        .collect()
}

pub fn user_name(uid: u32) -> String {
    users().into_iter().find(|u| u.uid == uid).map(|u| u.name).unwrap_or_else(|| uid.to_string())
}

fn group_name(gid: u32) -> String {
    groups_db().into_iter().find(|g| g.gid == gid).map(|g| g.name).unwrap_or_else(|| gid.to_string())
}

fn lookup_user(s: &str) -> Option<User> {
    users().into_iter().find(|u| u.name == s || s.parse::<u32>().is_ok_and(|n| n == u.uid))
}

fn supplementary(user: &str, primary: u32) -> Vec<u32> {
    let mut v = vec![primary];
    for g in groups_db() {
        if g.members.iter().any(|m| m == user) && !v.contains(&g.gid) {
            v.push(g.gid);
        }
    }
    v
}

fn cmd_id(args: &[String]) -> i32 {
    let (mut u, mut g, mut gs, mut n, mut r) = (false, false, false, false, false);
    let mut who = None;
    for a in args {
        match a.as_str() {
            "-u" | "--user" => u = true,
            "-g" | "--group" => g = true,
            "-G" | "--groups" => gs = true,
            "-n" | "--name" => n = true,
            "-r" | "--real" => r = true,
            s if s.starts_with('-') && s.len() > 1 => {
                for c in s[1..].chars() {
                    match c { 'u' => u = true, 'g' => g = true, 'G' => gs = true, 'n' => n = true, 'r' => r = true, _ => { eprintln!("id: invalid option -- '{c}'"); return 1; } }
                }
            }
            s => who = Some(s.to_string()),
        }
    }
    let (ruid, rgid, euid, egid) = creds();
    let (uid, gid, eu, eg, uname) = match &who {
        Some(w) => match lookup_user(w) {
            Some(us) => (us.uid, us.gid, us.uid, us.gid, us.name),
            None => { eprintln!("id: '{w}': no such user"); return 1; }
        },
        None => (ruid, rgid, euid, egid, user_name(euid)),
    };
    let fmt = |id: u32, name: bool, grp: bool| if name { if grp { group_name(id) } else { user_name(id) } } else { id.to_string() };
    let ids = supplementary(&uname, eg);
    if u { println!("{}", fmt(if r { uid } else { eu }, n, false)); return 0; }
    if g { println!("{}", fmt(if r { gid } else { eg }, n, true)); return 0; }
    if gs { println!("{}", ids.iter().map(|i| fmt(*i, n, true)).collect::<Vec<_>>().join(" ")); return 0; }
    let mut out = format!("uid={}({}) gid={}({})", uid, user_name(uid), gid, group_name(gid));
    if eu != uid { out += &format!(" euid={}({})", eu, user_name(eu)); }
    if eg != gid { out += &format!(" egid={}({})", eg, group_name(eg)); }
    out += &format!(" groups={}", ids.iter().map(|i| format!("{}({})", i, group_name(*i))).collect::<Vec<_>>().join(","));
    println!("{out}");
    0
}

fn cmd_whoami() -> i32 {
    println!("{}", user_name(creds().2));
    0
}

fn cmd_groups(args: &[String]) -> i32 {
    let (_, _, eu, eg) = creds();
    let name = args.first().cloned().unwrap_or_else(|| user_name(eu));
    let primary = lookup_user(&name).map(|u| u.gid).unwrap_or(eg);
    println!("{}", supplementary(&name, primary).iter().map(|g| group_name(*g)).collect::<Vec<_>>().join(" "));
    0
}

pub fn statx_path(path: &str) -> Result<(u32, u32, u32, u32, u64), i32> {
    let mut o = [0u32; 6];
    let r = unsafe { statx(path.as_ptr(), path.len(), o.as_mut_ptr()) };
    if r < 0 { Err(-r) } else { Ok((o[0], o[1], o[2], o[3], o[4] as u64 | (o[5] as u64) << 32)) }
}

fn errmsg(e: i32) -> &'static str {
    match e { 2 => "No such file or directory", 1 => "Operation not permitted", 13 => "Permission denied", 20 => "Not a directory", _ => "Input/output error" }
}

fn mode_string(mode: u32, kind: u32) -> String {
    let mut s = String::new();
    s.push(match kind { 1 => 'd', 2 => 'l', _ => '-' });
    for (i, c) in "rwxrwxrwx".chars().enumerate() {
        let on = mode & (0o400 >> i) != 0;
        let special = match i { 2 => mode & 0o4000 != 0, 5 => mode & 0o2000 != 0, 8 => mode & 0o1000 != 0, _ => false };
        s.push(match (special, on, i) {
            (true, true, 8) => 't', (true, false, 8) => 'T',
            (true, true, _) => 's', (true, false, _) => 'S',
            (false, true, _) => c, _ => '-',
        });
    }
    s
}

fn parse_mode(spec: &str, cur: u32) -> Option<u32> {
    if let Ok(m) = u32::from_str_radix(spec, 8) { return (m <= 0o7777).then_some(m); }
    let mut mode = cur & 0o7777;
    for clause in spec.split(',') {
        let mut ch = clause.chars().peekable();
        let mut who = 0u32;
        while let Some(c) = ch.peek() {
            match c { 'u' => who |= 0o4700, 'g' => who |= 0o2070, 'o' => who |= 0o1007, 'a' => who |= 0o7777, _ => break }
            ch.next();
        }
        if who == 0 { who = 0o0777; }
        let op = ch.next()?;
        let mut bits = 0u32;
        for c in ch {
            bits |= match c {
                'r' => 0o444, 'w' => 0o222, 'x' => 0o111, 's' => 0o6000, 't' => 0o1000,
                'X' => if cur & 0o111 != 0 { 0o111 } else { 0 },
                _ => return None,
            };
        }
        bits &= who;
        match op { '+' => mode |= bits, '-' => mode &= !bits, '=' => mode = (mode & !who) | bits, _ => return None }
    }
    Some(mode)
}

fn cmd_chmod(args: &[String]) -> i32 {
    let a: Vec<&String> = args.iter().filter(|s| !(s.starts_with('-') && s.len() > 1 && !s.starts_with("-w") && !s.starts_with("-x") && !s.starts_with("-r"))).collect();
    if a.len() < 2 { eprintln!("chmod: missing operand"); return 1; }
    let mut rc = 0;
    for f in &a[1..] {
        match statx_path(f) {
            Err(e) => { eprintln!("chmod: cannot access '{f}': {}", errmsg(e)); rc = 1; }
            Ok((mode, ..)) => match parse_mode(a[0], mode) {
                None => { eprintln!("chmod: invalid mode: '{}'", a[0]); return 1; }
                Some(m) => {
                    if unsafe { chmod(f.as_ptr(), f.len(), m) } < 0 { eprintln!("chmod: changing permissions of '{f}': Operation not permitted"); rc = 1; }
                }
            },
        }
    }
    rc
}

fn cmd_chown(args: &[String], group_only: bool) -> i32 {
    let a: Vec<&String> = args.iter().filter(|s| !s.starts_with('-')).collect();
    let name = if group_only { "chgrp" } else { "chown" };
    if a.len() < 2 { eprintln!("{name}: missing operand"); return 1; }
    let (us, gs) = if group_only { ("", a[0].as_str()) } else { a[0].split_once(':').or_else(|| a[0].split_once('.')).map(|(u, g)| (u, g)).unwrap_or((a[0].as_str(), "")) };
    let uid = if us.is_empty() { None } else { match lookup_user(us) { Some(u) => Some(u.uid), None => { eprintln!("{name}: invalid user: '{}'", a[0]); return 1; } } };
    let gid = if gs.is_empty() { None } else {
        match groups_db().into_iter().find(|g| g.name == gs || gs.parse::<u32>().is_ok_and(|n| n == g.gid)) {
            Some(g) => Some(g.gid), None => { eprintln!("{name}: invalid group: '{}'", a[0]); return 1; }
        }
    };
    let mut rc = 0;
    for f in &a[1..] {
        match statx_path(f) {
            Err(e) => { eprintln!("{name}: cannot access '{f}': {}", errmsg(e)); rc = 1; }
            Ok((_, cu, cg, ..)) => {
                if unsafe { chown(f.as_ptr(), f.len(), uid.unwrap_or(cu), gid.unwrap_or(cg)) } < 0 {
                    eprintln!("{name}: changing ownership of '{f}': Operation not permitted");
                    rc = 1;
                }
            }
        }
    }
    rc
}

fn cmd_stat(args: &[String]) -> i32 {
    let mut rc = 0;
    for f in args.iter().filter(|s| !s.starts_with('-')) {
        match statx_path(f) {
            Err(e) => { eprintln!("stat: cannot statx '{f}': {}", errmsg(e)); rc = 1; }
            Ok((mode, uid, gid, kind, size)) => {
                let k = match kind { 1 => "directory", 2 => "symbolic link", _ => "regular file" };
                println!("  File: {f}\n  Size: {size:<10}\t{k}");
                println!("Access: ({:04o}/{})  Uid: ({:5}/{:>8})   Gid: ({:5}/{:>8})", mode & 0o7777, mode_string(mode, kind), uid, user_name(uid), gid, group_name(gid));
            }
        }
    }
    rc
}

fn run_child(prog: &str, argv: &[String], env: &[String], cwd: &str) -> i32 {
    let j = |v: &[String]| { let mut b = Vec::new(); for s in v { b.extend_from_slice(s.as_bytes()); b.push(0); } b };
    let (a, e) = (j(argv), j(env));
    let pid = unsafe { spawn(prog.as_ptr(), prog.len(), a.as_ptr(), a.len(), e.as_ptr(), e.len(), cwd.as_ptr(), cwd.len(), 0, 1, 2) };
    if pid < 0 {
        eprintln!("{}: command not found", prog);
        return 127;
    }
    unsafe { run(pid as u32); wait(pid as u32) }
}

fn env_with(user: &User, login: bool, extra: &[(&str, String)]) -> (Vec<String>, String) {
    let mut env: Vec<(String, String)> = std::env::vars().collect();
    let mut set = |k: &str, v: String| { env.retain(|(a, _)| a != k); env.push((k.to_string(), v)); };
    set("USER", user.name.clone());
    set("LOGNAME", user.name.clone());
    set("HOME", user.home.clone());
    set("SHELL", user.shell.clone());
    let mut cwd = std::env::var("PWD").unwrap_or_else(|_| "/".into());
    if login { cwd = user.home.clone(); set("PWD", cwd.clone()); }
    for (k, v) in extra { set(k, v.clone()); }
    (env.into_iter().map(|(k, v)| format!("{k}={v}")).collect(), cwd)
}

fn enter_shell(user: &User, env: &[String], cwd: &str) {
    let ps1 = if user.uid == 0 { "PS1=\\[\\e[1;31m\\]\\u@mavex\\[\\e[0m\\]:\\[\\e[1;34m\\]\\w\\[\\e[0m\\]# " } else { "PS1=\\[\\e[1;32m\\]\\u@mavex\\[\\e[0m\\]:\\[\\e[1;34m\\]\\w\\[\\e[0m\\]\\$ " };
    let mut env: Vec<String> = env.iter().filter(|e| !e.starts_with("PS1=")).cloned().collect();
    env.push(ps1.to_string());
    let e: Vec<u8> = env.iter().flat_map(|s| s.bytes().chain(std::iter::once(0))).collect();
    unsafe { push_shell(cwd.as_ptr(), cwd.len(), e.as_ptr(), e.len()) };
}

struct Rule { allow_nopasswd: bool, runas_any: bool, runas: Vec<String>, cmds_all: bool, cmds: Vec<String> }

fn sudoers_for(user: &str, gids: &[u32]) -> Result<Vec<Rule>, String> {
    let text = std::fs::read_to_string("/etc/sudoers").map_err(|_| "unable to open /etc/sudoers".to_string())?;
    let gnames: Vec<String> = gids.iter().map(|g| group_name(*g)).collect();
    let mut rules = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let Some((who, rest)) = line.split_once(char::is_whitespace) else { continue };
        let matches = who == "ALL" || who == user || who.strip_prefix('%').is_some_and(|g| gnames.iter().any(|n| n == g));
        if !matches { continue; }
        let Some((_host, spec)) = rest.trim().split_once('=') else { continue };
        let spec = spec.trim();
        let (runas, mut cmds) = if let Some(r) = spec.strip_prefix('(') {
            let (inside, after) = r.split_once(')').unwrap_or((r, ""));
            (inside.split(':').next().unwrap_or("ALL").trim().to_string(), after.trim())
        } else { ("root".to_string(), spec) };
        let mut nopasswd = false;
        while let Some((tag, after)) = cmds.split_once(':') {
            if tag.trim().chars().all(|c| c.is_ascii_uppercase()) && !tag.trim().is_empty() {
                if tag.trim() == "NOPASSWD" { nopasswd = true; }
                cmds = after.trim();
            } else { break; }
        }
        let list: Vec<String> = cmds.split(',').map(|s| s.trim().to_string()).collect();
        rules.push(Rule {
            allow_nopasswd: nopasswd,
            runas_any: runas == "ALL",
            runas: runas.split(',').map(|s| s.trim().to_string()).collect(),
            cmds_all: list.iter().any(|c| c == "ALL"),
            cmds: list,
        });
    }
    Ok(rules)
}

fn cmd_sudo(args: &[String]) -> i32 {
    let (ruid, _, _, _) = creds();
    let me = user_name(ruid);
    let mut target = "root".to_string();
    let (mut login, mut shell, mut list) = (false, false, false);
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-u" | "--user" => { i += 1; target = args.get(i).cloned().unwrap_or_default(); }
            "-i" | "--login" => login = true,
            "-s" | "--shell" => shell = true,
            "-l" | "--list" => list = true,
            "-n" | "-v" | "-k" | "-K" | "-H" | "-E" | "-S" | "-b" => {}
            "--" => { i += 1; break; }
            s if s.starts_with('-') && s.len() > 1 => { eprintln!("sudo: unrecognized option '{s}'"); return 1; }
            _ => break,
        }
        i += 1;
    }
    let cmd = &args[i.min(args.len())..];
    let Some(tu) = lookup_user(&target) else { eprintln!("sudo: unknown user {target}"); return 1; };
    let Some(mu) = lookup_user(&me) else { eprintln!("sudo: you do not exist in the passwd database"); return 1; };
    let rules = match sudoers_for(&me, &supplementary(&me, mu.gid)) {
        Ok(r) => r,
        Err(e) => { eprintln!("sudo: {e}"); return 1; }
    };
    if list {
        if rules.is_empty() { println!("User {me} is not allowed to run sudo on mavex."); return 1; }
        println!("User {me} may run the following commands on mavex:");
        for r in &rules { println!("    ({}) {}{}", if r.runas_any { "ALL : ALL".into() } else { r.runas.join(", ") }, if r.allow_nopasswd { "NOPASSWD: " } else { "" }, r.cmds.join(", ")); }
        return 0;
    }
    let prog_base = cmd.first().map(|c| c.rsplit('/').next().unwrap_or(c).to_string());
    let rule = rules.iter().rev().find(|r| {
        (r.runas_any || r.runas.iter().any(|x| *x == tu.name)) && (r.cmds_all || prog_base.as_ref().is_some_and(|b| r.cmds.iter().any(|c| c.rsplit('/').next() == Some(b.as_str()))) || (cmd.is_empty() && r.cmds_all))
    });
    let Some(rule) = rule else {
        if rules.is_empty() { eprintln!("{me} is not in the sudoers file."); } else { eprintln!("Sorry, user {me} is not allowed to execute '{}' as {} on mavex.", cmd.join(" "), tu.name); }
        return 1;
    };
    if ruid != 0 && !rule.allow_nopasswd {
        eprintln!("sudo: a password is required (interactive password prompts are not supported yet)");
        return 1;
    }
    if unsafe { setcreds(tu.uid, tu.gid) } < 0 { eprintln!("sudo: effective uid is not 0, is /bin/sudo setuid root?"); return 1; }
    let extra = [("SUDO_USER", me.clone()), ("SUDO_UID", ruid.to_string()), ("SUDO_GID", mu.gid.to_string()), ("SUDO_COMMAND", cmd.join(" "))];
    let (env, cwd) = env_with(&tu, login, &extra);
    if cmd.is_empty() {
        if !(login || shell) { eprintln!("usage: sudo [-u user] [-i|-s] [--] command [args]"); return 1; }
        enter_shell(&tu, &env, &cwd);
        return 0;
    }
    if login || shell {
        let line = cmd.join(" ");
        return run_child("brush", &["brush".into(), "-c".into(), line], &env, &cwd);
    }
    run_child(&cmd[0], cmd, &env, &cwd)
}

fn cmd_su(args: &[String]) -> i32 {
    let (ruid, _, _, _) = creds();
    let mut login = false;
    let mut target = "root".to_string();
    let mut command = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-" | "-l" | "--login" => login = true,
            "-c" | "--command" => { i += 1; command = args.get(i).cloned(); }
            "-m" | "-p" | "-s" => {}
            s if s.starts_with('-') && s.len() > 1 => { eprintln!("su: unrecognized option '{s}'"); return 1; }
            s => target = s.to_string(),
        }
        i += 1;
    }
    let Some(tu) = lookup_user(&target) else { eprintln!("su: user {target} does not exist or the user entry does not contain all the required fields"); return 1; };
    if ruid != 0 {
        let shadow = std::fs::read_to_string("/etc/shadow").unwrap_or_default();
        let field = shadow.lines().find_map(|l| { let f: Vec<&str> = l.split(':').collect(); (f.first() == Some(&tu.name.as_str())).then(|| f.get(1).copied().unwrap_or("!").to_string()) }).unwrap_or_else(|| "!".into());
        if !field.is_empty() {
            eprintln!("su: Authentication failure");
            return 1;
        }
    }
    if unsafe { setcreds(tu.uid, tu.gid) } < 0 { eprintln!("su: must be setuid root"); return 1; }
    let (env, cwd) = env_with(&tu, login, &[]);
    match command {
        Some(c) => run_child("brush", &["brush".into(), "-c".into(), c], &env, &cwd),
        None => { enter_shell(&tu, &env, &cwd); 0 }
    }
}

fn main() {
    if let Ok(pwd) = std::env::var("PWD") {
        let _ = std::env::set_current_dir(pwd);
    }
    let argv: Vec<String> = std::env::args().collect();
    let name = argv.first().map(|a| a.rsplit('/').next().unwrap_or(a).to_string()).unwrap_or_default();
    let (name, args) = if name == "mavexutils" && argv.len() > 1 { (argv[1].clone(), argv[2..].to_vec()) } else { (name, argv[1.min(argv.len())..].to_vec()) };
    let rc = match name.as_str() {
        "id" => cmd_id(&args),
        "whoami" => cmd_whoami(),
        "groups" => cmd_groups(&args),
        "chmod" => cmd_chmod(&args),
        "chown" => cmd_chown(&args, false),
        "chgrp" => cmd_chown(&args, true),
        "stat" => cmd_stat(&args),
        "sudo" => cmd_sudo(&args),
        "su" => cmd_su(&args),
        "ip" => ip::main(&args),
        "wg" => wg::wg(&args),
        "wg-quick" => wg::wg_quick(&args),
        "uname" => sysutils::uname(&args),
        "hostname" => sysutils::hostname(&args),
        "clear" => sysutils::clear(),
        "uptime" => sysutils::uptime(&args),
        "free" => sysutils::free(&args),
        "nproc" => sysutils::nproc(),
        "which" => sysutils::which(&args),
        "top" | "htop" => top::run(&name, &args),
        _ => { eprintln!("mavexutils: unknown applet '{name}'"); 127 }
    };
    exit(rc);
}

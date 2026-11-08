//! Passwords: /etc/shadow hashes (salted, iterated SHA-256 — not crypt(3)), echo-off prompts, `passwd`.
use sha2::{Digest, Sha256};
use std::io::{BufRead, IsTerminal, Write};

#[link(wasm_import_module = "mavex")]
unsafe extern "C" {
    fn tty_getattr() -> i32;
    fn tty_setattr(mask: i32) -> i32;
}
const ECHO: i32 = 2;
const ROUNDS: u32 = 5000;

fn hex(b: &[u8]) -> String { b.iter().map(|x| format!("{x:02x}")).collect() }

fn digest(salt: &str, pw: &str) -> String {
    let mut h = Sha256::digest([salt.as_bytes(), pw.as_bytes()].concat());
    for _ in 0..ROUNDS { h = Sha256::digest([&h[..], salt.as_bytes(), pw.as_bytes()].concat()); }
    hex(&h)
}

pub fn hash(pw: &str) -> String {
    let mut s = [0u8; 8];
    getrandom::getrandom(&mut s).expect("random");
    let salt = hex(&s);
    format!("$mx1${salt}${}", digest(&salt, pw))
}

pub fn shadow_field(name: &str) -> Option<String> {
    let t = std::fs::read_to_string("/etc/shadow").ok()?;
    t.lines().find_map(|l| { let f: Vec<&str> = l.split(':').collect(); (f.first() == Some(&name)).then(|| f.get(1).copied().unwrap_or("!").to_string()) })
}

pub fn is_locked(f: &str) -> bool { f.starts_with('!') || f.starts_with('*') }

pub fn verify(field: &str, pw: &str) -> bool {
    let mut it = field.split('$');
    if it.next() != Some("") || it.next() != Some("mx1") { return false; }
    let (Some(salt), Some(h)) = (it.next(), it.next()) else { return false };
    digest(salt, pw) == h
}

pub fn read_password(prompt: &str) -> Option<String> {
    let tty = std::io::stdin().is_terminal();
    let old = unsafe { tty_getattr() };
    let mut e = std::io::stderr();
    let _ = write!(e, "{prompt}");
    let _ = e.flush();
    if tty { unsafe { tty_setattr(old & !ECHO) }; }
    let mut line = String::new();
    let n = std::io::stdin().lock().read_line(&mut line).ok();
    if tty { unsafe { tty_setattr(old) }; let _ = writeln!(e); }
    if n.unwrap_or(0) == 0 { return None; }
    Some(line.trim_end_matches(['\n', '\r']).to_string())
}

fn write_shadow(name: &str, newfield: &str) -> Result<(), String> {
    let t = std::fs::read_to_string("/etc/shadow").map_err(|e| e.to_string())?;
    let mut found = false;
    let mut out = String::new();
    for l in t.lines() {
        let mut f: Vec<String> = l.split(':').map(String::from).collect();
        if f.first().map(|s| s.as_str()) == Some(name) { f[1] = newfield.to_string(); found = true; }
        out.push_str(&f.join(":")); out.push('\n');
    }
    if !found { out.push_str(&format!("{name}:{newfield}:19000:0:99999:7:::\n")); }
    std::fs::write("/etc/shadow", out).map_err(|e| e.to_string())
}

pub fn passwd(args: &[String]) -> i32 {
    let (ruid, _, euid, _) = crate::creds();
    let me = crate::user_name(ruid);
    let mut target = me.clone();
    let mut mode = "";
    for a in args {
        match a.as_str() {
            "-d" | "--delete" => mode = "delete",
            "-l" | "--lock" => mode = "lock",
            "-u" | "--unlock" => mode = "unlock",
            s if s.starts_with('-') => { eprintln!("passwd: unrecognized option '{s}'"); return 1; }
            s => target = s.to_string(),
        }
    }
    if crate::lookup_user(&target).is_none() { eprintln!("passwd: user '{target}' does not exist"); return 1; }
    if euid != 0 { eprintln!("passwd: Permission denied (passwd must be setuid root)"); return 1; }
    if ruid != 0 && target != me { eprintln!("passwd: You may not view or modify password information for {target}."); return 1; }
    if !mode.is_empty() {
        if ruid != 0 { eprintln!("passwd: Permission denied."); return 1; }
        let cur = shadow_field(&target).unwrap_or_else(|| "*".into());
        let nf = match mode { "delete" => String::new(), "lock" => if is_locked(&cur) { cur } else { format!("!{cur}") }, _ => cur.trim_start_matches('!').to_string() };
        return match write_shadow(&target, &nf) { Ok(_) => { println!("passwd: password expiry information changed."); 0 } Err(e) => { eprintln!("passwd: {e}"); 1 } };
    }
    if ruid != 0 {
        let cur = shadow_field(&target).unwrap_or_else(|| "*".into());
        if !cur.is_empty() && !is_locked(&cur) {
            let Some(p) = read_password("Current password: ") else { return 1 };
            if !verify(&cur, &p) { eprintln!("passwd: Authentication failure"); return 1; }
        }
    }
    println!("Changing password for {target}.");
    let Some(a) = read_password("New password: ") else { return 1 };
    let Some(b) = read_password("Retype new password: ") else { return 1 };
    if a != b { eprintln!("Sorry, passwords do not match.\npasswd: Authentication token manipulation error\npasswd: password unchanged"); return 10; }
    if a.is_empty() { eprintln!("passwd: password unchanged (empty password)"); return 10; }
    match write_shadow(&target, &hash(&a)) { Ok(_) => { println!("passwd: password updated successfully"); 0 } Err(e) => { eprintln!("passwd: {e}"); 1 } }
}

/// Authenticates `name` (tries prompts); true on success. Empty field = no password.
pub fn authenticate(name: &str, prompt: &str, tries: u32) -> bool {
    let f = shadow_field(name).unwrap_or_else(|| "!".into());
    if f.is_empty() { return true; }
    for t in 0..tries {
        let ok = match read_password(prompt) { Some(p) => !is_locked(&f) && verify(&f, &p), None => return false };
        if ok { return true; }
        if t + 1 < tries { eprintln!("Sorry, try again."); }
    }
    false
}

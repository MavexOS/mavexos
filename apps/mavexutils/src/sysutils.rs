//! uname, hostname, clear, uptime, free, nproc, which, neofetch.
use std::io::IsTerminal;

#[link(wasm_import_module = "mavex")]
unsafe extern "C" {
    fn sysinfo(out: *mut u32) -> i32;
}

pub const KERNEL_NAME: &str = "MavexOS";
pub const KERNEL_RELEASE: &str = env!("CARGO_PKG_VERSION");
pub const KERNEL_VERSION: &str = concat!("#1 MavexOS ", env!("CARGO_PKG_VERSION"));
pub const MACHINE: &str = "wasm32";

struct Info { uptime: u32, mem_total_mib: u32, mem_used_mib: u32, cpus: u32 }

fn info() -> Info {
    let mut o = [0u32; 5];
    unsafe { sysinfo(o.as_mut_ptr()) };
    Info { uptime: o[0], mem_total_mib: o[1], mem_used_mib: o[2], cpus: o[3].max(1) }
}

pub fn hostname_str() -> String {
    std::fs::read_to_string("/etc/hostname").map(|s| s.trim().to_string()).ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "mavex".into())
}

fn os_release(key: &str) -> Option<String> {
    std::fs::read_to_string("/etc/os-release").ok()?.lines().find_map(|l| l.strip_prefix(&format!("{key}=")).map(|v| v.trim_matches('"').to_string()))
}

pub fn uname(args: &[String]) -> i32 {
    let (mut s, mut n, mut r, mut v, mut m, mut o) = (false, false, false, false, false, false);
    let mut any = false;
    for a in args {
        match a.as_str() {
            "--all" => { s = true; n = true; r = true; v = true; m = true; o = true; any = true; }
            "--kernel-name" => { s = true; any = true; }
            "--nodename" => { n = true; any = true; }
            "--kernel-release" => { r = true; any = true; }
            "--kernel-version" => { v = true; any = true; }
            "--machine" | "--processor" | "--hardware-platform" => { m = true; any = true; }
            "--operating-system" => { o = true; any = true; }
            x if x.starts_with('-') && x.len() > 1 && !x.starts_with("--") => {
                for c in x[1..].chars() {
                    match c {
                        'a' => { s = true; n = true; r = true; v = true; m = true; o = true; }
                        's' => s = true, 'n' => n = true, 'r' => r = true, 'v' => v = true,
                        'm' | 'p' | 'i' => m = true, 'o' => o = true,
                        _ => { eprintln!("uname: invalid option -- '{c}'"); return 1; }
                    }
                }
                any = true;
            }
            x => { eprintln!("uname: extra operand '{x}'"); return 1; }
        }
    }
    if !any { s = true; }
    let mut parts: Vec<String> = Vec::new();
    if s { parts.push(KERNEL_NAME.into()); }
    if n { parts.push(hostname_str()); }
    if r { parts.push(KERNEL_RELEASE.into()); }
    if v { parts.push(KERNEL_VERSION.into()); }
    if m { parts.push(MACHINE.into()); }
    if o { parts.push(KERNEL_NAME.into()); }
    println!("{}", parts.join(" "));
    0
}

pub fn hostname(args: &[String]) -> i32 {
    let names: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
    if let Some(n) = names.first() {
        if crate::creds().2 != 0 { eprintln!("hostname: you must be root to change the host name"); return 1; }
        if std::fs::write("/etc/hostname", format!("{n}\n")).is_err() { eprintln!("hostname: sethostname: Operation not permitted"); return 1; }
        return 0;
    }
    let h = hostname_str();
    if args.iter().any(|a| a == "-s" || a == "--short") { println!("{}", h.split('.').next().unwrap_or(&h)); } else { println!("{h}"); }
    0
}

pub fn clear() -> i32 {
    print!("\x1b[H\x1b[2J\x1b[3J");
    0
}

fn fmt_dur(secs: u32) -> String {
    let (d, h, m) = (secs / 86400, secs / 3600 % 24, secs / 60 % 60);
    let mut v = Vec::new();
    if d > 0 { v.push(format!("{d} day{}", if d == 1 { "" } else { "s" })); }
    if h > 0 { v.push(format!("{h} hour{}", if h == 1 { "" } else { "s" })); }
    v.push(format!("{m} min{}", if m == 1 { "" } else { "s" }));
    v.join(", ")
}

pub fn uptime(args: &[String]) -> i32 {
    let i = info();
    if args.iter().any(|a| a == "-p" || a == "--pretty") { println!("up {}", fmt_dur(i.uptime)); return 0; }
    println!(" up {},  1 user,  load average: 0.00, 0.00, 0.00", fmt_dur(i.uptime));
    0
}

pub fn free(args: &[String]) -> i32 {
    let i = info();
    let human = args.iter().any(|a| a == "-h" || a == "--human");
    let mb = args.iter().any(|a| a == "-m");
    let f = |mib: u32| -> String {
        if human { if mib >= 1024 { format!("{:.1}Gi", mib as f64 / 1024.0) } else { format!("{mib}Mi") } }
        else if mb { mib.to_string() } else { (mib as u64 * 1024).to_string() }
    };
    let free = i.mem_total_mib.saturating_sub(i.mem_used_mib);
    println!("{:<8}{:>12}{:>12}{:>12}", "", "total", "used", "free");
    println!("{:<8}{:>12}{:>12}{:>12}", "Mem:", f(i.mem_total_mib), f(i.mem_used_mib), f(free));
    println!("{:<8}{:>12}{:>12}{:>12}", "Swap:", f(0), f(0), f(0));
    0
}

pub fn nproc() -> i32 {
    println!("{}", info().cpus);
    0
}

pub fn which(args: &[String]) -> i32 {
    let path = std::env::var("PATH").unwrap_or_else(|_| "/bin".into());
    let mut rc = 0;
    for a in args.iter().filter(|a| !a.starts_with('-')) {
        let found = if a.contains('/') { std::fs::metadata(a).ok().map(|_| a.clone()) } else { path.split(':').map(|d| format!("{d}/{a}")).find(|p| std::fs::symlink_metadata(p).is_ok()) };
        match found { Some(p) => println!("{p}"), None => rc = 1 }
    }
    rc
}

pub fn neofetch(args: &[String]) -> i32 {
    let color = !args.iter().any(|a| a == "--off" || a == "--stdout") && (std::io::stdout().is_terminal() || args.iter().any(|a| a == "--color"));
    let c = |code: &str, s: &str| if color { format!("\x1b[{code}m{s}\x1b[0m") } else { s.to_string() };
    let i = info();
    let (user, host) = (crate::user_name(crate::creds().2), hostname_str());
    let os = os_release("PRETTY_NAME").unwrap_or_else(|| format!("{} {}", KERNEL_NAME, KERNEL_RELEASE));
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/brush".into());
    let pkgs = std::fs::read_dir("/bin").map(|d| d.count()).unwrap_or(0);
    let title = format!("{}@{}", c("1;36", &user), c("1;36", &host));
    let rows: Vec<(String, String)> = vec![
        (title, String::new()),
        (c("36", &"-".repeat(user.len() + host.len() + 1)), String::new()),
        (c("1;36", "OS"), format!("{os} {MACHINE}")),
        (c("1;36", "Host"), "Browser tab (wasm sandbox)".into()),
        (c("1;36", "Kernel"), format!("{KERNEL_NAME} {KERNEL_RELEASE}")),
        (c("1;36", "Uptime"), fmt_dur(i.uptime)),
        (c("1;36", "Packages"), format!("{pkgs} (bin)")),
        (c("1;36", "Shell"), shell.rsplit('/').next().unwrap_or(&shell).to_string()),
        (c("1;36", "Terminal"), "mavex tty1".into()),
        (c("1;36", "CPU"), format!("wasm32 virtual ({})", i.cpus)),
        (c("1;36", "Memory"), format!("{}MiB / {}MiB", i.mem_used_mib, i.mem_total_mib)),
        (String::new(), String::new()),
        ((30..38).map(|n| c(&format!("4{}", n - 30), "   ")).collect::<String>(), String::new()),
        ((90..98).map(|n| c(&format!("10{}", n - 90), "   ")).collect::<String>(), String::new()),
    ];
    let logo = [
        "      .--.      ",
        "     /    \\     ",
        "    | ^  ^ |    ",
        "    |  ()  |    ",
        "   /| M  X |\\   ",
        "  / |______| \\  ",
        " /__/  /\\  \\__\\ ",
        "      /__\\      ",
    ];
    for (n, (k, v)) in rows.iter().enumerate() {
        let l = logo.get(n).copied().unwrap_or("                ");
        let line = if v.is_empty() { k.clone() } else { format!("{}: {}", k, v) };
        println!("{} {}", c("1;36", l), line);
    }
    0
}

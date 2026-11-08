//! `stty` and `tty` over the MavexOS tty layer (flags: icanon, echo, isig).
use std::io::IsTerminal;

#[link(wasm_import_module = "mavex")]
unsafe extern "C" {
    fn tty_getattr() -> i32;
    fn tty_setattr(mask: i32) -> i32;
    fn tty_size(out: *mut u32) -> i32;
    fn tty_name() -> i32;
}

const ICANON: i32 = 1;
const ECHO: i32 = 2;
const ISIG: i32 = 4;

fn size() -> (u32, u32) {
    let mut o = [0u32; 2];
    unsafe { tty_size(o.as_mut_ptr()) };
    (o[1], o[0])
}

pub fn tty(_args: &[String]) -> i32 {
    if std::io::stdin().is_terminal() { println!("/dev/tty{}", unsafe { tty_name() }); 0 } else { println!("not a tty"); 1 }
}

fn flag(name: &str) -> Option<i32> {
    match name { "icanon" => Some(ICANON), "echo" => Some(ECHO), "isig" => Some(ISIG), _ => None }
}

fn show(m: i32, all: bool) {
    let (rows, cols) = size();
    println!("speed 38400 baud; rows {rows}; columns {cols};");
    let f = |bit: i32, n: &str| format!("{}{n}", if m & bit != 0 { "" } else { "-" });
    println!("{} {} {}", f(ISIG, "isig"), f(ICANON, "icanon"), f(ECHO, "echo"));
    if all { println!("intr = ^C; eof = ^D; susp = ^Z; erase = ^?;"); }
}

pub fn stty(args: &[String]) -> i32 {
    if !std::io::stdin().is_terminal() {
        eprintln!("stty: 'standard input': Inappropriate ioctl for device");
        return 1;
    }
    let mut m = unsafe { tty_getattr() };
    if args.is_empty() { show(m, false); return 0; }
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "-a" | "--all" => { show(m, true); return 0; }
            "-g" | "--save" => { println!("mavex:{m}"); return 0; }
            "size" => { let (r, c) = size(); println!("{r} {c}"); return 0; }
            "raw" => m = 0,
            "-raw" | "cooked" => m |= ICANON | ISIG | ECHO,
            "sane" => m = ICANON | ECHO | ISIG,
            "cbreak" => m = (m & !ICANON) | ISIG,
            "-cbreak" => m |= ICANON,
            _ if a.starts_with("mavex:") => match a[6..].parse::<i32>() { Ok(v) => m = v & 7, Err(_) => { eprintln!("stty: invalid argument '{a}'"); return 1; } },
            _ => {
                let (neg, n) = match a.strip_prefix('-') { Some(n) => (true, n), None => (false, a) };
                match flag(n) {
                    Some(bit) => if neg { m &= !bit } else { m |= bit },
                    None if matches!(n, "rows" | "cols" | "columns" | "speed") => { i += 1; }
                    None => { eprintln!("stty: invalid argument '{a}'"); return 1; }
                }
            }
        }
        i += 1;
    }
    unsafe { tty_setattr(m) };
    0
}

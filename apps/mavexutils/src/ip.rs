//! `ip`: a MavexOS reimplementation of an iproute2 subset (not the official iproute2) over the virtual netns.
use crate::net_request;

struct Link { idx: u32, name: String, up: bool, mtu: u32, kind: String, mac: String }
struct Addr { idx: u32, fam: u8, addr: String, plen: u32, scope: String }
struct Route { fam: u8, dst: String, plen: u32, gw: String, dev: String, proto: String, scope: String, src: String }

fn dump(w: &str) -> Result<Vec<Vec<String>>, i32> {
    match net_request(w) {
        Ok(s) => Ok(s.lines().map(|l| l.split('\t').map(String::from).collect()).collect()),
        Err((_, m)) => { eprintln!("{m}"); Err(2) }
    }
}

fn links() -> Result<Vec<Link>, i32> {
    Ok(dump("link")?.into_iter().filter(|f| f.len() >= 7).map(|f| Link { idx: f[1].parse().unwrap_or(0), name: f[2].clone(), up: f[3] == "up", mtu: f[4].parse().unwrap_or(0), kind: f[5].clone(), mac: f[6].clone() }).collect())
}
fn addrs() -> Result<Vec<Addr>, i32> {
    Ok(dump("addr")?.into_iter().filter(|f| f.len() >= 6).map(|f| Addr { idx: f[1].parse().unwrap_or(0), fam: f[2].parse().unwrap_or(4), addr: f[3].clone(), plen: f[4].parse().unwrap_or(0), scope: f[5].clone() }).collect())
}
fn routes() -> Result<Vec<Route>, i32> {
    Ok(dump("route")?.into_iter().filter(|f| f.len() >= 9).map(|f| Route { fam: f[1].parse().unwrap_or(4), dst: f[2].clone(), plen: f[3].parse().unwrap_or(0), gw: f[4].clone(), dev: f[5].clone(), proto: f[6].clone(), scope: f[7].clone(), src: f[8].clone() }).collect())
}

fn flags(l: &Link) -> String {
    let mut f = if l.kind == "loopback" { vec!["LOOPBACK"] } else { vec!["BROADCAST", "NOARP"] };
    if l.up { f.push("UP"); f.push("LOWER_UP"); }
    f.join(",")
}
fn state(l: &Link) -> &'static str { if !l.up { "DOWN" } else if l.kind == "loopback" || l.kind == "dummy" { "UNKNOWN" } else { "UP" } }
fn j(s: &str) -> String { s.replace('\\', "\\\\").replace('"', "\\\"") }

fn link_lines(l: &Link) -> String {
    let (lt, brd) = if l.kind == "loopback" { ("loopback", "00:00:00:00:00:00") } else { ("ether", "ff:ff:ff:ff:ff:ff") };
    format!("{}: {}: <{}> mtu {} qdisc {} state {} mode DEFAULT group default qlen 1000\n    link/{} {} brd {}\n", l.idx, l.name, flags(l), l.mtu, if l.kind == "loopback" { "noqueue" } else { "noop" }, state(l), lt, l.mac, brd)
}

fn show_link(args: &[String], br: bool, json: bool) -> i32 {
    let dev = dev_filter(args);
    let Ok(ls) = links() else { return 1 };
    let ls: Vec<&Link> = ls.iter().filter(|l| dev.as_ref().is_none_or(|d| *d == l.name)).collect();
    if let Some(d) = &dev { if ls.is_empty() { eprintln!("Device \"{d}\" does not exist."); return 1; } }
    if json {
        let items: Vec<String> = ls.iter().map(|l| format!("{{\"ifindex\":{},\"ifname\":\"{}\",\"flags\":[{}],\"mtu\":{},\"operstate\":\"{}\",\"link_type\":\"{}\",\"address\":\"{}\"}}", l.idx, j(&l.name), flags(l).split(',').map(|f| format!("\"{f}\"")).collect::<Vec<_>>().join(","), l.mtu, state(l), if l.kind == "loopback" { "loopback" } else { "ether" }, l.mac)).collect();
        println!("[{}]", items.join(","));
    } else if br {
        for l in ls { println!("{:<16} {:<14} {} <{}>", l.name, state(l), l.mac, flags(l)); }
    } else {
        for l in ls { print!("{}", link_lines(l)); }
    }
    0
}

fn show_addr(args: &[String], br: bool, json: bool, fam: Option<u8>) -> i32 {
    let dev = dev_filter(args);
    let (Ok(ls), Ok(ad)) = (links(), addrs()) else { return 1 };
    let ls: Vec<&Link> = ls.iter().filter(|l| dev.as_ref().is_none_or(|d| *d == l.name)).collect();
    if let Some(d) = &dev { if ls.is_empty() { eprintln!("Device \"{d}\" does not exist."); return 1; } }
    if json {
        let items: Vec<String> = ls.iter().map(|l| {
            let a: Vec<String> = ad.iter().filter(|a| a.idx == l.idx && fam.is_none_or(|f| f == a.fam)).map(|a| format!("{{\"family\":\"{}\",\"local\":\"{}\",\"prefixlen\":{},\"scope\":\"{}\"}}", if a.fam == 4 { "inet" } else { "inet6" }, a.addr, a.plen, a.scope)).collect();
            format!("{{\"ifindex\":{},\"ifname\":\"{}\",\"operstate\":\"{}\",\"mtu\":{},\"address\":\"{}\",\"addr_info\":[{}]}}", l.idx, j(&l.name), state(l), l.mtu, l.mac, a.join(","))
        }).collect();
        println!("[{}]", items.join(","));
        return 0;
    }
    for l in ls {
        let mine: Vec<&Addr> = ad.iter().filter(|a| a.idx == l.idx && fam.is_none_or(|f| f == a.fam)).collect();
        if br {
            let list: Vec<String> = mine.iter().map(|a| format!("{}/{}", a.addr, a.plen)).collect();
            println!("{:<16} {:<14} {}", l.name, state(l), list.join(" "));
            continue;
        }
        print!("{}", link_lines(l));
        for a in mine {
            let dev = &l.name;
            if a.fam == 4 { println!("    inet {}/{} scope {} {}\n       valid_lft forever preferred_lft forever", a.addr, a.plen, a.scope, dev); }
            else { println!("    inet6 {}/{} scope {}\n       valid_lft forever preferred_lft forever", a.addr, a.plen, a.scope); }
        }
    }
    0
}

fn show_route(args: &[String], json: bool, fam: Option<u8>) -> i32 {
    let dev = dev_filter(args);
    let Ok(rs) = routes() else { return 1 };
    let rs: Vec<&Route> = rs.iter().filter(|r| fam.is_none_or(|f| f == r.fam) && dev.as_ref().is_none_or(|d| *d == r.dev)).collect();
    if json {
        let items: Vec<String> = rs.iter().map(|r| format!("{{\"dst\":\"{}\",\"gateway\":\"{}\",\"dev\":\"{}\",\"protocol\":\"{}\",\"scope\":\"{}\"}}", if r.dst == "default" { "default".into() } else { format!("{}/{}", r.dst, r.plen) }, r.gw, j(&r.dev), r.proto, r.scope)).collect();
        println!("[{}]", items.join(","));
        return 0;
    }
    for r in rs {
        let dst = if r.dst == "default" { "default".to_string() } else if (r.fam == 4 && r.plen == 32) || (r.fam == 6 && r.plen == 128) { r.dst.clone() } else { format!("{}/{}", r.dst, r.plen) };
        let mut s = dst;
        if r.gw != "-" { s += &format!(" via {}", r.gw); }
        s += &format!(" dev {}", r.dev);
        if r.proto == "kernel" { s += &format!(" proto kernel scope link src {}", r.src); }
        println!("{s}");
    }
    0
}

fn dev_filter(args: &[String]) -> Option<String> {
    let mut i = 0;
    while i < args.len() {
        if args[i] == "dev" { return args.get(i + 1).cloned(); }
        i += 1;
    }
    args.iter().find(|a| !matches!(a.as_str(), "show" | "list" | "ls" | "dev" | "up" | "down")).cloned()
}

fn kw(args: &[String], key: &str) -> Option<String> {
    args.iter().position(|a| a == key).and_then(|i| args.get(i + 1).cloned())
}

fn send(req: String) -> i32 {
    match net_request(&req) {
        Ok(_) => 0,
        Err((_, m)) => {
            if m.starts_with("Error") || m.starts_with("Cannot") { eprintln!("{m}"); } else { eprintln!("RTNETLINK answers: {m}"); }
            2
        }
    }
}

fn is(a: &str, full: &str, min: usize) -> bool { a.len() >= min && full.starts_with(a) }

pub fn main(args: &[String]) -> i32 {
    let (mut br, mut json, mut fam) = (false, false, None);
    let mut i = 0;
    while i < args.len() && args[i].starts_with('-') && args[i].len() > 1 {
        match args[i].as_str() {
            "-br" | "-brief" => br = true,
            "-j" | "-json" => json = true,
            "-4" => fam = Some(4),
            "-6" => fam = Some(6),
            "-o" | "-s" | "-c" | "-p" | "-d" | "-h" | "-human" | "-color" | "-oneline" | "-stats" => {}
            "-V" | "-Version" => { println!("ip utility, MavexOS reimplementation (iproute2-compatible subset)"); return 0; }
            o => { eprintln!("Option \"{o}\" is unknown, try \"ip -help\"."); return 1; }
        }
        i += 1;
    }
    let rest = &args[i..];
    let Some(obj) = rest.first() else { eprintln!("Usage: ip [ OPTIONS ] OBJECT {{ COMMAND | help }}\nOBJECT := {{ link | address | route }}"); return 1 };
    let sub: Vec<String> = rest[1..].to_vec();
    let verb = sub.first().map(String::as_str).unwrap_or("show");
    let rv = if sub.is_empty() { &[][..] } else { &sub[1..] };
    let show = matches!(verb, "show" | "list" | "ls" | "lst" | "sh") || sub.is_empty();
    let is_show_arg = |s: &String| s == "dev" || !matches!(s.as_str(), "show" | "list" | "ls");
    if is(obj, "link", 1) {
        let a: Vec<String> = if show { if sub.is_empty() { vec![] } else { sub[1.min(sub.len())..].to_vec() } } else { vec![] };
        let _ = is_show_arg;
        if show { return show_link(&a, br, json); }
        match verb {
            "set" => {
                let Some(dev) = kw(rv, "dev").or_else(|| rv.first().cloned()) else { eprintln!("Not enough information: \"dev\" argument is required."); return 1 };
                let mut rc = 0;
                if rv.iter().any(|a| a == "up") { rc |= send(format!("link-set\t{dev}\tup\t-")); }
                if rv.iter().any(|a| a == "down") { rc |= send(format!("link-set\t{dev}\tdown\t-")); }
                if let Some(m) = kw(rv, "mtu") { rc |= send(format!("link-set\t{dev}\tmtu\t{m}")); }
                if let Some(n) = kw(rv, "name") { rc |= send(format!("link-set\t{dev}\tname\t{n}")); }
                rc
            }
            "add" => {
                let name = kw(rv, "name").or_else(|| rv.first().filter(|s| *s != "type").cloned());
                let ty = kw(rv, "type");
                match (name, ty) { (Some(n), Some(t)) => send(format!("link-add\t{n}\t{t}")), _ => { eprintln!("Not enough information: \"type\" argument is required."); 1 } }
            }
            "del" | "delete" => match kw(rv, "dev").or_else(|| rv.first().cloned()) { Some(d) => send(format!("link-del\t{d}")), None => { eprintln!("Not enough information: \"dev\" argument is required."); 1 } },
            _ => { eprintln!("Command \"{verb}\" is unknown, try \"ip link help\"."); 1 }
        }
    } else if is(obj, "address", 1) {
        let a: Vec<String> = if show && !sub.is_empty() { sub[1..].to_vec() } else { vec![] };
        if show { return show_addr(&a, br, json, fam); }
        let Some(dev) = kw(rv, "dev") else { eprintln!("Not enough information: \"dev\" argument is required."); return 1 };
        let Some(cidr) = rv.iter().find(|s| s.contains('.') || s.contains(':')).cloned() else { eprintln!("Not enough information: \"prefix\" argument is required."); return 1 };
        match verb { "add" => send(format!("addr-add\t{dev}\t{cidr}")), "del" | "delete" => send(format!("addr-del\t{dev}\t{cidr}")), _ => { eprintln!("Command \"{verb}\" is unknown, try \"ip addr help\"."); 1 } }
    } else if is(obj, "route", 1) {
        let a: Vec<String> = if show && !sub.is_empty() { sub[1..].to_vec() } else { vec![] };
        if show { return show_route(&a, json, fam); }
        let dst = rv.first().cloned().unwrap_or_default();
        if dst.is_empty() { eprintln!("Error: Invalid prefix or missing destination."); return 1; }
        let dev = kw(rv, "dev").unwrap_or_default();
        match verb {
            "add" | "replace" => send(format!("route-add\t{dst}\t{}\t{dev}", kw(rv, "via").unwrap_or("-".into()))),
            "del" | "delete" => send(format!("route-del\t{dst}")),
            _ => { eprintln!("Command \"{verb}\" is unknown, try \"ip route help\"."); 1 }
        }
    } else {
        eprintln!("Object \"{obj}\" is unknown, try \"ip help\"."); 1
    }
}

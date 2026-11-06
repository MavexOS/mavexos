//! `wg` / `wg-quick`: WireGuard tooling over the virtual netns. The protocol engine is Cloudflare's boringtun (vendored, wasi clock patch).
//! There is no UDP transport in a browser yet, so `up` creates the interface and address but no packets leave the tab.
use crate::net_request;
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use boringtun::noise::{Tunn, TunnResult};
use boringtun::x25519::{PublicKey, StaticSecret};

fn rand32() -> [u8; 32] {
    let mut k = [0u8; 32];
    getrandom::getrandom(&mut k).expect("random");
    k
}

fn key(s: &str) -> Result<[u8; 32], String> {
    let v = B64.decode(s.trim()).map_err(|_| "Key is not the correct length or format".to_string())?;
    v.try_into().map_err(|_| "Key is not the correct length or format".to_string())
}

fn ipv4_packet(payload: &[u8]) -> Vec<u8> {
    let len = 20 + payload.len();
    let mut p = vec![0x45, 0, (len >> 8) as u8, len as u8, 0, 0, 0, 0, 64, 17, 0, 0, 10, 9, 0, 1, 10, 9, 0, 2];
    let mut sum: u32 = 0;
    for c in p.chunks(2) { sum += u32::from(u16::from_be_bytes([c[0], c[1]])); }
    while sum > 0xffff { sum = (sum & 0xffff) + (sum >> 16); }
    let ck = !(sum as u16);
    p[10] = (ck >> 8) as u8; p[11] = ck as u8;
    p.extend_from_slice(payload);
    p
}

fn selftest() -> i32 {
    let (sa, sb) = (StaticSecret::from(rand32()), StaticSecret::from(rand32()));
    let (pa, pb) = (PublicKey::from(&sa), PublicKey::from(&sb));
    let psk = rand32();
    let mut a = Tunn::new(sa, pb, Some(psk), None, 1, None);
    let mut b = Tunn::new(sb, pa, Some(psk), None, 2, None);
    let (mut buf_a, mut buf_b) = (vec![0u8; 2048], vec![0u8; 2048]);
    let plain = ipv4_packet(b"hello over wireguard");
    let init: Vec<u8> = match a.encapsulate(&plain, &mut buf_a) { TunnResult::WriteToNetwork(d) => d.to_vec(), r => { eprintln!("wg: unexpected {}", name(&r)); return 1; } };
    println!("A -> B  handshake initiation ({} bytes)", init.len());
    let resp: Vec<u8> = match b.decapsulate(None, &init, &mut buf_b) {
        TunnResult::WriteToNetwork(d) => d.to_vec(),
        TunnResult::Err(e) => { eprintln!("wg: B: {e:?}"); return 1; }
        r => { eprintln!("wg: B: unexpected {}", name(&r)); return 1; }
    };
    println!("B -> A  handshake response ({} bytes)", resp.len());
    let mut out: Vec<Vec<u8>> = Vec::new();
    let mut datagram: &[u8] = &resp;
    loop {
        match a.decapsulate(None, datagram, &mut buf_a) {
            TunnResult::WriteToNetwork(d) => out.push(d.to_vec()),
            TunnResult::Err(e) => { eprintln!("wg: A: {e:?}"); return 1; }
            _ => break,
        }
        datagram = &[];
    }
    let mut delivered: Option<Vec<u8>> = None;
    for d in &out {
        println!("A -> B  transport packet, encrypted ({} bytes)", d.len());
        if let TunnResult::WriteToTunnelV4(p, _) = b.decapsulate(None, d, &mut buf_b) { delivered = Some(p.to_vec()); }
    }
    match delivered {
        Some(d) if d == plain => { println!("B decrypted {} bytes, identical to what A sent. boringtun OK", d.len()); 0 }
        _ => { eprintln!("wg: handshake or decryption failed"); 1 }
    }
}

fn name(r: &TunnResult) -> &'static str { match r { TunnResult::Done => "done", TunnResult::Err(_) => "error", _ => "packet" } }

struct Conf { private: String, addrs: Vec<String>, peers: Vec<Vec<(String, String)>> }

fn conf_path(ifname: &str) -> String { format!("/etc/wireguard/{ifname}.conf") }

fn parse_conf(ifname: &str) -> Result<Conf, String> {
    let path = conf_path(ifname);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("wg: {path}: {e}"))?;
    let mut c = Conf { private: String::new(), addrs: vec![], peers: vec![] };
    let mut in_peer = false;
    for line in text.lines() {
        let l = line.split('#').next().unwrap_or("").trim();
        if l.is_empty() { continue; }
        if l.eq_ignore_ascii_case("[interface]") { in_peer = false; continue; }
        if l.eq_ignore_ascii_case("[peer]") { in_peer = true; c.peers.push(vec![]); continue; }
        let Some((k, v)) = l.split_once('=') else { continue };
        let (k, v) = (k.trim().to_string(), v.trim().to_string());
        if in_peer { c.peers.last_mut().unwrap().push((k, v)); }
        else if k.eq_ignore_ascii_case("PrivateKey") { c.private = v; }
        else if k.eq_ignore_ascii_case("Address") { c.addrs.extend(v.split(',').map(|a| a.trim().to_string())); }
    }
    if c.private.is_empty() { return Err(format!("wg: {path}: missing PrivateKey")); }
    key(&c.private).map_err(|e| format!("wg: {path}: PrivateKey: {e}"))?;
    Ok(c)
}

fn show(args: &[String]) -> i32 {
    let links = net_request("link").unwrap_or_default();
    let mut any = false;
    for l in links.lines() {
        let f: Vec<&str> = l.split('\t').collect();
        if f.len() < 6 || f[5] != "wireguard" { continue; }
        if let Some(w) = args.first() { if w != f[2] { continue; } }
        let Ok(c) = parse_conf(f[2]) else { continue };
        any = true;
        let sk = StaticSecret::from(key(&c.private).unwrap());
        println!("interface: {}\n  public key: {}\n  private key: (hidden)", f[2], B64.encode(PublicKey::from(&sk).as_bytes()));
        for p in &c.peers {
            let g = |n: &str| p.iter().find(|(k, _)| k.eq_ignore_ascii_case(n)).map(|(_, v)| v.as_str());
            println!("\npeer: {}", g("PublicKey").unwrap_or("?"));
            if let Some(e) = g("Endpoint") { println!("  endpoint: {e}"); }
            if let Some(a) = g("AllowedIPs") { println!("  allowed ips: {a}"); }
        }
        println!();
    }
    if !any && args.is_empty() { return 0; }
    if !any { eprintln!("wg: Unable to access interface: No such device"); return 1; }
    0
}

pub fn wg(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("genkey") => { let mut k = rand32(); k[0] &= 248; k[31] = (k[31] & 127) | 64; println!("{}", B64.encode(k)); 0 }
        Some("genpsk") => { println!("{}", B64.encode(rand32())); 0 }
        Some("pubkey") => {
            let mut s = String::new();
            let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut s);
            match key(&s) { Ok(k) => { println!("{}", B64.encode(PublicKey::from(&StaticSecret::from(k)).as_bytes())); 0 } Err(e) => { eprintln!("wg: {e}"); 1 } }
        }
        Some("selftest") => selftest(),
        Some("show") | None => show(&args[args.len().min(1)..]),
        Some(_) => { eprintln!("Usage: wg <genkey|genpsk|pubkey|show [iface]|selftest>"); 1 }
    }
}

pub fn wg_quick(args: &[String]) -> i32 {
    let (Some(act), Some(ifname)) = (args.first(), args.get(1)) else { eprintln!("Usage: wg-quick <up|down> <interface>"); return 1 };
    let r = |req: String| net_request(&req).map_err(|(_, m)| m);
    match act.as_str() {
        "up" => {
            let c = match parse_conf(ifname) { Ok(c) => c, Err(e) => { eprintln!("{e}"); return 1 } };
            eprintln!("[#] ip link add {ifname} type wireguard");
            let mut steps: Vec<String> = vec![format!("link-add\t{ifname}\twireguard")];
            for a in &c.addrs { eprintln!("[#] ip address add {a} dev {ifname}"); steps.push(format!("addr-add\t{ifname}\t{a}")); }
            eprintln!("[#] ip link set {ifname} up");
            steps.push(format!("link-set\t{ifname}\tup\t-"));
            for s in steps { if let Err(e) = r(s) { eprintln!("{e}"); return 1; } }
            eprintln!("[!] note: no UDP transport in the browser yet; peers are configured but not reachable");
            0
        }
        "down" => match r(format!("link-del\t{ifname}")) { Ok(_) => { eprintln!("[#] ip link delete dev {ifname}"); 0 } Err(e) => { eprintln!("{e}"); 1 } },
        _ => { eprintln!("Usage: wg-quick <up|down> <interface>"); 1 }
    }
}

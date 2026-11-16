//! `mavex-resolved`: the local DNS resolver on 127.0.0.1:53. It forwards every query, unchanged, to the
//! page's DNS-over-HTTPS layer (resolvers from /etc/mavex/network) and sends the answer back.
//! `dig`: a small DNS lookup tool in the style of dig. Both are MavexOS's own code, not BIND's dig or
//! systemd-resolved; DNS messages are built and parsed with the `simple-dns` crate.
//!
//! Sockets here are MavexOS loopback datagram handles (`mavex.sock_*`), not POSIX sockets: a browser tab
//! cannot open real UDP sockets, so only programs inside MavexOS can talk to each other this way.
use simple_dns::rdata::RData;
use simple_dns::{Name, Packet, PacketFlag, Question, CLASS, QCLASS, QTYPE, RCODE, TYPE};
use std::time::Instant;

#[link(wasm_import_module = "mavex")]
unsafe extern "C" {
    fn sock_open() -> i32;
    fn sock_bind(id: i32, port: u32) -> i32;
    fn sock_sendto(id: i32, port: u32, ptr: *const u8, len: usize) -> i32;
    fn sock_recvfrom(id: i32, ptr: *mut u8, len: usize, from_port: *mut u32, timeout_ms: u32) -> i32;
    fn sock_close(id: i32) -> i32;
}

pub fn resolved() -> i32 {
    let s = unsafe { sock_open() };
    let r = unsafe { sock_bind(s, 53) };
    if r < 0 {
        eprintln!("mavex-resolved: cannot bind 127.0.0.1:53: {}", if r == -13 { "permission denied (must be root)" } else { "address already in use" });
        return 1;
    }
    println!("mavex-resolved: listening on 127.0.0.1:53, forwarding over DNS-over-HTTPS");
    let mut buf = vec![0u8; 4096];
    loop {
        let mut from = 0u32;
        let n = unsafe { sock_recvfrom(s, buf.as_mut_ptr(), buf.len(), &mut from, 0) };
        if n < 12 {
            if n < 0 && n != -110 { eprintln!("mavex-resolved: receive failed ({n})"); return 1; }
            continue;
        }
        let q = &buf[..n as usize];
        let reply = match crate::netutil::dns_exchange(q) {
            Ok((body, _)) if body.len() >= 12 => body,
            other => {
                if let Err(e) = other { eprintln!("mavex-resolved: upstream failed: {e}"); }
                // SERVFAIL: echo the query with QR set and RCODE 2.
                let mut f = q.to_vec();
                f[2] |= 0x80;
                f[3] = (f[3] & 0xf0) | 2;
                f
            }
        };
        unsafe { sock_sendto(s, from, reply.as_ptr(), reply.len()) };
    }
}

fn qtype(s: &str) -> Option<TYPE> {
    Some(match s.to_ascii_uppercase().as_str() {
        "A" => TYPE::A, "AAAA" => TYPE::AAAA, "CNAME" => TYPE::CNAME, "NS" => TYPE::NS, "MX" => TYPE::MX, "TXT" => TYPE::TXT,
        "SOA" => TYPE::SOA, "PTR" => TYPE::PTR, "SRV" => TYPE::SRV, "CAA" => TYPE::CAA, "HTTPS" => TYPE::HTTPS, "SVCB" => TYPE::SVCB,
        _ => return None,
    })
}

fn rcode_name(r: RCODE) -> String {
    match r {
        RCODE::NoError => "NOERROR".into(), RCODE::FormatError => "FORMERR".into(), RCODE::ServerFailure => "SERVFAIL".into(),
        RCODE::NameError => "NXDOMAIN".into(), RCODE::NotImplemented => "NOTIMP".into(), RCODE::Refused => "REFUSED".into(),
        other => format!("{other:?}").to_uppercase(),
    }
}

fn rdata_text(d: &RData) -> String {
    match d {
        RData::A(a) => std::net::Ipv4Addr::from(a.address).to_string(),
        RData::AAAA(a) => std::net::Ipv6Addr::from(a.address).to_string(),
        RData::CNAME(n) => format!("{}.", n.0),
        RData::NS(n) => format!("{}.", n.0),
        RData::PTR(n) => format!("{}.", n.0),
        RData::MX(m) => format!("{} {}.", m.preference, m.exchange),
        RData::SRV(v) => format!("{} {} {} {}.", v.priority, v.weight, v.port, v.target),
        RData::SOA(o) => format!("{}. {}. {} {} {} {} {}", o.mname, o.rname, o.serial, o.refresh, o.retry, o.expire, o.minimum),
        RData::TXT(t) => {
            let parts: Vec<String> = t.iter_raw().map(|(k, v)| {
                let mut s = String::from_utf8_lossy(k).into_owned();
                if let Some(v) = v { s.push('='); s.push_str(&String::from_utf8_lossy(v)); }
                s
            }).collect();
            format!("\"{}\"", parts.join("\" \""))
        }
        other => format!("{other:?}"),
    }
}

fn resolv_conf_port_ok() -> Result<(), String> {
    let txt = std::fs::read_to_string("/etc/resolv.conf").unwrap_or_default();
    for l in txt.lines() {
        if let Some(ns) = l.trim().strip_prefix("nameserver") {
            let ns = ns.trim();
            return if ns == "127.0.0.1" || ns == "::1" || ns == "localhost" { Ok(()) } else {
                Err(format!("/etc/resolv.conf names {ns}, but a browser tab cannot send DNS packets to other machines; only the local resolver (127.0.0.1) works"))
            };
        }
    }
    Ok(())
}

pub fn dig(args: &[String]) -> i32 {
    let (mut name, mut ty, mut short, mut port, mut server) = (None, TYPE::A, false, 53u32, None::<String>);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" => {
                println!("usage: dig [@127.0.0.1] [-p port] name [type] [+short]\nLooks a name up through the local MavexOS resolver (127.0.0.1:53), which forwards over DNS-over-HTTPS.\ntypes: A AAAA CNAME NS MX TXT SOA PTR SRV CAA HTTPS SVCB");
                return 0;
            }
            "+short" => short = true,
            "-p" => port = it.next().and_then(|v| v.parse().ok()).unwrap_or(53),
            "-t" => match it.next().and_then(|v| qtype(v)) { Some(t) => ty = t, None => { eprintln!("dig: unknown type"); return 1; } },
            s if s.starts_with('@') => server = Some(s[1..].to_string()),
            s if s.starts_with('+') || s.starts_with('-') => {}
            s => match (name.is_some(), qtype(s)) {
                (true, Some(t)) => ty = t,
                (false, _) => name = Some(s.trim_end_matches('.').to_string()),
                _ => { eprintln!("dig: unknown type {s}"); return 1; }
            },
        }
    }
    let Some(name) = name else { eprintln!("dig: usage: dig [@127.0.0.1] [-p port] name [type] [+short]"); return 1 };
    match server.as_deref() {
        None => if let Err(e) = resolv_conf_port_ok() { eprintln!("dig: {e}"); return 1; },
        Some("127.0.0.1") | Some("localhost") | Some("::1") => {}
        Some(other) => { eprintln!("dig: cannot query {other}: a browser tab cannot send DNS packets to other machines; only the local resolver (127.0.0.1) works. Upstream resolvers are the doh = lines in /etc/mavex/network."); return 1; }
    }
    let mut idb = [0u8; 2];
    let _ = getrandom::getrandom(&mut idb);
    let id = u16::from_le_bytes(idb);
    let Ok(qname) = Name::new(&name) else { eprintln!("dig: '{name}' is not a legal name"); return 1 };
    let mut q = Packet::new_query(id);
    q.set_flags(PacketFlag::RECURSION_DESIRED);
    q.questions.push(Question::new(qname, QTYPE::TYPE(ty), QCLASS::CLASS(CLASS::IN), false));
    let Ok(wire) = q.build_bytes_vec() else { eprintln!("dig: could not build the query"); return 1 };
    let t0 = Instant::now();
    let mut buf = vec![0u8; 4096];
    let n = unsafe {
        let s = sock_open();
        let sent = sock_sendto(s, port, wire.as_ptr(), wire.len());
        let n = if sent < 0 { sent } else { let mut from = 0u32; sock_recvfrom(s, buf.as_mut_ptr(), buf.len(), &mut from, 8000) };
        sock_close(s);
        n
    };
    let ms = t0.elapsed().as_millis();
    if n < 0 {
        if n == -111 { eprintln!("dig: no resolver is listening on 127.0.0.1:{port} (is mavex-resolved running? try: systemctl status mavex-resolved)"); }
        else { eprintln!("dig: connection timed out; no servers could be reached"); }
        return 9;
    }
    let Ok(r) = Packet::parse(&buf[..n as usize]) else { eprintln!("dig: malformed reply"); return 1 };
    if short {
        for a in &r.answers { println!("{}", rdata_text(&a.rdata)); }
        return 0;
    }
    println!("\n; <<>> MavexOS dig <<>> {name} {ty:?}");
    println!(";; ->>HEADER<<- opcode: QUERY, status: {}, id: {}", rcode_name(r.rcode()), r.id());
    println!(";; QUERY: {}, ANSWER: {}, AUTHORITY: {}, ADDITIONAL: {}\n", r.questions.len(), r.answers.len(), r.name_servers.len(), r.additional_records.len());
    println!(";; QUESTION SECTION:\n;{name}.\t\t\tIN\t{ty:?}\n");
    for (title, set) in [("ANSWER", &r.answers), ("AUTHORITY", &r.name_servers)] {
        if set.is_empty() { continue; }
        println!(";; {title} SECTION:");
        for a in set.iter() { println!("{}.\t\t{}\tIN\t{:?}\t{}", a.name, a.ttl, a.rdata.type_code(), rdata_text(&a.rdata)); }
        println!();
    }
    println!(";; Query time: {ms} msec\n;; SERVER: 127.0.0.1#{port} (mavex-resolved, DNS-over-HTTPS upstream)\n;; MSG SIZE  rcvd: {n}\n");
    0
}

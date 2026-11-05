//! Virtual network namespace with a netlink-like text message API (rtnetlink emulation).
//! Requests are tab-separated words; replies are tab-separated records:
//!   L index name flags mtu kind mac        (link)
//!   A index family addr prefix scope       (address)
//!   R family dst prefixlen gw dev proto scope src   (route)
use std::net::{IpAddr, Ipv4Addr};

pub struct Iface {
    pub index: u32,
    pub name: String,
    pub up: bool,
    pub mtu: u32,
    pub kind: &'static str,
    pub mac: [u8; 6],
    pub addrs: Vec<(IpAddr, u8)>,
}

pub struct Route {
    pub dst: Option<(IpAddr, u8)>,
    pub gw: Option<IpAddr>,
    pub dev: String,
}

pub struct Net {
    pub ifaces: Vec<Iface>,
    pub routes: Vec<Route>,
    next_index: u32,
}

pub type NetResult = Result<String, (i32, String)>;

fn parse_cidr(s: &str) -> Option<(IpAddr, u8)> {
    let (a, p) = match s.split_once('/') {
        Some((a, p)) => (a, Some(p)),
        None => (s, None),
    };
    let ip: IpAddr = a.parse().ok()?;
    let max = if ip.is_ipv4() { 32 } else { 128 };
    let p = match p {
        Some(p) => p.parse::<u8>().ok().filter(|p| *p <= max)?,
        None => max,
    };
    Some((ip, p))
}

fn mask_v4(ip: Ipv4Addr, p: u8) -> Ipv4Addr {
    let m = if p == 0 { 0 } else { u32::MAX << (32 - p as u32) };
    Ipv4Addr::from(u32::from(ip) & m)
}

pub fn network_of(ip: IpAddr, p: u8) -> IpAddr {
    match ip {
        IpAddr::V4(v) => IpAddr::V4(mask_v4(v, p)),
        IpAddr::V6(v) => {
            let m = if p == 0 { 0 } else { u128::MAX << (128 - p as u32) };
            IpAddr::V6((u128::from(v) & m).into())
        }
    }
}

const EPERM: i32 = 1;
const ENODEV: i32 = 19;
const EEXIST: i32 = 17;
const EINVAL: i32 = 22;
const EADDRNOTAVAIL: i32 = 99;
const ESRCH: i32 = 3;

impl Default for Net {
    fn default() -> Self {
        Self::new()
    }
}

impl Net {
    pub fn new() -> Self {
        let lo = Iface {
            index: 1,
            name: "lo".into(),
            up: true,
            mtu: 65536,
            kind: "loopback",
            mac: [0; 6],
            addrs: vec![(IpAddr::V4(Ipv4Addr::LOCALHOST), 8), ("::1".parse().unwrap(), 128)],
        };
        Net { ifaces: vec![lo], routes: Vec::new(), next_index: 2 }
    }

    fn find(&self, name: &str) -> Result<usize, (i32, String)> {
        self.ifaces.iter().position(|i| i.name == name).ok_or((-ENODEV, format!("Cannot find device \"{}\"", name)))
    }

    fn mac_for(index: u32) -> [u8; 6] {
        [0x02, 0x4d, 0x58, 0x00, (index >> 8) as u8, index as u8]
    }

    /// `root` is whether the caller has euid 0 (required for any mutation).
    pub fn request(&mut self, root: bool, req: &str) -> NetResult {
        let w: Vec<&str> = req.split('\t').collect();
        let need_root = !matches!(w[0], "link" | "addr" | "route");
        if need_root && !root {
            return Err((-EPERM, "Operation not permitted".into()));
        }
        match w[0] {
            "link" => Ok(self.dump_links()),
            "addr" => Ok(self.dump_addrs()),
            "route" => Ok(self.dump_routes()),
            "link-set" if w.len() >= 4 => {
                let i = self.find(w[1])?;
                match w[2] {
                    "up" => self.ifaces[i].up = true,
                    "down" => self.ifaces[i].up = false,
                    "mtu" => {
                        let m: u32 = w[3].parse().map_err(|_| (-EINVAL, "Error: argument \"mtu\" is wrong".to_string()))?;
                        if !(68..=65536).contains(&m) {
                            return Err((-EINVAL, "RTNETLINK answers: Invalid argument".into()));
                        }
                        self.ifaces[i].mtu = m;
                    }
                    "name" => {
                        if self.ifaces[i].up {
                            return Err((-16, "RTNETLINK answers: Device or resource busy".into()));
                        }
                        self.ifaces[i].name = w[3].to_string();
                    }
                    _ => return Err((-EINVAL, "Error: invalid link attribute".into())),
                }
                self.prune_routes();
                Ok(String::new())
            }
            "link-add" if w.len() >= 3 => {
                if w[2] != "dummy" {
                    return Err((-95, format!("Error: Unknown device type \"{}\"", w[2])));
                }
                if self.ifaces.iter().any(|i| i.name == w[1]) {
                    return Err((-EEXIST, "RTNETLINK answers: File exists".into()));
                }
                let index = self.next_index;
                self.next_index += 1;
                self.ifaces.push(Iface { index, name: w[1].into(), up: false, mtu: 1500, kind: "dummy", mac: Self::mac_for(index), addrs: vec![] });
                Ok(String::new())
            }
            "link-del" if w.len() >= 2 => {
                let i = self.find(w[1])?;
                if self.ifaces[i].kind == "loopback" {
                    return Err((-95, "RTNETLINK answers: Operation not supported".into()));
                }
                let name = self.ifaces.remove(i).name;
                self.routes.retain(|r| r.dev != name);
                Ok(String::new())
            }
            "addr-add" if w.len() >= 3 => {
                let i = self.find(w[1])?;
                let a = parse_cidr(w[2]).ok_or((-EINVAL, format!("Error: any valid prefix is expected rather than \"{}\".", w[2])))?;
                if self.ifaces[i].addrs.contains(&a) {
                    return Err((-EEXIST, "RTNETLINK answers: File exists".into()));
                }
                self.ifaces[i].addrs.push(a);
                Ok(String::new())
            }
            "addr-del" if w.len() >= 3 => {
                let i = self.find(w[1])?;
                let a = parse_cidr(w[2]).ok_or((-EINVAL, format!("Error: any valid prefix is expected rather than \"{}\".", w[2])))?;
                let n = self.ifaces[i].addrs.len();
                self.ifaces[i].addrs.retain(|x| *x != a);
                if self.ifaces[i].addrs.len() == n {
                    return Err((-EADDRNOTAVAIL, "RTNETLINK answers: Cannot assign requested address".into()));
                }
                self.prune_routes();
                Ok(String::new())
            }
            "route-add" if w.len() >= 4 => {
                // dst ("default" or cidr), gw ("-" or ip), dev
                let dst = if w[1] == "default" { None } else { Some(parse_cidr(w[1]).ok_or((-EINVAL, format!("Error: inet prefix is expected rather than \"{}\".", w[1])))?) };
                let gw = if w[2] == "-" { None } else { Some(w[2].parse::<IpAddr>().map_err(|_| (-EINVAL, format!("Error: inet address is expected rather than \"{}\".", w[2])))?) };
                let dev = if w[3].is_empty() {
                    let g = gw.ok_or((-EINVAL, "Error: Device or gateway required".to_string()))?;
                    let found = self.ifaces.iter().find(|i| i.up && i.addrs.iter().any(|(a, p)| a.is_ipv4() == g.is_ipv4() && network_of(*a, *p) == network_of(g, *p)));
                    found.map(|i| i.name.clone()).ok_or((-101, "Error: Nexthop has invalid gateway.".to_string()))?
                } else {
                    self.find(w[3])?;
                    w[3].to_string()
                };
                let i = self.find(&dev)?;
                if !self.ifaces[i].up {
                    return Err((-100, "Error: Network is down".into())).or_else(|e: (i32, String)| Err((e.0, "RTNETLINK answers: Network is down".into())));
                }
                let dst = dst.map(|(a, p)| (network_of(a, p), p));
                if self.routes.iter().any(|r| r.dst == dst && r.dev == dev) {
                    return Err((-EEXIST, "RTNETLINK answers: File exists".into()));
                }
                self.routes.push(Route { dst, gw, dev });
                Ok(String::new())
            }
            "route-del" if w.len() >= 2 => {
                let dst = if w[1] == "default" { None } else { Some(parse_cidr(w[1]).ok_or((-EINVAL, format!("Error: inet prefix is expected rather than \"{}\".", w[1])))?) };
                let dst = dst.map(|(a, p)| (network_of(a, p), p));
                let n = self.routes.len();
                self.routes.retain(|r| r.dst != dst);
                if self.routes.len() == n {
                    return Err((-ESRCH, "RTNETLINK answers: No such process".into()));
                }
                Ok(String::new())
            }
            _ => Err((-EINVAL, "Error: malformed netlink request".into())),
        }
    }

    fn prune_routes(&mut self) {
        let ifs = &self.ifaces;
        self.routes.retain(|r| ifs.iter().any(|i| i.name == r.dev && i.up));
    }

    fn dump_links(&self) -> String {
        let mut o = String::new();
        for i in &self.ifaces {
            let m = i.mac;
            o.push_str(&format!(
                "L\t{}\t{}\t{}\t{}\t{}\t{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}\n",
                i.index, i.name, if i.up { "up" } else { "down" }, i.mtu, i.kind, m[0], m[1], m[2], m[3], m[4], m[5]
            ));
        }
        o
    }

    fn dump_addrs(&self) -> String {
        let mut o = String::new();
        for i in &self.ifaces {
            for (a, p) in &i.addrs {
                let scope = if a.is_loopback() { "host" } else { "global" };
                o.push_str(&format!("A\t{}\t{}\t{}\t{}\t{}\n", i.index, if a.is_ipv4() { 4 } else { 6 }, a, p, scope));
            }
        }
        o
    }

    fn dump_routes(&self) -> String {
        let mut o = String::new();
        let mut line = |fam: u8, dst: String, plen: u8, gw: String, dev: &str, proto: &str, scope: &str, src: String| {
            o.push_str(&format!("R\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n", fam, dst, plen, gw, dev, proto, scope, src));
        };
        for r in &self.routes {
            let fam = match (&r.dst, &r.gw) {
                (Some((a, _)), _) => if a.is_ipv4() { 4 } else { 6 },
                (None, Some(g)) => if g.is_ipv4() { 4 } else { 6 },
                _ => 4,
            };
            let (dst, plen) = r.dst.map(|(a, p)| (a.to_string(), p)).unwrap_or(("default".into(), 0));
            line(fam, dst, plen, r.gw.map(|g| g.to_string()).unwrap_or("-".into()), &r.dev, "static", if r.gw.is_some() { "universe" } else { "link" }, "-".into());
        }
        for i in self.ifaces.iter().filter(|i| i.up && i.kind != "loopback") {
            for (a, p) in &i.addrs {
                if let IpAddr::V4(_) = a {
                    if *p < 32 {
                        line(4, network_of(*a, *p).to_string(), *p, "-".into(), &i.name, "kernel", "link", a.to_string());
                    }
                }
            }
        }
        o
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lo_defaults_and_mutation_rules() {
        let mut n = Net::new();
        assert!(n.request(false, "link").unwrap().contains("L\t1\tlo\tup\t65536\tloopback\t00:00:00:00:00:00"));
        assert_eq!(n.request(false, "link-add\tdummy0\tdummy").unwrap_err().0, -EPERM);
        n.request(true, "link-add\tdummy0\tdummy").unwrap();
        n.request(true, "addr-add\tdummy0\t10.0.0.2/24").unwrap();
        assert!(n.request(true, "addr-add\tdummy0\t10.0.0.2/24").is_err());
        assert!(n.request(true, "route-add\tdefault\t10.0.0.1\t").is_err());
        n.request(true, "link-set\tdummy0\tup\t-").unwrap();
        n.request(true, "route-add\tdefault\t10.0.0.1\t").unwrap();
        let r = n.request(false, "route").unwrap();
        assert!(r.contains("R\t4\tdefault\t0\t10.0.0.1\tdummy0\tstatic"));
        assert!(r.contains("R\t4\t10.0.0.0\t24\t-\tdummy0\tkernel\tlink\t10.0.0.2"));
        n.request(true, "link-set\tdummy0\tdown\t-").unwrap();
        assert!(!n.request(false, "route").unwrap().contains("dummy0"));
    }
}

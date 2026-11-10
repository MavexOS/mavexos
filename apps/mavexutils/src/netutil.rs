//! `ping` (HTTPS reachability, not ICMP) and `mavex-backup` / `mavex-restore` over the page's fetch seam.
use std::time::Instant;

#[link(wasm_import_module = "mavex")]
unsafe extern "C" {
    fn fetch_open(mp: *const u8, mn: usize, up: *const u8, un: usize, hp: *const u8, hn: usize, bp: *const u8, bn: usize) -> i32;
    fn fetch_wait(id: i32) -> i32;
    fn fetch_info(id: i32, out: *mut u32) -> i32;
    fn fetch_read(id: i32, which: i32, off: usize, ptr: *mut u8, len: usize) -> i32;
    fn fetch_close(id: i32) -> i32;
}

struct Reply { status: u32, kind: u32, note: String }

fn call(method: &str, url: &str) -> Reply {
    unsafe {
        let id = fetch_open(method.as_ptr(), method.len(), url.as_ptr(), url.len(), std::ptr::null(), 0, std::ptr::null(), 0);
        fetch_wait(id);
        let mut info = [0u32; 5];
        fetch_info(id, info.as_mut_ptr());
        let mut buf = vec![0u8; info[4] as usize];
        let n = fetch_read(id, 2, 0, buf.as_mut_ptr(), buf.len()).max(0) as usize;
        fetch_close(id);
        Reply { status: info[0], kind: info[1], note: String::from_utf8_lossy(&buf[..n]).into_owned() }
    }
}

pub fn ping(args: &[String]) -> i32 {
    let mut count = 4u32;
    let mut target = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-c" => count = it.next().and_then(|v| v.parse().ok()).unwrap_or(4),
            "-h" | "--help" => {
                println!("usage: ping [-c count] host-or-url\nHTTPS reachability probe through the browser (not ICMP): a reply means the host answered,\n\"reachable (no CORS)\" means it answered but the browser hides the response.");
                return 0;
            }
            s if !s.starts_with('-') => target = Some(s.to_string()),
            _ => {}
        }
    }
    let Some(t) = target else { eprintln!("ping: usage: ping [-c count] host-or-url"); return 2 };
    let url = if t.contains("://") { t.clone() } else { format!("https://{t}/") };
    println!("PING {url} (HTTPS HEAD through the browser, not ICMP)");
    let (mut ok, mut times) = (0u32, Vec::new());
    for seq in 1..=count {
        let t0 = Instant::now();
        let r = call("HEAD", &url);
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        match r.kind {
            0 => { ok += 1; times.push(ms); println!("reply from {t}: seq={seq} status={} time={ms:.1} ms", r.status); }
            2 => { ok += 1; times.push(ms); println!("reachable (no CORS) {t}: seq={seq} time={ms:.1} ms"); }
            _ => println!("no reply from {t}: seq={seq}: {}", r.note),
        }
    }
    println!("--- {t} ping statistics ---\n{count} sent, {ok} answered, {}% loss", (count - ok) * 100 / count.max(1));
    if !times.is_empty() {
        let min = times.iter().cloned().fold(f64::MAX, f64::min);
        let max = times.iter().cloned().fold(0.0, f64::max);
        let avg = times.iter().sum::<f64>() / times.len() as f64;
        println!("rtt min/avg/max = {min:.1}/{avg:.1}/{max:.1} ms");
    }
    if ok > 0 { 0 } else { 1 }
}

pub fn backup_restore(name: &str) -> i32 {
    let method = if name == "mavex-restore" { "RESTORE" } else { "BACKUP" };
    let r = call(method, "mavex:storage");
    if r.kind == 0 { println!("{}", r.note); 0 } else { eprintln!("{name}: {}", r.note); 1 }
}

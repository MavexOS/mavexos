//! /proc: regenerated into the vfs whenever a /proc path is opened, stat'ed or listed.
use crate::Sys;

#[derive(Clone, Default)]
pub struct HostInfo {
    pub boot_ns: u64,
    pub now_ns: u64,
    pub mem_total_kib: u64,
    pub mem_used_kib: u64,
    pub ncpu: u32,
    pub version: String,
}

pub fn is_proc(path: &str) -> bool {
    path == "/proc" || path.starts_with("/proc/")
}

impl Sys {
    pub fn refresh_procfs(&mut self) {
        self.as_root();
        let h = self.hostinfo.clone();
        let up = h.now_ns.saturating_sub(h.boot_ns) as f64 / 1e9;
        let total = h.mem_total_kib.max(1);
        let used = h.mem_used_kib.min(total);
        let ncpu = h.ncpu.max(1);
        let v = &mut self.vfs;
        let _ = v.mkdir("/proc", 0o555);
        let w = |v: &mut mavex_vfs::Vfs, p: &str, s: String| {
            let _ = v.write_file(p, s.as_bytes());
        };
        w(v, "/proc/version", format!("MavexOS version {0} (mavex@mavexos) (rustc, wasm32) #1 MavexOS {0}\n", h.version));
        w(v, "/proc/uptime", format!("{:.2} {:.2}\n", up, up * ncpu as f64 * 0.98));
        w(v, "/proc/loadavg", format!("0.00 0.00 0.00 1/{} {}\n", self.procs.len().max(1), self.next_pid.saturating_sub(1)));
        w(v, "/proc/meminfo", format!(
            "MemTotal:       {total:>8} kB\nMemFree:        {:>8} kB\nMemAvailable:   {:>8} kB\nBuffers:        {:>8} kB\nCached:         {:>8} kB\nSwapCached:     {:>8} kB\nSwapTotal:      {:>8} kB\nSwapFree:       {:>8} kB\n",
            total - used, total - used, 0, 0, 0, 0, 0
        ));
        let mut cpu = String::new();
        for i in 0..ncpu {
            cpu += &format!("processor\t: {i}\nvendor_id\t: MavexOS\nmodel name\t: wasm32 virtual CPU\ncpu MHz\t\t: 1000.000\ncache size\t: 0 KB\ncpu cores\t: {ncpu}\nflags\t\t: wasm32 simd128\n\n");
        }
        w(v, "/proc/cpuinfo", cpu);
        w(v, "/proc/mounts", "rootfs / rootfs rw 0 0\nproc /proc proc rw 0 0\n".into());
        w(v, "/proc/stat", format!("cpu  0 0 0 {} 0 0 0 0 0 0\nbtime {}\nprocesses {}\nprocs_running 1\n", (up * 100.0) as u64, h.boot_ns / 1_000_000_000, self.next_pid.saturating_sub(1)));

        let live: Vec<u32> = self.procs.keys().copied().collect();
        if let Ok(ents) = self.vfs.readdir("/proc") {
            for e in ents.iter().filter(|e| e.chars().all(|c| c.is_ascii_digit()) && !e.is_empty()) {
                if !live.contains(&e.parse().unwrap_or(0)) {
                    for f in ["stat", "status", "cmdline", "comm", "cwd", "exe"] {
                        let _ = self.vfs.unlink(&format!("/proc/{e}/{f}"));
                    }
                    let _ = self.vfs.rmdir(&format!("/proc/{e}"));
                }
            }
        }
        for pid in live {
            let p = &self.procs[&pid];
            let dir = format!("/proc/{pid}");
            let comm = p.argv.first().map(|a| a.rsplit('/').next().unwrap_or(a).to_string()).unwrap_or_default();
            let cmdline: Vec<u8> = p.argv.iter().flat_map(|a| a.bytes().chain(std::iter::once(0))).collect();
            let state = if p.exit.is_some() { 'Z' } else { 'R' };
            let stat = format!("{pid} ({comm}) {state} {} {pid} {pid} 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 0 0 0\n", p.ppid);
            let status = format!("Name:\t{comm}\nState:\t{state}\nPid:\t{pid}\nPPid:\t{}\nUid:\t{}\t{}\t{}\t{}\nGid:\t{}\t{}\t{}\t{}\nThreads:\t1\n", p.ppid, p.uid, p.euid, p.uid, p.euid, p.gid, p.egid, p.gid, p.egid);
            let (cwd, exe) = (p.cwd.clone(), p.exe.clone());
            let _ = self.vfs.mkdir(&dir, 0o555);
            let _ = self.vfs.write_file(&format!("{dir}/stat"), stat.as_bytes());
            let _ = self.vfs.write_file(&format!("{dir}/status"), status.as_bytes());
            let _ = self.vfs.write_file(&format!("{dir}/cmdline"), &cmdline);
            let _ = self.vfs.write_file(&format!("{dir}/comm"), format!("{comm}\n").as_bytes());
            for (n, t) in [("cwd", cwd), ("exe", exe)] {
                let _ = self.vfs.unlink(&format!("{dir}/{n}"));
                let _ = self.vfs.symlink(&t, &format!("{dir}/{n}"));
            }
        }
    }
}

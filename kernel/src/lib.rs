use mavex_shell::{ExecReq, ExecRes, Host, Shell};
use mavex_sys::{errno, Stdio, Sys};
use mavex_vfs::{Errno, Kind, Vfs};
use std::cell::RefCell;

#[link(wasm_import_module = "host")]
extern "C" {
    fn console_write(ptr: *const u8, len: usize);
    fn run_process(pid: u32);
    fn now_ns() -> f64;
}

fn print(s: &str) {
    unsafe { console_write(s.as_ptr(), s.len()) }
}

thread_local! {
    static SYS: RefCell<Sys> = RefCell::new(Sys::new(Vfs::new()));
    static SHELL: RefCell<Shell> = RefCell::new(Shell::new());
    static RESULT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

fn with_sys<R>(f: impl FnOnce(&mut Sys) -> R) -> R {
    SYS.with(|s| {
        let mut s = s.borrow_mut();
        s.vfs.set_time(unsafe { now_ns() } as u64);
        s.as_root();
        f(&mut s)
    })
}

fn errno_code(e: Errno) -> i32 {
    -match e {
        Errno::NoEnt => 2,
        Errno::Exist => 17,
        Errno::NotDir => 20,
        Errno::IsDir => 21,
        Errno::NotEmpty => 39,
        Errno::Inval => 22,
        Errno::Loop => 40,
        Errno::Acces => 13,
        Errno::Perm => 1,
    }
}

unsafe fn str_arg<'a>(ptr: *const u8, len: usize) -> &'a str {
    std::str::from_utf8(std::slice::from_raw_parts(ptr, len)).unwrap_or("")
}

fn ret(r: Result<(), Errno>) -> i32 {
    match r {
        Ok(()) => 0,
        Err(e) => errno_code(e),
    }
}

fn stage(d: Vec<u8>) -> i32 {
    let len = d.len() as i32;
    RESULT.with(|r| *r.borrow_mut() = d);
    len
}

fn wr<T>(r: Result<T, u16>, f: impl FnOnce(T) -> i32) -> i32 {
    match r {
        Ok(v) => f(v),
        Err(e) => -(e as i32),
    }
}

fn nul_list(s: &str) -> Vec<String> {
    s.split('\0').filter(|x| !x.is_empty()).map(String::from).collect()
}

fn join_nul(v: &[String]) -> Vec<u8> {
    let mut out = Vec::new();
    for s in v {
        out.extend_from_slice(s.as_bytes());
        out.push(0);
    }
    out
}

#[no_mangle]
pub extern "C" fn k_alloc(len: usize) -> *mut u8 {
    let mut v = Vec::<u8>::with_capacity(len);
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

#[no_mangle]
pub unsafe extern "C" fn k_free(ptr: *mut u8, len: usize) {
    drop(Vec::from_raw_parts(ptr, 0, len));
}

#[no_mangle]
pub extern "C" fn k_result_ptr() -> *const u8 {
    RESULT.with(|r| r.borrow().as_ptr())
}

// ---- host-facing filesystem helpers ----
#[no_mangle]
pub unsafe extern "C" fn k_mkdir(p: *const u8, n: usize) -> i32 {
    let path = str_arg(p, n);
    with_sys(|s| ret(s.vfs.mkdir(path, 0o755)))
}

#[no_mangle]
pub unsafe extern "C" fn k_write_file(p: *const u8, n: usize, d: *const u8, dn: usize) -> i32 {
    let path = str_arg(p, n);
    let data = std::slice::from_raw_parts(d, dn);
    with_sys(|s| ret(s.vfs.write_file(path, data)))
}

#[no_mangle]
pub unsafe extern "C" fn k_symlink(t: *const u8, tn: usize, p: *const u8, n: usize) -> i32 {
    let (target, path) = (str_arg(t, tn), str_arg(p, n));
    with_sys(|s| ret(s.vfs.symlink(target, path)))
}

#[no_mangle]
pub unsafe extern "C" fn k_chmod(p: *const u8, n: usize, mode: u32) -> i32 {
    let path = str_arg(p, n);
    with_sys(|s| ret(s.vfs.chmod(path, mode)))
}

#[no_mangle]
pub unsafe extern "C" fn k_chown(p: *const u8, n: usize, uid: u32, gid: u32) -> i32 {
    let path = str_arg(p, n);
    with_sys(|s| ret(s.vfs.chown(path, uid, gid)))
}

#[no_mangle]
pub unsafe extern "C" fn k_unlink(p: *const u8, n: usize) -> i32 {
    let path = str_arg(p, n);
    with_sys(|s| ret(s.vfs.unlink(path)))
}

#[no_mangle]
pub unsafe extern "C" fn k_read_file(p: *const u8, n: usize) -> i32 {
    let path = str_arg(p, n);
    match with_sys(|s| s.vfs.read_file(path)) {
        Ok(d) => stage(d),
        Err(e) => errno_code(e),
    }
}

#[no_mangle]
pub unsafe extern "C" fn k_readdir(p: *const u8, n: usize) -> i32 {
    let path = str_arg(p, n);
    match with_sys(|s| s.vfs.readdir(path)) {
        Ok(names) => stage(names.join("\n").into_bytes()),
        Err(e) => errno_code(e),
    }
}

// ---- processes ----
fn stdio_arg(v: i32) -> Stdio {
    match v {
        n if n >= 0 => Stdio::Fd(n as u32),
        -2 => Stdio::Null,
        _ => Stdio::Tty,
    }
}

/// stdio args: >=0 duplicates that fd of the parent, -1 = terminal, -2 = null.
#[no_mangle]
pub unsafe extern "C" fn k_spawn(
    ppid: u32, exe_p: *const u8, exe_n: usize, argv_p: *const u8, argv_n: usize, env_p: *const u8, env_n: usize,
    cwd_p: *const u8, cwd_n: usize, s0: i32, s1: i32, s2: i32,
) -> i32 {
    let name = str_arg(exe_p, exe_n);
    let argv = nul_list(str_arg(argv_p, argv_n));
    let env_given = nul_list(str_arg(env_p, env_n));
    let cwd_given = str_arg(cwd_p, cwd_n).to_string();
    with_sys(|s| {
        let env = if env_n == 0 { s.proc_env(ppid).unwrap_or_default() } else { env_given };
        let cwd = if cwd_n == 0 { s.proc_cwd(ppid).unwrap_or_else(|| "/".into()) } else { cwd_given };
        let path = env.iter().find_map(|e| e.strip_prefix("PATH=")).unwrap_or("/bin").to_string();
        let exe = match s.which(name, &path, &cwd) {
            Some(e) => e,
            None => return -(errno::NOENT as i32),
        };
        let argv = if argv.is_empty() { vec![name.to_string()] } else { argv };
        wr(s.spawn(ppid, exe, argv, env, cwd, [stdio_arg(s0), stdio_arg(s1), stdio_arg(s2)]), |p| p as i32)
    })
}

#[no_mangle]
pub extern "C" fn k_proc_exit(pid: u32, code: i32) {
    with_sys(|s| s.exit(pid, code));
}

#[no_mangle]
pub extern "C" fn k_wait(pid: u32) -> i32 {
    with_sys(|s| wr(s.wait(pid), |c| c))
}

#[no_mangle]
pub extern "C" fn k_proc_image(pid: u32) -> i32 {
    match with_sys(|s| s.proc_exe(pid).and_then(|e| s.vfs.read_file(&e).map_err(|_| errno::NOENT))) {
        Ok(d) => stage(d),
        Err(e) => -(e as i32),
    }
}

#[no_mangle]
pub extern "C" fn k_proc_args(pid: u32) -> i32 {
    with_sys(|s| wr(s.proc_args(pid), |a| stage(join_nul(&a))))
}

#[no_mangle]
pub extern "C" fn k_proc_env(pid: u32) -> i32 {
    with_sys(|s| wr(s.proc_env(pid), |a| stage(join_nul(&a))))
}

#[no_mangle]
pub extern "C" fn k_pipe(pid: u32) -> i32 {
    with_sys(|s| wr(s.create_pipe(pid), |(r, w)| ((w << 16) | r) as i32))
}

// ---- fd and path operations (WASI-shaped) ----
#[no_mangle]
pub extern "C" fn k_tty_take() -> i32 {
    let out = with_sys(|s| std::mem::take(&mut s.tty_out));
    if out.is_empty() { 0 } else { stage(out) }
}

#[no_mangle]
pub extern "C" fn k_proc_creds(pid: u32) -> i32 {
    with_sys(|s| wr(s.proc_creds(pid), |(u, g, eu, eg)| {
        let mut b = Vec::new();
        for v in [u, g, eu, eg] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        stage(b)
    }))
}

#[no_mangle]
pub extern "C" fn k_setcreds(pid: u32, uid: u32, gid: u32) -> i32 {
    with_sys(|s| wr(s.set_proc_creds(pid, uid, gid), |_| 0))
}

#[no_mangle]
pub unsafe extern "C" fn k_chmod_as(pid: u32, p: *const u8, n: usize, mode: u32) -> i32 {
    let path = str_arg(p, n);
    with_sys(|s| {
        s.enter(pid);
        let abs = mavex_vfs::normalize(&s.proc_cwd(pid).unwrap_or_else(|| "/".into()), path);
        ret(s.vfs.chmod(&abs, mode))
    })
}

#[no_mangle]
pub unsafe extern "C" fn k_chown_as(pid: u32, p: *const u8, n: usize, uid: u32, gid: u32) -> i32 {
    let path = str_arg(p, n);
    with_sys(|s| {
        s.enter(pid);
        let abs = mavex_vfs::normalize(&s.proc_cwd(pid).unwrap_or_else(|| "/".into()), path);
        ret(s.vfs.chown(&abs, uid, gid))
    })
}

/// Netlink-like request. Returns reply length (>= 0) staged, or a negative errno with the message staged.
#[no_mangle]
pub unsafe extern "C" fn k_net(pid: u32, p: *const u8, n: usize) -> i32 {
    let req = str_arg(p, n);
    with_sys(|s| match s.net_request(pid, req) {
        Ok(t) => stage(t.into_bytes()),
        Err((code, msg)) => {
            stage(msg.into_bytes());
            code
        }
    })
}

/// Stages [mode, uid, gid, kind(0 file,1 dir,2 symlink), size_lo, size_hi] as 6 LE u32 for an absolute path (lstat semantics).
#[no_mangle]
pub unsafe extern "C" fn k_statx(pid: u32, p: *const u8, n: usize) -> i32 {
    let path = str_arg(p, n);
    with_sys(|s| {
        s.enter(pid);
        let cwd = s.proc_cwd(pid).unwrap_or_else(|| "/".into());
        let abs = mavex_vfs::normalize(&cwd, path);
        match s.vfs.lstat(&abs) {
            Ok(st) => {
                let kind = match st.kind {
                    mavex_vfs::Kind::File => 0u32,
                    mavex_vfs::Kind::Dir => 1,
                    _ => 2,
                };
                let mut b = Vec::new();
                for v in [st.mode, st.uid, st.gid, kind, st.size as u32, (st.size >> 32) as u32] {
                    b.extend_from_slice(&v.to_le_bytes());
                }
                stage(b)
            }
            Err(e) => errno_code(e),
        }
    })
}

#[no_mangle]
pub extern "C" fn k_result_len() -> usize {
    RESULT.with(|r| r.borrow().len())
}

#[no_mangle]
pub extern "C" fn k_fd_dup(pid: u32, fd: u32) -> i32 {
    with_sys(|s| wr(s.fd_dup(pid, fd), |n| n as i32))
}

#[no_mangle]
pub extern "C" fn k_proc_ready(pid: u32) -> i32 {
    with_sys(|s| s.proc_ready(pid)) as i32
}

#[no_mangle]
pub extern "C" fn k_fd_close(pid: u32, fd: u32) -> i32 {
    with_sys(|s| wr(s.fd_close(pid, fd), |_| 0))
}

#[no_mangle]
pub extern "C" fn k_fd_read(pid: u32, fd: u32, len: usize) -> i32 {
    with_sys(|s| wr(s.fd_read(pid, fd, len), stage))
}

#[no_mangle]
pub unsafe extern "C" fn k_fd_write(pid: u32, fd: u32, p: *const u8, n: usize) -> i32 {
    let data = std::slice::from_raw_parts(p, n);
    with_sys(|s| wr(s.fd_write(pid, fd, data), |n| n as i32))
}

#[no_mangle]
pub extern "C" fn k_fd_pread(pid: u32, fd: u32, len: usize, off: f64) -> i32 {
    with_sys(|s| wr(s.fd_pread(pid, fd, len, off as u64), stage))
}

#[no_mangle]
pub unsafe extern "C" fn k_fd_pwrite(pid: u32, fd: u32, p: *const u8, n: usize, off: f64) -> i32 {
    let data = std::slice::from_raw_parts(p, n);
    with_sys(|s| wr(s.fd_pwrite(pid, fd, data, off as u64), |n| n as i32))
}

/// Returns the new offset, or a negative errno.
#[no_mangle]
pub extern "C" fn k_fd_seek(pid: u32, fd: u32, delta: f64, whence: u32) -> f64 {
    match with_sys(|s| s.fd_seek(pid, fd, delta as i64, whence as u8)) {
        Ok(o) => o as f64,
        Err(e) => -(e as f64),
    }
}

#[no_mangle]
pub extern "C" fn k_fd_set_size(pid: u32, fd: u32, len: f64) -> i32 {
    with_sys(|s| wr(s.fd_set_size(pid, fd, len as u64), |_| 0))
}

fn filestat_bytes(st: mavex_vfs::Stat) -> Vec<u8> {
    let mut b = Vec::with_capacity(64);
    b.extend_from_slice(&0u64.to_le_bytes());
    b.extend_from_slice(&(st.ino as u64).to_le_bytes());
    let ft = match st.kind {
        Kind::File => 4u8,
        Kind::Dir => 3,
        Kind::Symlink => 7,
    };
    b.push(ft);
    b.extend_from_slice(&[0u8; 7]);
    b.extend_from_slice(&1u64.to_le_bytes());
    b.extend_from_slice(&st.size.to_le_bytes());
    for _ in 0..3 {
        b.extend_from_slice(&st.mtime.to_le_bytes());
    }
    b
}

#[no_mangle]
pub extern "C" fn k_fd_filestat(pid: u32, fd: u32) -> i32 {
    with_sys(|s| wr(s.fd_stat(pid, fd), |st| stage(filestat_bytes(st))))
}

/// Returns filetype | (append << 8) or a negative errno.
#[no_mangle]
pub extern "C" fn k_fd_fdstat(pid: u32, fd: u32) -> i32 {
    with_sys(|s| wr(s.fd_type(pid, fd), |(t, a)| t as i32 | ((a as i32) << 8)))
}

#[no_mangle]
pub extern "C" fn k_fd_prestat(pid: u32, fd: u32) -> i32 {
    with_sys(|s| wr(s.fd_prestat(pid, fd), |n| stage(n.into_bytes())))
}

/// Stages WASI dirent records starting at `cookie` (an entry index); returns the byte count.
#[no_mangle]
pub extern "C" fn k_fd_readdir(pid: u32, fd: u32, cookie: f64, bufsize: usize) -> i32 {
    with_sys(|s| {
        wr(s.fd_readdir(pid, fd), |ents| {
            let mut out = Vec::new();
            for (i, e) in ents.iter().enumerate().skip(cookie as usize) {
                if out.len() >= bufsize {
                    break;
                }
                out.extend_from_slice(&((i + 1) as u64).to_le_bytes());
                out.extend_from_slice(&e.ino.to_le_bytes());
                out.extend_from_slice(&(e.name.len() as u32).to_le_bytes());
                out.push(e.ftype);
                out.extend_from_slice(&[0u8; 3]);
                out.extend_from_slice(e.name.as_bytes());
            }
            out.truncate(bufsize);
            stage(out)
        })
    })
}

#[no_mangle]
pub unsafe extern "C" fn k_path_open(pid: u32, dirfd: u32, p: *const u8, n: usize, oflags: u32, fdflags: u32, read: u32, write: u32) -> i32 {
    let path = str_arg(p, n);
    with_sys(|s| {
        wr(
            s.path_open(pid, dirfd, path, oflags & 1 != 0, oflags & 2 != 0, oflags & 4 != 0, oflags & 8 != 0, fdflags & 1 != 0, read != 0, write != 0),
            |fd| fd as i32,
        )
    })
}

#[no_mangle]
pub unsafe extern "C" fn k_path_mkdir(pid: u32, dirfd: u32, p: *const u8, n: usize) -> i32 {
    let path = str_arg(p, n);
    with_sys(|s| wr(s.path_mkdir(pid, dirfd, path), |_| 0))
}

#[no_mangle]
pub unsafe extern "C" fn k_path_rmdir(pid: u32, dirfd: u32, p: *const u8, n: usize) -> i32 {
    let path = str_arg(p, n);
    with_sys(|s| wr(s.path_rmdir(pid, dirfd, path), |_| 0))
}

#[no_mangle]
pub unsafe extern "C" fn k_path_unlink(pid: u32, dirfd: u32, p: *const u8, n: usize) -> i32 {
    let path = str_arg(p, n);
    with_sys(|s| wr(s.path_unlink(pid, dirfd, path), |_| 0))
}

#[no_mangle]
pub unsafe extern "C" fn k_path_rename(pid: u32, d1: u32, p1: *const u8, n1: usize, d2: u32, p2: *const u8, n2: usize) -> i32 {
    let (a, b) = (str_arg(p1, n1), str_arg(p2, n2));
    with_sys(|s| wr(s.path_rename(pid, d1, a, d2, b), |_| 0))
}

#[no_mangle]
pub unsafe extern "C" fn k_path_symlink(pid: u32, t: *const u8, tn: usize, dirfd: u32, p: *const u8, n: usize) -> i32 {
    let (target, path) = (str_arg(t, tn), str_arg(p, n));
    with_sys(|s| wr(s.path_symlink(pid, target, dirfd, path), |_| 0))
}

#[no_mangle]
pub unsafe extern "C" fn k_path_readlink(pid: u32, dirfd: u32, p: *const u8, n: usize) -> i32 {
    let path = str_arg(p, n);
    with_sys(|s| wr(s.path_readlink(pid, dirfd, path), |t| stage(t.into_bytes())))
}

#[no_mangle]
pub unsafe extern "C" fn k_path_filestat(pid: u32, dirfd: u32, p: *const u8, n: usize, follow: u32) -> i32 {
    let path = str_arg(p, n);
    with_sys(|s| wr(s.path_stat(pid, dirfd, path, follow != 0), |st| stage(filestat_bytes(st))))
}

// ---- shell ----
struct KHost;

impl Host for KHost {
    fn with_vfs(&self, f: &mut dyn FnMut(&mut Vfs)) {
        with_sys(|s| {
            s.enter_default();
            f(&mut s.vfs)
        });
    }

    fn find_exe(&self, name: &str, path: &str, cwd: &str) -> Option<String> {
        with_sys(|s| s.which(name, path, cwd))
    }

    fn exec(&self, req: &ExecReq) -> Option<ExecRes> {
        let path = req.env.get("PATH").cloned().unwrap_or_default();
        let mut env: Vec<String> = req.env.iter().filter(|(k, _)| k.as_str() != "PWD").map(|(k, v)| format!("{}={}", k, v)).collect();
        env.push(format!("PWD={}", req.cwd));
        let (pid, out_pipe) = with_sys(|s| {
            let exe = s.which(&req.argv[0], &path, req.cwd)?;
            s.tty_out.clear();
            let stdin = match req.stdin {
                Some(t) => {
                    let id = s.new_pipe();
                    s.pipe_fill(id, t.as_bytes());
                    Stdio::PipeR(id)
                }
                None => Stdio::Tty,
            };
            let (stdout, out_pipe) = if req.tty {
                (Stdio::Tty, None)
            } else {
                let id = s.new_pipe();
                s.pipe_host_reader(id);
                (Stdio::PipeW(id), Some(id))
            };
            let pid = s.spawn(0, exe, req.argv.to_vec(), env, req.cwd.to_string(), [stdin, stdout, Stdio::Tty]).ok()?;
            Some((pid, out_pipe))
        })?;
        unsafe { run_process(pid) };
        Some(with_sys(|s| {
            let code = s.wait(pid).unwrap_or(255);
            let tty = String::from_utf8_lossy(&std::mem::take(&mut s.tty_out)).into_owned();
            match out_pipe {
                Some(id) => {
                    let out = String::from_utf8_lossy(&s.pipe_drain(id)).into_owned();
                    s.pipe_release(id);
                    ExecRes { out, err: tty, code }
                }
                None => ExecRes { out: tty, err: String::new(), code },
            }
        }))
    }
}

#[no_mangle]
pub unsafe extern "C" fn k_shell_run(p: *const u8, n: usize) -> i32 {
    let line = str_arg(p, n);
    let out = SHELL.with(|s| s.borrow_mut().run_line(&KHost, line));
    stage(out.into_bytes())
}

#[no_mangle]
pub extern "C" fn k_shell_prompt() -> i32 {
    stage(SHELL.with(|s| s.borrow().prompt()).into_bytes())
}

#[no_mangle]
pub unsafe extern "C" fn k_shell_complete(p: *const u8, n: usize, is_first: i32) -> i32 {
    let word = str_arg(p, n);
    let c = with_sys(|sys| SHELL.with(|s| s.borrow().complete(&sys.vfs, word, is_first != 0)));
    stage(c.join("\n").into_bytes())
}

#[no_mangle]
pub extern "C" fn k_shell_exited() -> i32 {
    SHELL.with(|s| s.borrow().exited) as i32
}

#[no_mangle]
pub extern "C" fn k_shell_reset() {
    SHELL.with(|s| {
        let cols = s.borrow().cols;
        *s.borrow_mut() = Shell::new();
        s.borrow_mut().cols = cols;
    });
}

#[no_mangle]
pub extern "C" fn k_tty_resize(cols: u32) {
    SHELL.with(|s| s.borrow_mut().cols = cols.clamp(20, 500));
}

#[no_mangle]
pub extern "C" fn kmain() {
    let ver = env!("CARGO_PKG_VERSION");
    print(&format!("MavexOS {} (wasm32) - kernel starting\n", ver));
    print("[ ok ] first boot on this device: no saved state found\n");
    print("[ ok ] memory: wasm linear memory, RAM-only (nothing persists yet)\n");
    with_sys(|s| {
        let v = &mut s.vfs;
        for d in ["/bin", "/etc", "/home", "/home/user", "/tmp", "/usr"] {
            v.mkdir(d, 0o755).unwrap();
        }
        v.chmod("/tmp", 0o1777).unwrap();
        v.write_file("/etc/hostname", b"mavex\n").unwrap();
        v.write_file("/etc/os-release", format!("NAME=\"MavexOS\"\nPRETTY_NAME=\"MavexOS {0}\"\nVERSION=\"{0}\"\nVERSION_ID=\"{0}\"\nID=mavexos\nHOME_URL=\"https://github.com/MavexOS\"\n", ver).as_bytes()).unwrap();
        v.write_file("/etc/motd", b"Welcome to MavexOS. Type 'help' for commands.\n").unwrap();
        v.chown("/home/user", 1000, 1000).unwrap();
        v.mkdir("/root", 0o700).unwrap();
        v.write_file("/etc/passwd", b"root:x:0:0:root:/root:/bin/brush\nuser:x:1000:1000:User:/home/user:/bin/brush\n").unwrap();
        v.write_file("/etc/group", b"root:x:0:\nsudo:x:27:user\nuser:x:1000:\n").unwrap();
        v.write_file("/etc/shadow", b"root::19000:0:99999:7:::\nuser:*:19000:0:99999:7:::\n").unwrap();
        v.chmod("/etc/shadow", 0o600).unwrap();
        v.write_file("/etc/sudoers", b"# MavexOS sudoers (no password prompts: ttys cannot block yet)\nroot ALL=(ALL) ALL\n%sudo ALL=(ALL) NOPASSWD: ALL\n").unwrap();
        v.chmod("/etc/sudoers", 0o440).unwrap();
        s.set_default_creds(1000, 1000);
    });
    print("[ ok ] vfs: in-memory root filesystem mounted at /\n");
    print("[ ok ] process layer: pid table, fds, pipes, tty ready\n");
    print("[ ok ] created /home/user (uid 1000)\n");
}

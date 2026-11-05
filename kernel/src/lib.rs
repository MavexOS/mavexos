use mavex_vfs::{Errno, Vfs};
use mavex_shell::Shell;
use std::cell::RefCell;

#[link(wasm_import_module = "host")]
extern "C" {
    fn console_write(ptr: *const u8, len: usize);
}

fn print(s: &str) {
    unsafe { console_write(s.as_ptr(), s.len()) }
}

thread_local! {
    static VFS: RefCell<Vfs> = RefCell::new(Vfs::new());
    static SHELL: RefCell<Shell> = RefCell::new(Shell::new());
    static RESULT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
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
pub unsafe extern "C" fn k_mkdir(p: *const u8, n: usize) -> i32 {
    let path = str_arg(p, n);
    VFS.with(|v| ret(v.borrow_mut().mkdir(path, 0o755)))
}

#[no_mangle]
pub unsafe extern "C" fn k_write_file(p: *const u8, n: usize, d: *const u8, dn: usize) -> i32 {
    let path = str_arg(p, n);
    let data = std::slice::from_raw_parts(d, dn);
    VFS.with(|v| ret(v.borrow_mut().write_file(path, data)))
}

#[no_mangle]
pub unsafe extern "C" fn k_unlink(p: *const u8, n: usize) -> i32 {
    let path = str_arg(p, n);
    VFS.with(|v| ret(v.borrow_mut().unlink(path)))
}

/// Stages the file's bytes in the result buffer; returns the length or -errno.
#[no_mangle]
pub unsafe extern "C" fn k_read_file(p: *const u8, n: usize) -> i32 {
    let path = str_arg(p, n);
    match VFS.with(|v| v.borrow().read_file(path)) {
        Ok(d) => {
            let len = d.len() as i32;
            RESULT.with(|r| *r.borrow_mut() = d);
            len
        }
        Err(e) => errno_code(e),
    }
}

/// Stages newline-separated entry names; returns the length or -errno.
#[no_mangle]
pub unsafe extern "C" fn k_readdir(p: *const u8, n: usize) -> i32 {
    let path = str_arg(p, n);
    match VFS.with(|v| v.borrow().readdir(path)) {
        Ok(names) => {
            let d = names.join("\n").into_bytes();
            let len = d.len() as i32;
            RESULT.with(|r| *r.borrow_mut() = d);
            len
        }
        Err(e) => errno_code(e),
    }
}

#[no_mangle]
pub extern "C" fn k_result_ptr() -> *const u8 {
    RESULT.with(|r| r.borrow().as_ptr())
}

fn stage(d: Vec<u8>) -> i32 {
    let len = d.len() as i32;
    RESULT.with(|r| *r.borrow_mut() = d);
    len
}

#[no_mangle]
pub unsafe extern "C" fn k_shell_run(p: *const u8, n: usize) -> i32 {
    let line = str_arg(p, n);
    let out = VFS.with(|v| SHELL.with(|s| s.borrow_mut().run_line(&mut v.borrow_mut(), line)));
    stage(out.into_bytes())
}

#[no_mangle]
pub extern "C" fn k_shell_prompt() -> i32 {
    stage(SHELL.with(|s| s.borrow().prompt()).into_bytes())
}

/// Stages newline-separated completion candidates for `word`.
#[no_mangle]
pub unsafe extern "C" fn k_shell_complete(p: *const u8, n: usize, is_first: i32) -> i32 {
    let word = str_arg(p, n);
    let c = VFS.with(|v| SHELL.with(|s| s.borrow().complete(&v.borrow(), word, is_first != 0)));
    stage(c.join("\n").into_bytes())
}

#[no_mangle]
pub extern "C" fn k_shell_exited() -> i32 {
    SHELL.with(|s| s.borrow().exited) as i32
}

#[no_mangle]
pub extern "C" fn k_shell_reset() {
    SHELL.with(|s| *s.borrow_mut() = Shell::new());
}

#[no_mangle]
pub extern "C" fn kmain() {
    let ver = env!("CARGO_PKG_VERSION");
    print(&format!("MavexOS {} (wasm32) - kernel starting\n", ver));
    print("[ ok ] first boot on this device: no saved state found\n");
    print("[ ok ] memory: wasm linear memory, RAM-only (nothing persists yet)\n");
    VFS.with(|v| {
        let mut v = v.borrow_mut();
        for d in ["/bin", "/etc", "/home", "/home/user", "/tmp", "/usr"] {
            v.mkdir(d, 0o755).unwrap();
        }
        v.chmod("/tmp", 0o1777).unwrap();
        v.write_file("/etc/hostname", b"mavex\n").unwrap();
        v.write_file("/etc/os-release", format!("NAME=MavexOS\nVERSION={}\nID=mavexos\n", ver).as_bytes()).unwrap();
        v.write_file("/etc/motd", b"Welcome to MavexOS. Type 'help' for commands.\n").unwrap();
        v.write_file("/bin/msh", b"").unwrap();
        v.chmod("/bin/msh", 0o755).unwrap();
        for c in mavex_shell::COMMANDS {
            let _ = v.symlink("/bin/msh", &format!("/bin/{}", c));
        }
        v.chown("/home/user", 1000, 1000).unwrap();
        v.set_creds(1000, 1000);
    });
    print("[ ok ] vfs: in-memory root filesystem mounted at /\n");
    print("[ ok ] created /home/user (uid 1000)\n");
    print("[ ok ] started /bin/msh\n");
}

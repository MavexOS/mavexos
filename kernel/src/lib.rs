use mavex_vfs::{Errno, Vfs};
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

#[no_mangle]
pub extern "C" fn kmain() {
    print("MavexOS kernel booting...\n");
    VFS.with(|v| {
        let mut v = v.borrow_mut();
        for d in ["/bin", "/etc", "/home", "/home/user", "/tmp", "/usr"] {
            v.mkdir(d, 0o755).unwrap();
        }
        v.write_file("/etc/hostname", b"mavex\n").unwrap();
        v.write_file("/etc/os-release", b"NAME=MavexOS\n").unwrap();
    });
    print("kernel: vfs mounted at /\n");
    print("mavex$ ");
}

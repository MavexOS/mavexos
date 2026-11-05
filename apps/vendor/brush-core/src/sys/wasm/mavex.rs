//! MavexOS kernel bridge: pipes and process spawning through host imports.
use std::io::{Read, Write};
use std::os::fd::{FromRawFd, OwnedFd, AsRawFd, BorrowedFd, AsFd};
use std::sync::Arc;

#[link(wasm_import_module = "mavex")]
unsafe extern "C" {
    fn pipe(out: *mut u32) -> i32;
    fn spawn(prog: *const u8, prog_n: usize, argv: *const u8, argv_n: usize, env: *const u8, env_n: usize,
             cwd: *const u8, cwd_n: usize, s0: i32, s1: i32, s2: i32) -> i32;
    fn ready(pid: u32) -> i32;
    fn run(pid: u32) -> i32;
    fn wait(pid: u32) -> i32;
    fn dup(fd: u32) -> i32;
}

#[derive(Clone)]
pub struct PipeReader(Arc<OwnedFd>);
#[derive(Clone)]
pub struct PipeWriter(Arc<OwnedFd>);

impl PipeReader {
    pub fn try_clone(&self) -> std::io::Result<Self> { Ok(self.clone()) }
}
impl PipeWriter {
    pub fn try_clone(&self) -> std::io::Result<Self> { Ok(self.clone()) }
}
impl AsRawFd for PipeReader { fn as_raw_fd(&self) -> i32 { self.0.as_raw_fd() } }
impl AsRawFd for PipeWriter { fn as_raw_fd(&self) -> i32 { self.0.as_raw_fd() } }
impl AsFd for PipeReader { fn as_fd(&self) -> BorrowedFd<'_> { self.0.as_fd() } }
impl AsFd for PipeWriter { fn as_fd(&self) -> BorrowedFd<'_> { self.0.as_fd() } }

fn file_of(fd: &Arc<OwnedFd>) -> std::mem::ManuallyDrop<std::fs::File> {
    std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(fd.as_raw_fd()) })
}

impl Read for PipeReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> { file_of(&self.0).read(buf) }
}
impl Write for PipeWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> { file_of(&self.0).write(buf) }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

pub fn make_pipe() -> std::io::Result<(PipeReader, PipeWriter)> {
    let mut out = [0u32; 2];
    let r = unsafe { pipe(out.as_mut_ptr()) };
    if r < 0 { return Err(std::io::Error::from_raw_os_error(-r)); }
    unsafe { Ok((PipeReader(Arc::new(OwnedFd::from_raw_fd(out[0] as i32))), PipeWriter(Arc::new(OwnedFd::from_raw_fd(out[1] as i32))))) }
}

/// Registers a child process (it does not run until `run_to_exit`).
pub fn spawn_child(prog: &str, argv: &[String], env: &[String], cwd: &str, fds: [i32; 3]) -> std::io::Result<u32> {
    let join = |v: &[String]| { let mut b = Vec::new(); for s in v { b.extend_from_slice(s.as_bytes()); b.push(0); } b };
    let (a, e) = (join(argv), join(env));
    let r = unsafe { spawn(prog.as_ptr(), prog.len(), a.as_ptr(), a.len(), e.as_ptr(), e.len(), cwd.as_ptr(), cwd.len(), fds[0], fds[1], fds[2]) };
    if r < 0 { Err(std::io::Error::from_raw_os_error(-r)) } else { Ok(r as u32) }
}

pub fn child_ready(pid: u32) -> bool { unsafe { ready(pid) != 0 } }
pub fn run_child(pid: u32) -> i32 { unsafe { run(pid); wait(pid) } }

pub fn dup_file(f: &std::fs::File) -> std::io::Result<std::fs::File> {
    let r = unsafe { dup(f.as_raw_fd() as u32) };
    if r < 0 { Err(std::io::Error::from_raw_os_error(-r)) } else { Ok(unsafe { std::fs::File::from_raw_fd(r) }) }
}

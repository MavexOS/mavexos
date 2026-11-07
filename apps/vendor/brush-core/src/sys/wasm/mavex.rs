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
    fn kill(pid: i32, sig: i32) -> i32;
    fn child_state(pid: u32) -> i32;
    fn wait_change(ms: u32) -> i32;
    fn tcsetpgrp(pgid: u32) -> i32;
    fn tcgetpgrp() -> i32;
    fn setpgid(pid: u32, pgid: u32) -> i32;
    fn getpgid(pid: u32) -> i32;
    fn poll_child(pid: u32) -> i32;
}

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;

thread_local! {
    static PG_SPEC: Cell<Option<i32>> = const { Cell::new(None) };
    static FG: Cell<bool> = const { Cell::new(false) };
    static LIVE: RefCell<BTreeSet<u32>> = const { RefCell::new(BTreeSet::new()) };
    static REPORTED: RefCell<BTreeSet<u32>> = const { RefCell::new(BTreeSet::new()) };
}

/// Records the process-group request for the next spawn (0 = new group, n = join group n).
pub fn set_pg_spec(pg: i32) { PG_SPEC.with(|c| c.set(Some(pg))); }
pub fn set_take_fg() { FG.with(|c| c.set(true)); }

pub fn send_signal(pid: i32, sig: i32) -> i32 { unsafe { kill(pid, sig) } }
pub fn tc_set_fg(pgid: u32) { unsafe { tcsetpgrp(pgid); } }
pub fn tc_get_fg() -> i32 { unsafe { tcgetpgrp() } }
pub fn get_pgid(pid: u32) -> i32 { unsafe { getpgid(pid) } }

/// True when a tracked child is stopped and the stop has not been reported yet.
pub fn has_unreported_stop() -> bool {
    LIVE.with(|l| l.borrow().iter().any(|&p| unsafe { child_state(p) } == 1 && !REPORTED.with(|r| r.borrow().contains(&p))))
}

pub fn take_unreported_stop() -> bool {
    let stopped: Vec<u32> = LIVE.with(|l| l.borrow().iter().copied().filter(|&p| unsafe { child_state(p) } == 1 && !REPORTED.with(|r| r.borrow().contains(&p))).collect());
    for p in &stopped { REPORTED.with(|r| r.borrow_mut().insert(*p)); }
    !stopped.is_empty()
}

pub fn child_stopped(pid: u32) -> bool { unsafe { child_state(pid) == 1 } }
pub fn child_gone(pid: u32) -> bool { (unsafe { child_state(pid) }) >= 2 }
pub fn clear_reported(pid: u32) { REPORTED.with(|r| r.borrow_mut().remove(&pid)); }
pub fn untrack(pid: u32) { LIVE.with(|l| l.borrow_mut().remove(&pid)); REPORTED.with(|r| r.borrow_mut().remove(&pid)); }
thread_local! { static DRIVING: Cell<bool> = const { Cell::new(false) }; }
/// While set, waits only yield instead of parking (used to give background jobs a turn between lines).
pub fn set_driving(on: bool) { DRIVING.with(|c| c.set(on)); }
pub fn block_until_change(ms: u32) { if !DRIVING.with(|c| c.get()) { unsafe { wait_change(ms); } } }
pub fn reap(pid: u32) -> i32 { unsafe { poll_child(pid) } }

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
    if r < 0 { return Err(std::io::Error::from_raw_os_error(-r)); }
    let pid = r as u32;
    LIVE.with(|l| l.borrow_mut().insert(pid));
    if let Some(pg) = PG_SPEC.with(|c| c.take()) {
        let target = if pg == 0 { pid } else { pg as u32 };
        unsafe { setpgid(pid, target); }
        if FG.with(|c| c.take()) { unsafe { tcsetpgrp(target); } }
    }
    FG.with(|c| c.set(false));
    Ok(pid)
}

pub fn child_ready(pid: u32) -> bool { unsafe { ready(pid) != 0 } }
pub fn run_child(pid: u32) -> i32 { unsafe { run(pid); wait(pid) } }


pub fn dup_file(f: &std::fs::File) -> std::io::Result<std::fs::File> {
    let r = unsafe { dup(f.as_raw_fd() as u32) };
    if r < 0 { Err(std::io::Error::from_raw_os_error(-r)) } else { Ok(unsafe { std::fs::File::from_raw_fd(r) }) }
}

/// The calling process's pid (std::process::id is unsupported on wasip1).
pub fn self_pid() -> u32 {
    std::fs::read_to_string("/proc/self/stat").ok()
        .and_then(|s| s.split_whitespace().next().and_then(|p| p.parse().ok()))
        .unwrap_or(1)
}

//! Process, fd, pipe and tty layer. Error numbers are WASI errnos so the JS bridge can pass them straight through.
use mavex_vfs::{normalize, Errno as VErr, Ino, Kind, Stat, Vfs};
use std::collections::{BTreeMap, VecDeque};

pub mod net;
pub mod procfs;

pub mod errno {
    pub const ACCES: u16 = 2;
    pub const PERM: u16 = 63;
    pub const AGAIN: u16 = 6;
    pub const BADF: u16 = 8;
    pub const CHILD: u16 = 10;
    pub const EXIST: u16 = 20;
    pub const INVAL: u16 = 28;
    pub const ISDIR: u16 = 31;
    pub const LOOP: u16 = 32;
    pub const NOENT: u16 = 44;
    pub const NOEXEC: u16 = 45;
    pub const NOTDIR: u16 = 54;
    pub const NOTEMPTY: u16 = 55;
    pub const PIPE: u16 = 64;
    pub const SPIPE: u16 = 70;
    pub const SRCH: u16 = 71;
}

pub mod ftype {
    pub const UNKNOWN: u8 = 0;
    pub const CHAR: u8 = 2;
    pub const DIR: u8 = 3;
    pub const REG: u8 = 4;
    pub const SYMLINK: u8 = 7;
}

const PIPE_CAP: usize = 4 << 20;

pub type R<T> = Result<T, u16>;

fn verr(e: VErr) -> u16 {
    match e {
        VErr::NoEnt => errno::NOENT,
        VErr::Exist => errno::EXIST,
        VErr::NotDir => errno::NOTDIR,
        VErr::IsDir => errno::ISDIR,
        VErr::NotEmpty => errno::NOTEMPTY,
        VErr::Inval => errno::INVAL,
        VErr::Loop => errno::LOOP,
        VErr::Acces => errno::ACCES,
        VErr::Perm => errno::PERM,
    }
}

fn kind_ftype(k: Kind) -> u8 {
    match k {
        Kind::File => ftype::REG,
        Kind::Dir => ftype::DIR,
        Kind::Symlink => ftype::SYMLINK,
    }
}

#[derive(Clone, Debug)]
enum FdKind {
    File { ino: Ino, off: u64, append: bool, read: bool, write: bool },
    Dir { path: String },
    PipeR(usize),
    PipeW(usize),
    Tty,
    Null,
}

#[derive(Clone, Debug)]
struct Fd {
    kind: FdKind,
    preopen: Option<String>,
}

struct Pipe {
    buf: VecDeque<u8>,
    readers: u32,
    writers: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Stdio {
    /// Duplicate the parent's fd with this number.
    Fd(u32),
    Tty,
    Null,
    PipeR(usize),
    PipeW(usize),
}

pub struct Proc {
    pub ppid: u32,
    pub argv: Vec<String>,
    pub env: Vec<String>,
    pub cwd: String,
    pub exe: String,
    pub exit: Option<i32>,
    pub uid: u32,
    pub gid: u32,
    pub euid: u32,
    pub egid: u32,
    pub cpu_ms: u64,
    pub rss_kib: u64,
    pub state: u8,
    pub start_ms: u64,
    pub pgid: u32,
    pub sid: u32,
    fds: BTreeMap<u32, Fd>,
}

pub struct Sys {
    pub vfs: Vfs,
    procs: BTreeMap<u32, Proc>,
    pipes: Vec<Option<Pipe>>,
    next_pid: u32,
    default_cred: (u32, u32),
    pub net: net::Net,
    pub hostinfo: procfs::HostInfo,
    pub tty_out: Vec<u8>,
    pub tty_in: VecDeque<u8>,
    pub tty_eof: bool,
    pub tty_fg: u32,
}

pub struct DirEnt {
    pub name: String,
    pub ino: u64,
    pub ftype: u8,
}

impl Sys {
    pub fn new(vfs: Vfs) -> Self {
        Sys { vfs, procs: BTreeMap::new(), pipes: Vec::new(), next_pid: 1, default_cred: (0, 0), net: net::Net::new(), hostinfo: procfs::HostInfo::default(), tty_out: Vec::new(), tty_in: VecDeque::new(), tty_eof: false, tty_fg: 0 }
    }

    pub fn set_default_creds(&mut self, uid: u32, gid: u32) {
        self.default_cred = (uid, gid);
        self.vfs.set_creds(uid, gid);
    }

    pub fn enter_default(&mut self) {
        let (u, g) = self.default_cred;
        self.vfs.set_creds(u, g);
    }

    pub fn as_root(&mut self) {
        self.vfs.set_creds(0, 0);
    }

    pub fn enter(&mut self, pid: u32) {
        let (u, g) = self.procs.get(&pid).map(|p| (p.euid, p.egid)).unwrap_or(self.default_cred);
        self.vfs.set_creds(u, g);
    }

    pub fn net_request(&mut self, pid: u32, req: &str) -> Result<String, (i32, String)> {
        let root = self.procs.get(&pid).is_some_and(|p| p.euid == 0);
        self.net.request(root, req)
    }

    pub fn proc_creds(&self, pid: u32) -> R<(u32, u32, u32, u32)> {
        let p = self.proc(pid)?;
        Ok((p.uid, p.gid, p.euid, p.egid))
    }

    /// Privileged (euid 0) credential change: sets real and effective ids.
    pub fn force_proc_creds(&mut self, pid: u32, uid: u32, gid: u32) -> R<()> {
        let p = self.procs.get_mut(&pid).ok_or(errno::CHILD)?;
        p.uid = uid;
        p.gid = gid;
        p.euid = uid;
        p.egid = gid;
        Ok(())
    }

    pub fn set_proc_creds(&mut self, pid: u32, uid: u32, gid: u32) -> R<()> {
        let p = self.procs.get_mut(&pid).ok_or(errno::CHILD)?;
        if p.euid != 0 {
            return Err(errno::PERM);
        }
        p.uid = uid;
        p.gid = gid;
        p.euid = uid;
        p.egid = gid;
        Ok(())
    }

    fn proc(&self, pid: u32) -> R<&Proc> {
        self.procs.get(&pid).ok_or(errno::CHILD)
    }

    fn fd(&self, pid: u32, fd: u32) -> R<&Fd> {
        self.proc(pid)?.fds.get(&fd).ok_or(errno::BADF)
    }

    pub fn proc_exe(&self, pid: u32) -> R<String> {
        Ok(self.proc(pid)?.exe.clone())
    }

    pub fn proc_args(&self, pid: u32) -> R<Vec<String>> {
        Ok(self.proc(pid)?.argv.clone())
    }

    pub fn proc_env(&self, pid: u32) -> R<Vec<String>> {
        Ok(self.proc(pid)?.env.clone())
    }

    pub fn fd_dup(&mut self, pid: u32, fd: u32) -> R<u32> {
        let f = self.procs.get(&pid).ok_or(errno::CHILD)?.fds.get(&fd).ok_or(errno::BADF)?.clone();
        self.open_fd(pid, Fd { preopen: None, ..f })
    }

    /// A process may run once its stdin pipe (if any) has no live writers.
    pub fn proc_ready(&self, pid: u32) -> bool {
        match self.procs.get(&pid).and_then(|p| p.fds.get(&0)).map(|f| &f.kind) {
            Some(FdKind::PipeR(i)) => self.pipes.get(*i).and_then(|p| p.as_ref()).is_none_or(|p| p.writers == 0),
            _ => true,
        }
    }

    /// True when a read (or write) on the fd would not block.
    pub fn fd_ready(&self, pid: u32, fd: u32, write: bool) -> bool {
        let Some(f) = self.procs.get(&pid).and_then(|p| p.fds.get(&fd)) else { return true };
        match &f.kind {
            FdKind::PipeR(i) if !write => self.pipes.get(*i).and_then(|p| p.as_ref()).is_none_or(|p| !p.buf.is_empty() || p.writers == 0),
            FdKind::PipeW(i) if write => self.pipes.get(*i).and_then(|p| p.as_ref()).is_none_or(|p| p.buf.len() < PIPE_CAP || p.readers == 0),
            FdKind::Tty if !write => !self.tty_in.is_empty() || self.tty_eof,
            _ => true,
        }
    }

    pub fn proc_exited(&self, pid: u32) -> bool {
        self.procs.get(&pid).is_none_or(|p| p.exit.is_some())
    }

    pub fn proc_ppid(&self, pid: u32) -> Option<u32> {
        self.procs.get(&pid).map(|p| p.ppid)
    }

    pub fn proc_cwd(&self, pid: u32) -> Option<String> {
        self.procs.get(&pid).map(|p| p.cwd.clone())
    }

    pub fn set_proc_stat(&mut self, pid: u32, cpu_ms: u64, rss_kib: u64, state: u8) {
        if let Some(p) = self.procs.get_mut(&pid) {
            p.cpu_ms = cpu_ms;
            p.rss_kib = rss_kib;
            p.state = state;
        }
    }

    pub fn proc_count(&self) -> usize {
        self.procs.len()
    }

    // ---- pipes ----
    pub fn new_pipe(&mut self) -> usize {
        let p = Pipe { buf: VecDeque::new(), readers: 0, writers: 0 };
        if let Some(i) = self.pipes.iter().position(|x| x.is_none()) {
            self.pipes[i] = Some(p);
            i
        } else {
            self.pipes.push(Some(p));
            self.pipes.len() - 1
        }
    }

    pub fn pipe_fill(&mut self, id: usize, data: &[u8]) {
        if let Some(Some(p)) = self.pipes.get_mut(id) {
            p.buf.extend(data);
        }
    }

    pub fn pipe_drain(&mut self, id: usize) -> Vec<u8> {
        match self.pipes.get_mut(id) {
            Some(Some(p)) => p.buf.drain(..).collect(),
            _ => Vec::new(),
        }
    }

    /// Marks the host as a reader so children writing to the pipe don't get EPIPE.
    pub fn pipe_host_reader(&mut self, id: usize) {
        if let Some(Some(p)) = self.pipes.get_mut(id) {
            p.readers += 1;
        }
    }

    pub fn pipe_release(&mut self, id: usize) {
        if let Some(slot) = self.pipes.get_mut(id) {
            if slot.as_ref().is_some_and(|p| p.writers == 0) {
                *slot = None;
            }
        }
    }

    fn pipe_ref(&mut self, kind: &FdKind, delta: i32) {
        let (id, writer) = match kind {
            FdKind::PipeR(i) => (*i, false),
            FdKind::PipeW(i) => (*i, true),
            _ => return,
        };
        if let Some(Some(p)) = self.pipes.get_mut(id) {
            let c = if writer { &mut p.writers } else { &mut p.readers };
            *c = (*c as i32 + delta).max(0) as u32;
        }
    }

    fn open_fd(&mut self, pid: u32, fd: Fd) -> R<u32> {
        self.pipe_ref(&fd.kind.clone(), 1);
        let p = self.procs.get_mut(&pid).ok_or(errno::CHILD)?;
        let n = (0u32..).find(|n| !p.fds.contains_key(n)).unwrap();
        p.fds.insert(n, fd);
        Ok(n)
    }

    pub fn create_pipe(&mut self, pid: u32) -> R<(u32, u32)> {
        let id = self.new_pipe();
        let r = self.open_fd(pid, Fd { kind: FdKind::PipeR(id), preopen: None })?;
        let w = self.open_fd(pid, Fd { kind: FdKind::PipeW(id), preopen: None })?;
        Ok((r, w))
    }

    // ---- processes ----
    pub fn spawn(&mut self, ppid: u32, exe: String, argv: Vec<String>, env: Vec<String>, cwd: String, stdio: [Stdio; 3]) -> R<u32> {
        let pid = self.next_pid;
        let (uid, gid, mut euid, mut egid) = match self.procs.get(&ppid) {
            Some(pp) => (pp.uid, pp.gid, pp.euid, pp.egid),
            None => (self.default_cred.0, self.default_cred.1, self.default_cred.0, self.default_cred.1),
        };
        self.enter(ppid);
        if let Ok(st) = self.vfs.stat(&exe) {
            if self.vfs.check_access(st.ino, 1).is_err() {
                return Err(errno::ACCES);
            }
            if st.mode & 0o4000 != 0 {
                euid = st.uid;
            }
            if st.mode & 0o2000 != 0 {
                egid = st.gid;
            }
        }
        self.next_pid += 1;
        let (pgid, sid) = self.procs.get(&ppid).map_or((pid, pid), |pp| (pp.pgid, pp.sid));
        self.procs.insert(pid, Proc { pgid, sid, ppid, argv, env, cwd: cwd.clone(), exe, exit: None, uid, gid, euid, egid, cpu_ms: 0, rss_kib: 0, state: b'R', start_ms: self.hostinfo.now_ns / 1_000_000, fds: BTreeMap::new() });
        for (i, s) in stdio.iter().enumerate() {
            let kind = match *s {
                Stdio::Tty => FdKind::Tty,
                Stdio::Null => FdKind::Null,
                Stdio::PipeR(id) => FdKind::PipeR(id),
                Stdio::PipeW(id) => FdKind::PipeW(id),
                Stdio::Fd(n) => match self.procs.get(&ppid).and_then(|p| p.fds.get(&n)) {
                    Some(f) => f.kind.clone(),
                    None => FdKind::Null,
                },
            };
            self.pipe_ref(&kind, 1);
            self.procs.get_mut(&pid).unwrap().fds.insert(i as u32, Fd { kind, preopen: None });
        }
        let p = self.procs.get_mut(&pid).unwrap();
        p.fds.insert(3, Fd { kind: FdKind::Dir { path: "/".into() }, preopen: Some("/".into()) });
        p.fds.insert(4, Fd { kind: FdKind::Dir { path: cwd }, preopen: Some(".".into()) });
        Ok(pid)
    }

    pub fn setpgid(&mut self, pid: u32, pgid: u32) -> R<()> {
        let pg = if pgid == 0 { pid } else { pgid };
        let p = self.procs.get_mut(&pid).ok_or(errno::SRCH)?;
        p.pgid = pg;
        Ok(())
    }

    pub fn setsid(&mut self, pid: u32) -> R<u32> {
        let p = self.procs.get_mut(&pid).ok_or(errno::SRCH)?;
        p.sid = pid;
        p.pgid = pid;
        Ok(pid)
    }

    pub fn pgid_of(&self, pid: u32) -> Option<u32> {
        self.procs.get(&pid).map(|p| p.pgid)
    }

    pub fn sid_of(&self, pid: u32) -> Option<u32> {
        self.procs.get(&pid).map(|p| p.sid)
    }

    /// Live (not exited) members of a process group.
    pub fn group_members(&self, pgid: u32) -> Vec<u32> {
        self.procs.iter().filter(|(_, p)| p.pgid == pgid && p.exit.is_none()).map(|(k, _)| *k).collect()
    }

    pub fn children_of(&self, ppid: u32) -> Vec<u32> {
        self.procs.iter().filter(|(_, p)| p.ppid == ppid).map(|(k, _)| *k).collect()
    }

    fn close_all(&mut self, pid: u32) {
        let kinds: Vec<FdKind> = match self.procs.get_mut(&pid) {
            Some(p) => std::mem::take(&mut p.fds).into_values().map(|f| f.kind).collect(),
            None => return,
        };
        for k in kinds {
            self.pipe_ref(&k, -1);
        }
    }

    pub fn exit(&mut self, pid: u32, code: i32) {
        self.close_all(pid);
        if let Some(p) = self.procs.get_mut(&pid) {
            p.exit = Some(code);
        }
    }

    /// Returns the exit code and removes the process; errors if it is unknown or still running.
    pub fn wait(&mut self, pid: u32) -> R<i32> {
        match self.procs.get(&pid) {
            Some(Proc { exit: Some(c), .. }) => {
                let c = *c;
                self.procs.remove(&pid);
                Ok(c)
            }
            Some(_) => Err(errno::AGAIN),
            None => Err(errno::CHILD),
        }
    }

    /// Finds an executable file by name using a PATH-style list; absolute or relative-with-slash names are used as-is.
    pub fn which(&self, name: &str, path: &str, cwd: &str) -> Option<String> {
        let ok = |p: &str| self.vfs.stat(p).map(|s| s.kind == Kind::File && s.mode & 0o111 != 0).unwrap_or(false);
        if name.contains('/') {
            let abs = normalize(cwd, name);
            return ok(&abs).then_some(abs);
        }
        path.split(':').filter(|d| !d.is_empty()).map(|d| normalize("/", &format!("{}/{}", d, name))).find(|p| ok(p))
    }

    // ---- paths ----
    fn resolve(&self, pid: u32, dirfd: u32, path: &str) -> R<String> {
        match &self.fd(pid, dirfd)?.kind {
            FdKind::Dir { path: base } => {
                let abs = normalize(base, path);
                Ok(match abs.strip_prefix("/proc/self") {
                    Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("/proc/{pid}{rest}"),
                    _ => abs,
                })
            }
            _ => Err(errno::NOTDIR),
        }
    }

    pub fn path_open(&mut self, pid: u32, dirfd: u32, path: &str, creat: bool, directory: bool, excl: bool, trunc: bool, append: bool, read: bool, write: bool) -> R<u32> {
        self.enter(pid);
        let abs = self.resolve(pid, dirfd, path)?;
        if procfs::is_proc(&abs) { self.refresh_procfs(); self.enter(pid); }
        match self.vfs.stat(&abs) {
            Ok(st) if st.kind == Kind::Dir => {
                if creat && excl {
                    return Err(errno::EXIST);
                }
                return self.open_fd(pid, Fd { kind: FdKind::Dir { path: abs }, preopen: None });
            }
            Ok(_) if directory => return Err(errno::NOTDIR),
            _ => {}
        }
        let ino = self.vfs.open_file(&abs, creat, excl, trunc && write).map_err(verr)?;
        let want = (if read { 4 } else { 0 }) | (if write { 2 } else { 0 });
        self.vfs.check_access(ino, want).map_err(verr)?;
        self.open_fd(pid, Fd { kind: FdKind::File { ino, off: 0, append, read, write }, preopen: None })
    }

    pub fn path_mkdir(&mut self, pid: u32, dirfd: u32, path: &str) -> R<()> {
        self.enter(pid);
        let abs = self.resolve(pid, dirfd, path)?;
        self.vfs.mkdir(&abs, 0o755).map_err(verr)
    }

    pub fn path_rmdir(&mut self, pid: u32, dirfd: u32, path: &str) -> R<()> {
        self.enter(pid);
        let abs = self.resolve(pid, dirfd, path)?;
        self.vfs.rmdir(&abs).map_err(verr)
    }

    pub fn path_unlink(&mut self, pid: u32, dirfd: u32, path: &str) -> R<()> {
        self.enter(pid);
        let abs = self.resolve(pid, dirfd, path)?;
        self.vfs.unlink(&abs).map_err(verr)
    }

    pub fn path_rename(&mut self, pid: u32, d1: u32, p1: &str, d2: u32, p2: &str) -> R<()> {
        self.enter(pid);
        let a = self.resolve(pid, d1, p1)?;
        let b = self.resolve(pid, d2, p2)?;
        if let (Ok(src), Ok(dst)) = (self.vfs.lstat(&a), self.vfs.lstat(&b)) {
            if src.kind != Kind::Dir && dst.kind != Kind::Dir {
                self.vfs.unlink(&b).map_err(verr)?;
            }
        }
        self.vfs.rename(&a, &b).map_err(verr)
    }

    pub fn path_symlink(&mut self, pid: u32, target: &str, dirfd: u32, path: &str) -> R<()> {
        self.enter(pid);
        let abs = self.resolve(pid, dirfd, path)?;
        self.vfs.symlink(target, &abs).map_err(verr)
    }

    pub fn path_readlink(&mut self, pid: u32, dirfd: u32, path: &str) -> R<String> {
        self.enter(pid);
        let abs = self.resolve(pid, dirfd, path)?;
        self.vfs.readlink(&abs).map_err(verr)
    }

    pub fn path_stat(&mut self, pid: u32, dirfd: u32, path: &str, follow: bool) -> R<Stat> {
        self.enter(pid);
        let abs = self.resolve(pid, dirfd, path)?;
        if procfs::is_proc(&abs) { self.refresh_procfs(); self.enter(pid); }
        (if follow { self.vfs.stat(&abs) } else { self.vfs.lstat(&abs) }).map_err(verr)
    }

    // ---- fd operations ----
    pub fn fd_close(&mut self, pid: u32, fd: u32) -> R<()> {
        let f = self.procs.get_mut(&pid).ok_or(errno::CHILD)?.fds.remove(&fd).ok_or(errno::BADF)?;
        self.pipe_ref(&f.kind, -1);
        Ok(())
    }

    pub fn fd_read(&mut self, pid: u32, fd: u32, len: usize) -> R<Vec<u8>> {
        let kind = self.fd(pid, fd)?.kind.clone();
        match kind {
            FdKind::File { ino, off, read, .. } => {
                if !read {
                    return Err(errno::BADF);
                }
                let d = self.vfs.read_at(ino, off, len).map_err(verr)?;
                if let Some(Fd { kind: FdKind::File { off, .. }, .. }) = self.procs.get_mut(&pid).and_then(|p| p.fds.get_mut(&fd)) {
                    *off += d.len() as u64;
                }
                Ok(d)
            }
            FdKind::Dir { .. } => Err(errno::ISDIR),
            FdKind::PipeR(id) => match self.pipes.get_mut(id).and_then(|p| p.as_mut()) {
                Some(p) if !p.buf.is_empty() => {
                    let n = len.min(p.buf.len());
                    Ok(p.buf.drain(..n).collect())
                }
                Some(p) if p.writers == 0 => Ok(Vec::new()),
                Some(_) => Err(errno::AGAIN),
                None => Err(errno::BADF),
            },
            FdKind::PipeW(_) => Err(errno::BADF),
            FdKind::Tty => {
                if self.tty_in.is_empty() {
                    if std::mem::take(&mut self.tty_eof) {
                        return Ok(Vec::new());
                    }
                    return Err(errno::AGAIN);
                }
                let n = len.min(self.tty_in.len());
                Ok(self.tty_in.drain(..n).collect())
            }
            FdKind::Null => Ok(Vec::new()),
        }
    }

    pub fn fd_write(&mut self, pid: u32, fd: u32, data: &[u8]) -> R<usize> {
        let kind = self.fd(pid, fd)?.kind.clone();
        match kind {
            FdKind::File { ino, off, append, write, .. } => {
                if !write {
                    return Err(errno::BADF);
                }
                let at = if append { self.vfs.stat_ino(ino).size } else { off };
                let n = self.vfs.write_at(ino, at, data).map_err(verr)?;
                if let Some(Fd { kind: FdKind::File { off, .. }, .. }) = self.procs.get_mut(&pid).and_then(|p| p.fds.get_mut(&fd)) {
                    *off = at + n as u64;
                }
                Ok(n)
            }
            FdKind::Dir { .. } => Err(errno::ISDIR),
            FdKind::PipeW(id) => match self.pipes.get_mut(id).and_then(|p| p.as_mut()) {
                Some(p) if p.readers > 0 => {
                    let room = PIPE_CAP.saturating_sub(p.buf.len());
                    if room == 0 && !data.is_empty() {
                        return Err(errno::AGAIN);
                    }
                    let n = data.len().min(room);
                    p.buf.extend(&data[..n]);
                    Ok(n)
                }
                Some(_) => Err(errno::PIPE),
                None => Err(errno::BADF),
            },
            FdKind::PipeR(_) => Err(errno::BADF),
            FdKind::Tty => {
                self.tty_out.extend_from_slice(data);
                Ok(data.len())
            }
            FdKind::Null => Ok(data.len()),
        }
    }

    pub fn fd_pread(&mut self, pid: u32, fd: u32, len: usize, off: u64) -> R<Vec<u8>> {
        match self.fd(pid, fd)?.kind {
            FdKind::File { ino, read: true, .. } => self.vfs.read_at(ino, off, len).map_err(verr),
            FdKind::File { .. } => Err(errno::BADF),
            _ => Err(errno::SPIPE),
        }
    }

    pub fn fd_pwrite(&mut self, pid: u32, fd: u32, data: &[u8], off: u64) -> R<usize> {
        match self.fd(pid, fd)?.kind {
            FdKind::File { ino, write: true, .. } => self.vfs.write_at(ino, off, data).map_err(verr),
            FdKind::File { .. } => Err(errno::BADF),
            _ => Err(errno::SPIPE),
        }
    }

    /// whence: 0 = set, 1 = cur, 2 = end.
    pub fn fd_seek(&mut self, pid: u32, fd: u32, delta: i64, whence: u8) -> R<u64> {
        let (ino, cur) = match self.fd(pid, fd)?.kind {
            FdKind::File { ino, off, .. } => (ino, off),
            FdKind::Dir { .. } => return Err(errno::ISDIR),
            _ => return Err(errno::SPIPE),
        };
        let base = match whence {
            0 => 0,
            1 => cur as i64,
            2 => self.vfs.stat_ino(ino).size as i64,
            _ => return Err(errno::INVAL),
        };
        let new = base.checked_add(delta).filter(|n| *n >= 0).ok_or(errno::INVAL)? as u64;
        if let Some(Fd { kind: FdKind::File { off, .. }, .. }) = self.procs.get_mut(&pid).and_then(|p| p.fds.get_mut(&fd)) {
            *off = new;
        }
        Ok(new)
    }

    pub fn fd_set_size(&mut self, pid: u32, fd: u32, len: u64) -> R<()> {
        match self.fd(pid, fd)?.kind {
            FdKind::File { ino, write: true, .. } => self.vfs.truncate(ino, len).map_err(verr),
            _ => Err(errno::BADF),
        }
    }

    pub fn fd_stat(&self, pid: u32, fd: u32) -> R<Stat> {
        let synth = |kind, mode| Stat { ino: 0, kind, size: 0, mode, uid: 0, gid: 0, mtime: 0 };
        match &self.fd(pid, fd)?.kind {
            FdKind::File { ino, .. } => {
                if self.vfs.live(*ino) {
                    Ok(self.vfs.stat_ino(*ino))
                } else {
                    Err(errno::NOENT)
                }
            }
            FdKind::Dir { path } => self.vfs.stat(path).map_err(verr),
            _ => Ok(synth(Kind::File, 0o600)),
        }
    }

    /// (filetype, append flag)
    pub fn fd_type(&self, pid: u32, fd: u32) -> R<(u8, bool)> {
        Ok(match &self.fd(pid, fd)?.kind {
            FdKind::File { append, .. } => (ftype::REG, *append),
            FdKind::Dir { .. } => (ftype::DIR, false),
            FdKind::Tty | FdKind::Null => (ftype::CHAR, false),
            FdKind::PipeR(_) | FdKind::PipeW(_) => (ftype::UNKNOWN, false),
        })
    }

    pub fn fd_prestat(&self, pid: u32, fd: u32) -> R<String> {
        self.fd(pid, fd)?.preopen.clone().ok_or(errno::BADF)
    }

    pub fn fd_readdir(&mut self, pid: u32, fd: u32) -> R<Vec<DirEnt>> {
        self.enter(pid);
        let path = match &self.fd(pid, fd)?.kind {
            FdKind::Dir { path } => path.clone(),
            _ => return Err(errno::NOTDIR),
        };
        if procfs::is_proc(&path) { self.refresh_procfs(); self.enter(pid); }
        let here = self.vfs.stat(&path).map_err(verr)?;
        let parent = self.vfs.stat(&normalize(&path, "..")).map(|s| s.ino as u64).unwrap_or(here.ino as u64);
        let mut out = vec![
            DirEnt { name: ".".into(), ino: here.ino as u64, ftype: ftype::DIR },
            DirEnt { name: "..".into(), ino: parent, ftype: ftype::DIR },
        ];
        for name in self.vfs.readdir(&path).map_err(verr)? {
            let st = self.vfs.lstat(&normalize(&path, &name)).map_err(verr)?;
            out.push(DirEnt { ino: st.ino as u64, ftype: kind_ftype(st.kind), name });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (Sys, u32) {
        let mut v = Vfs::new();
        v.mkdir("/home", 0o755).unwrap();
        v.mkdir("/home/u", 0o755).unwrap();
        v.mkdir("/bin", 0o755).unwrap();
        let mut s = Sys::new(v);
        let pid = s.spawn(0, "/bin/x".into(), vec!["x".into()], vec![], "/home/u".into(), [Stdio::Tty, Stdio::Tty, Stdio::Tty]).unwrap();
        (s, pid)
    }

    fn open(s: &mut Sys, pid: u32, dirfd: u32, p: &str, flags: &str) -> R<u32> {
        let has = |c| flags.contains(c);
        s.path_open(pid, dirfd, p, has('c'), has('d'), has('x'), has('t'), has('a'), has('r'), has('w'))
    }

    #[test]
    fn file_io_and_relative_paths() {
        let (mut s, pid) = setup();
        let fd = open(&mut s, pid, 4, "a.txt", "cw").unwrap();
        assert_eq!(s.fd_write(pid, fd, b"hello").unwrap(), 5);
        assert_eq!(s.fd_write(pid, fd, b" world").unwrap(), 6);
        s.fd_close(pid, fd).unwrap();
        assert_eq!(s.vfs.read_file("/home/u/a.txt").unwrap(), b"hello world");
        let fd = open(&mut s, pid, 3, "home/u/a.txt", "r").unwrap();
        assert_eq!(s.fd_read(pid, fd, 5).unwrap(), b"hello");
        assert_eq!(s.fd_seek(pid, fd, -5, 2).unwrap(), 6);
        assert_eq!(s.fd_read(pid, fd, 100).unwrap(), b"world");
        assert!(s.fd_read(pid, fd, 100).unwrap().is_empty());
        assert_eq!(s.fd_write(pid, fd, b"x"), Err(errno::BADF));
        assert_eq!(open(&mut s, pid, 4, "nope", "r"), Err(errno::NOENT));
        assert_eq!(open(&mut s, pid, 4, "a.txt", "cxw"), Err(errno::EXIST));
        assert_eq!(s.fd_pread(pid, fd, 3, 1).unwrap(), b"ell");
    }

    #[test]
    fn append_trunc_and_dirs() {
        let (mut s, pid) = setup();
        let fd = open(&mut s, pid, 4, "f", "cwa").unwrap();
        s.fd_write(pid, fd, b"ab").unwrap();
        s.fd_close(pid, fd).unwrap();
        let fd = open(&mut s, pid, 4, "f", "wa").unwrap();
        s.fd_write(pid, fd, b"cd").unwrap();
        assert_eq!(s.vfs.read_file("/home/u/f").unwrap(), b"abcd");
        open(&mut s, pid, 4, "f", "wt").unwrap();
        assert!(s.vfs.read_file("/home/u/f").unwrap().is_empty());
        s.path_mkdir(pid, 4, "d").unwrap();
        let d = open(&mut s, pid, 4, "d", "rd").unwrap();
        assert_eq!(s.fd_type(pid, d).unwrap().0, ftype::DIR);
        assert_eq!(open(&mut s, pid, 4, "f", "rd"), Err(errno::NOTDIR));
        let names: Vec<_> = s.fd_readdir(pid, 4).unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(names, [".", "..", "d", "f"]);
        s.path_rename(pid, 4, "f", 4, "d/g").unwrap();
        assert!(s.path_stat(pid, 4, "d/g", true).is_ok());
        s.path_unlink(pid, 4, "d/g").unwrap();
        s.path_rmdir(pid, 4, "d").unwrap();
    }

    #[test]
    fn pipes_between_processes() {
        let (mut s, pid) = setup();
        let (r, w) = s.create_pipe(pid).unwrap();
        let child = s.spawn(pid, "/bin/y".into(), vec!["y".into()], vec![], "/".into(), [Stdio::Null, Stdio::Fd(w), Stdio::Tty]).unwrap();
        s.fd_close(pid, w).unwrap();
        s.fd_write(child, 1, b"from child").unwrap();
        assert_eq!(s.fd_read(pid, r, 100), Ok(b"from child".to_vec()));
        assert_eq!(s.fd_read(pid, r, 100), Err(errno::AGAIN));
        s.exit(child, 7);
        assert!(s.fd_read(pid, r, 100).unwrap().is_empty());
        assert_eq!(s.wait(child), Ok(7));
        assert_eq!(s.wait(child), Err(errno::CHILD));
    }

    #[test]
    fn host_pipes_and_tty() {
        let (mut s, _pid) = setup();
        let inp = s.new_pipe();
        s.pipe_fill(inp, b"in");
        let out = s.new_pipe();
        s.pipe_host_reader(out);
        let c = s.spawn(0, "/bin/c".into(), vec![], vec![], "/".into(), [Stdio::PipeR(inp), Stdio::PipeW(out), Stdio::Tty]).unwrap();
        assert_eq!(s.fd_read(c, 0, 10).unwrap(), b"in");
        s.fd_write(c, 1, b"out").unwrap();
        s.fd_write(c, 2, b"err").unwrap();
        s.exit(c, 0);
        assert_eq!(s.pipe_drain(out), b"out");
        assert_eq!(s.tty_out, b"err");
        assert_eq!(s.fd_type(c, 1), Err(errno::BADF));
        s.wait(c).unwrap();
        s.pipe_release(inp);
        s.pipe_release(out);
        assert_eq!(s.proc_count(), 1);
    }

    #[test]
    fn which_and_prestat() {
        let (mut s, pid) = setup();
        s.vfs.write_file("/bin/tool", b"").unwrap();
        assert_eq!(s.which("tool", "/bin", "/"), None);
        s.vfs.chmod("/bin/tool", 0o755).unwrap();
        assert_eq!(s.which("tool", "/usr/bin:/bin", "/"), Some("/bin/tool".into()));
        assert_eq!(s.which("./tool", "", "/bin"), Some("/bin/tool".into()));
        assert_eq!(s.fd_prestat(pid, 3).unwrap(), "/");
        assert_eq!(s.fd_prestat(pid, 4).unwrap(), ".");
        assert_eq!(s.fd_prestat(pid, 5), Err(errno::BADF));
        assert_eq!(s.fd_type(pid, 1).unwrap().0, ftype::CHAR);
    }
}

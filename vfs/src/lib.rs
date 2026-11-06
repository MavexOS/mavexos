use std::collections::BTreeMap;

pub type Ino = u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Errno {
    NoEnt,
    Exist,
    NotDir,
    IsDir,
    NotEmpty,
    Inval,
    Loop,
    Acces,
    Perm,
}

pub fn normalize(cwd: &str, p: &str) -> String {
    let full = if p.starts_with('/') { p.to_string() } else { format!("{}/{}", cwd, p) };
    let mut parts: Vec<&str> = Vec::new();
    for c in full.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            c => parts.push(c),
        }
    }
    format!("/{}", parts.join("/"))
}

pub type Result<T> = std::result::Result<T, Errno>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
    Symlink,
}

#[derive(Debug, Clone, Copy)]
pub struct Stat {
    pub ino: Ino,
    pub kind: Kind,
    pub size: u64,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub mtime: u64,
}

enum Data {
    File(Vec<u8>),
    Dir(BTreeMap<String, Ino>),
    Symlink(String),
}

struct Node {
    data: Data,
    mode: u32,
    uid: u32,
    gid: u32,
    mtime: u64,
}

pub struct Vfs {
    nodes: Vec<Option<Node>>,
    free: Vec<Ino>,
    cred: (u32, u32),
    now: u64,
}

const ROOT: Ino = 0;
const MAX_SYMLINKS: u32 = 40;

impl Default for Vfs {
    fn default() -> Self {
        Self::new()
    }
}

impl Vfs {
    pub fn new() -> Self {
        let root = Node { data: Data::Dir(BTreeMap::new()), mode: 0o755, uid: 0, gid: 0, mtime: 0 };
        Vfs { nodes: vec![Some(root)], free: Vec::new(), cred: (0, 0), now: 0 }
    }

    fn alloc(&mut self, node: Node) -> Ino {
        if let Some(i) = self.free.pop() {
            self.nodes[i as usize] = Some(node);
            i
        } else {
            self.nodes.push(Some(node));
            (self.nodes.len() - 1) as Ino
        }
    }

    fn may(&self, n: &Node, want: u32) -> bool {
        let (uid, gid) = self.cred;
        if uid == 0 {
            return want & 1 == 0 || matches!(n.data, Data::Dir(_)) || n.mode & 0o111 != 0;
        }
        let bits = if uid == n.uid { n.mode >> 6 } else if gid == n.gid { n.mode >> 3 } else { n.mode } & 7;
        bits & want == want
    }

    /// want: bit 4 = read, 2 = write, 1 = execute/search.
    pub fn check_access(&self, ino: Ino, want: u32) -> Result<()> {
        if self.may(self.node(ino), want) { Ok(()) } else { Err(Errno::Acces) }
    }

    fn may_modify_dir(&self, parent: Ino, child: Option<Ino>) -> Result<()> {
        let pn = self.node(parent);
        if !self.may(pn, 3) {
            return Err(Errno::Acces);
        }
        if let (Some(c), true) = (child, pn.mode & 0o1000 != 0) {
            let uid = self.cred.0;
            if uid != 0 && uid != pn.uid && uid != self.node(c).uid {
                return Err(Errno::Acces);
            }
        }
        Ok(())
    }

    fn node(&self, ino: Ino) -> &Node {
        self.nodes[ino as usize].as_ref().expect("dangling inode")
    }

    fn node_mut(&mut self, ino: Ino) -> &mut Node {
        self.nodes[ino as usize].as_mut().expect("dangling inode")
    }

    fn split(path: &str) -> Result<(&str, &str)> {
        let trimmed = path.trim_end_matches('/');
        if trimmed.is_empty() {
            return Err(Errno::Inval);
        }
        match trimmed.rfind('/') {
            Some(i) => Ok((&trimmed[..i.max(0)], &trimmed[i + 1..])),
            None => Err(Errno::Inval),
        }
    }

    fn walk(&self, path: &str, follow_last: bool, depth: u32) -> Result<Ino> {
        if depth > MAX_SYMLINKS {
            return Err(Errno::Loop);
        }
        if !path.starts_with('/') {
            return Err(Errno::Inval);
        }
        let comps: Vec<&str> = path.split('/').filter(|c| !c.is_empty() && *c != ".").collect();
        let mut stack: Vec<Ino> = vec![ROOT];
        for (i, comp) in comps.iter().enumerate() {
            let last = i + 1 == comps.len();
            if *comp == ".." {
                if stack.len() > 1 {
                    stack.pop();
                }
                continue;
            }
            let cur = *stack.last().unwrap();
            if !self.may(self.node(cur), 1) {
                return Err(Errno::Acces);
            }
            let child = match &self.node(cur).data {
                Data::Dir(m) => *m.get(*comp).ok_or(Errno::NoEnt)?,
                _ => return Err(Errno::NotDir),
            };
            if let Data::Symlink(target) = &self.node(child).data {
                if !last || follow_last {
                    let rest: Vec<&str> = comps[i + 1..].to_vec();
                    let mut base = if target.starts_with('/') {
                        target.clone()
                    } else {
                        let cur_ino = *stack.last().unwrap();
                        let p = if cur_ino == ROOT { String::new() } else { format!("/{}", self.name_of(cur_ino)) };
                        format!("{}/{}", p, target)
                    };
                    for r in rest {
                        base.push('/');
                        base.push_str(r);
                    }
                    return self.walk(&base, follow_last, depth + 1);
                }
            }
            stack.push(child);
        }
        Ok(*stack.last().unwrap())
    }

    fn name_of(&self, target: Ino) -> String {
        fn find(v: &Vfs, dir: Ino, target: Ino, out: &mut Vec<String>) -> bool {
            if let Data::Dir(m) = &v.node(dir).data {
                for (name, &ino) in m {
                    if ino == target {
                        out.push(name.clone());
                        return true;
                    }
                    if matches!(v.node(ino).data, Data::Dir(_)) && find(v, ino, target, out) {
                        out.insert(0, name.clone());
                        return true;
                    }
                }
            }
            false
        }
        let mut out = Vec::new();
        find(self, ROOT, target, &mut out);
        out.join("/")
    }

    fn parent_and_name(&self, path: &str) -> Result<(Ino, String)> {
        let (dir, name) = Self::split(path)?;
        if name == "." || name == ".." {
            return Err(Errno::Inval);
        }
        let dir = if dir.is_empty() { "/" } else { dir };
        let pino = self.walk(dir, true, 0)?;
        if !matches!(self.node(pino).data, Data::Dir(_)) {
            return Err(Errno::NotDir);
        }
        Ok((pino, name.to_string()))
    }

    fn insert(&mut self, parent: Ino, name: String, node: Node) -> Result<Ino> {
        if let Data::Dir(m) = &self.node(parent).data {
            if m.contains_key(&name) {
                return Err(Errno::Exist);
            }
        }
        self.may_modify_dir(parent, None)?;
        let ino = self.alloc(node);
        if let Data::Dir(m) = &mut self.node_mut(parent).data {
            m.insert(name, ino);
        }
        Ok(ino)
    }


    /// Bytes stored in regular files and symlinks, and live inode count (excludes /proc, which is regenerated).
    pub fn usage(&self) -> (u64, u64) {
        let (mut bytes, mut inodes) = (0u64, 0u64);
        for n in self.nodes.iter().flatten() {
            inodes += 1;
            match &n.data {
                Data::File(d) => bytes += d.len() as u64,
                Data::Symlink(t) => bytes += t.len() as u64,
                _ => {}
            }
        }
        (bytes, inodes)
    }

    pub fn set_time(&mut self, ns: u64) {
        self.now = ns;
    }

    pub fn live(&self, ino: Ino) -> bool {
        self.nodes.get(ino as usize).is_some_and(|n| n.is_some())
    }

    pub fn open_file(&mut self, path: &str, create: bool, excl: bool, trunc: bool) -> Result<Ino> {
        match self.walk(path, true, 0) {
            Ok(ino) => {
                if create && excl {
                    return Err(Errno::Exist);
                }
                let now = self.now;
                if trunc {
                    self.check_access(ino, 2)?;
                }
                let n = self.node_mut(ino);
                match &mut n.data {
                    Data::File(f) => {
                        if trunc {
                            f.clear();
                            n.mtime = now;
                        }
                        Ok(ino)
                    }
                    _ => Err(Errno::IsDir),
                }
            }
            Err(Errno::NoEnt) if create => {
                let (p, name) = self.parent_and_name(path)?;
                self.insert(p, name, Node { data: Data::File(Vec::new()), mode: 0o644, uid: self.cred.0, gid: self.cred.1, mtime: self.now })
            }
            Err(e) => Err(e),
        }
    }

    pub fn read_at(&self, ino: Ino, off: u64, len: usize) -> Result<Vec<u8>> {
        if !self.live(ino) {
            return Err(Errno::NoEnt);
        }
        match &self.node(ino).data {
            Data::File(f) => {
                let start = (off as usize).min(f.len());
                let end = start.saturating_add(len).min(f.len());
                Ok(f[start..end].to_vec())
            }
            _ => Err(Errno::IsDir),
        }
    }

    pub fn write_at(&mut self, ino: Ino, off: u64, data: &[u8]) -> Result<usize> {
        if !self.live(ino) {
            return Err(Errno::NoEnt);
        }
        let now = self.now;
        let n = self.node_mut(ino);
        match &mut n.data {
            Data::File(f) => {
                let off = off as usize;
                if f.len() < off {
                    f.resize(off, 0);
                }
                let end = off + data.len();
                if f.len() < end {
                    f.resize(end, 0);
                }
                f[off..end].copy_from_slice(data);
                n.mtime = now;
                Ok(data.len())
            }
            _ => Err(Errno::IsDir),
        }
    }

    pub fn truncate(&mut self, ino: Ino, len: u64) -> Result<()> {
        if !self.live(ino) {
            return Err(Errno::NoEnt);
        }
        let now = self.now;
        let n = self.node_mut(ino);
        match &mut n.data {
            Data::File(f) => {
                f.resize(len as usize, 0);
                n.mtime = now;
                Ok(())
            }
            _ => Err(Errno::IsDir),
        }
    }

    pub fn set_creds(&mut self, uid: u32, gid: u32) {
        self.cred = (uid, gid);
    }

    pub fn chown(&mut self, path: &str, uid: u32, gid: u32) -> Result<()> {
        let ino = self.walk(path, true, 0)?;
        if self.cred.0 != 0 {
            return Err(Errno::Perm);
        }
        let n = self.node_mut(ino);
        n.uid = uid;
        n.gid = gid;
        Ok(())
    }

    pub fn chmod(&mut self, path: &str, mode: u32) -> Result<()> {
        let ino = self.walk(path, true, 0)?;
        if self.cred.0 != 0 && self.cred.0 != self.node(ino).uid {
            return Err(Errno::Perm);
        }
        self.node_mut(ino).mode = mode & 0o7777;
        Ok(())
    }

    pub fn mkdir(&mut self, path: &str, mode: u32) -> Result<()> {
        let (p, name) = self.parent_and_name(path)?;
        self.insert(p, name, Node { data: Data::Dir(BTreeMap::new()), mode, uid: self.cred.0, gid: self.cred.1, mtime: self.now })?;
        Ok(())
    }

    pub fn symlink(&mut self, target: &str, path: &str) -> Result<()> {
        let (p, name) = self.parent_and_name(path)?;
        self.insert(p, name, Node { data: Data::Symlink(target.to_string()), mode: 0o777, uid: self.cred.0, gid: self.cred.1, mtime: self.now })?;
        Ok(())
    }

    pub fn readlink(&self, path: &str) -> Result<String> {
        let ino = self.walk(path, false, 0)?;
        match &self.node(ino).data {
            Data::Symlink(t) => Ok(t.clone()),
            _ => Err(Errno::Inval),
        }
    }

    pub fn write_file(&mut self, path: &str, data: &[u8]) -> Result<()> {
        match self.walk(path, true, 0) {
            Ok(ino) => match &mut self.nodes[ino as usize].as_mut().expect("dangling inode").data {
                Data::File(f) => {
                    f.clear();
                    f.extend_from_slice(data);
                    let now = self.now;
                    self.node_mut(ino).mtime = now;
                    Ok(())
                }
                _ => Err(Errno::IsDir),
            },
            Err(Errno::NoEnt) => {
                let (p, name) = self.parent_and_name(path)?;
                self.insert(p, name, Node { data: Data::File(data.to_vec()), mode: 0o644, uid: self.cred.0, gid: self.cred.1, mtime: self.now })?;
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    pub fn read_file(&self, path: &str) -> Result<Vec<u8>> {
        let ino = self.walk(path, true, 0)?;
        match &self.node(ino).data {
            Data::File(f) => Ok(f.clone()),
            _ => Err(Errno::IsDir),
        }
    }

    pub fn readdir(&self, path: &str) -> Result<Vec<String>> {
        let ino = self.walk(path, true, 0)?;
        self.check_access(ino, 4)?;
        match &self.node(ino).data {
            Data::Dir(m) => Ok(m.keys().cloned().collect()),
            _ => Err(Errno::NotDir),
        }
    }

    pub fn stat(&self, path: &str) -> Result<Stat> {
        let ino = self.walk(path, true, 0)?;
        Ok(self.stat_ino(ino))
    }

    pub fn lstat(&self, path: &str) -> Result<Stat> {
        let ino = self.walk(path, false, 0)?;
        Ok(self.stat_ino(ino))
    }

    pub fn stat_ino(&self, ino: Ino) -> Stat {
        let n = self.node(ino);
        let (kind, size) = match &n.data {
            Data::File(f) => (Kind::File, f.len() as u64),
            Data::Dir(m) => (Kind::Dir, m.len() as u64),
            Data::Symlink(t) => (Kind::Symlink, t.len() as u64),
        };
        Stat { ino, kind, size, mode: n.mode, uid: n.uid, gid: n.gid, mtime: n.mtime }
    }

    pub fn unlink(&mut self, path: &str) -> Result<()> {
        let (p, name) = self.parent_and_name(path)?;
        let child = match &self.node(p).data {
            Data::Dir(m) => *m.get(&name).ok_or(Errno::NoEnt)?,
            _ => return Err(Errno::NotDir),
        };
        if matches!(self.node(child).data, Data::Dir(_)) {
            return Err(Errno::IsDir);
        }
        self.may_modify_dir(p, Some(child))?;
        self.detach(p, &name, child);
        Ok(())
    }

    pub fn rmdir(&mut self, path: &str) -> Result<()> {
        let (p, name) = self.parent_and_name(path)?;
        let child = match &self.node(p).data {
            Data::Dir(m) => *m.get(&name).ok_or(Errno::NoEnt)?,
            _ => return Err(Errno::NotDir),
        };
        match &self.node(child).data {
            Data::Dir(m) if m.is_empty() => {}
            Data::Dir(_) => return Err(Errno::NotEmpty),
            _ => return Err(Errno::NotDir),
        }
        self.may_modify_dir(p, Some(child))?;
        self.detach(p, &name, child);
        Ok(())
    }

    fn detach(&mut self, parent: Ino, name: &str, child: Ino) {
        if let Data::Dir(m) = &mut self.node_mut(parent).data {
            m.remove(name);
        }
        self.nodes[child as usize] = None;
        self.free.push(child);
    }

    pub fn rename(&mut self, from: &str, to: &str) -> Result<()> {
        let (fp, fname) = self.parent_and_name(from)?;
        let child = match &self.node(fp).data {
            Data::Dir(m) => *m.get(&fname).ok_or(Errno::NoEnt)?,
            _ => return Err(Errno::NotDir),
        };
        let (tp, tname) = self.parent_and_name(to)?;
        self.may_modify_dir(fp, Some(child))?;
        self.may_modify_dir(tp, None)?;
        if let Data::Dir(m) = &self.node(tp).data {
            if m.contains_key(&tname) {
                return Err(Errno::Exist);
            }
        }
        if matches!(self.node(child).data, Data::Dir(_)) {
            let mut cur = tp;
            loop {
                if cur == child {
                    return Err(Errno::Inval);
                }
                if cur == ROOT {
                    break;
                }
                cur = self.parent_of(cur);
            }
        }
        if let Data::Dir(m) = &mut self.node_mut(fp).data {
            m.remove(&fname);
        }
        if let Data::Dir(m) = &mut self.node_mut(tp).data {
            m.insert(tname, child);
        }
        Ok(())
    }

    fn parent_of(&self, target: Ino) -> Ino {
        for (i, n) in self.nodes.iter().enumerate() {
            if let Some(Node { data: Data::Dir(m), .. }) = n {
                if m.values().any(|&c| c == target) {
                    return i as Ino;
                }
            }
        }
        ROOT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_files_and_dirs() {
        let mut v = Vfs::new();
        v.mkdir("/home", 0o755).unwrap();
        v.mkdir("/home/user", 0o755).unwrap();
        v.write_file("/home/user/a.txt", b"hello").unwrap();
        assert_eq!(v.read_file("/home/user/a.txt").unwrap(), b"hello");
        assert_eq!(v.readdir("/home").unwrap(), vec!["user"]);
        assert_eq!(v.stat("/home/user/a.txt").unwrap().size, 5);
        assert_eq!(v.mkdir("/home", 0o755), Err(Errno::Exist));
        assert_eq!(v.read_file("/nope"), Err(Errno::NoEnt));
        assert_eq!(v.read_file("/home"), Err(Errno::IsDir));
    }

    #[test]
    fn overwrite_and_unlink() {
        let mut v = Vfs::new();
        v.write_file("/f", b"one").unwrap();
        v.write_file("/f", b"two!").unwrap();
        assert_eq!(v.read_file("/f").unwrap(), b"two!");
        v.unlink("/f").unwrap();
        assert_eq!(v.read_file("/f"), Err(Errno::NoEnt));
        v.write_file("/g", b"x").unwrap();
        assert_eq!(v.stat("/g").unwrap().size, 1);
    }

    #[test]
    fn rmdir_rules() {
        let mut v = Vfs::new();
        v.mkdir("/d", 0o755).unwrap();
        v.write_file("/d/f", b"x").unwrap();
        assert_eq!(v.rmdir("/d"), Err(Errno::NotEmpty));
        assert_eq!(v.unlink("/d"), Err(Errno::IsDir));
        v.unlink("/d/f").unwrap();
        v.rmdir("/d").unwrap();
        assert_eq!(v.stat("/d").map(|_| ()), Err(Errno::NoEnt));
    }

    #[test]
    fn dotdot_and_relative_rejected() {
        let mut v = Vfs::new();
        v.mkdir("/a", 0o755).unwrap();
        v.mkdir("/a/b", 0o755).unwrap();
        assert!(v.stat("/a/b/../b/.").is_ok());
        assert!(v.stat("/../../a").is_ok());
        assert_eq!(v.stat("a").map(|_| ()), Err(Errno::Inval));
    }

    #[test]
    fn creds_chown_chmod() {
        let mut v = Vfs::new();
        v.chmod("/", 0o777).unwrap();
        v.set_creds(1000, 1000);
        v.write_file("/f", b"x").unwrap();
        let st = v.stat("/f").unwrap();
        assert_eq!((st.uid, st.gid), (1000, 1000));
        assert_eq!(v.chown("/f", 0, 5), Err(Errno::Perm));
        v.set_creds(0, 0);
        v.chown("/f", 0, 5).unwrap();
        v.chmod("/f", 0o100755).unwrap();
        let st = v.stat("/f").unwrap();
        assert_eq!((st.uid, st.gid, st.mode), (0, 5, 0o755));
    }

    #[test]
    fn permissions_enforced() {
        let mut v = Vfs::new();
        v.mkdir("/priv", 0o700).unwrap();
        v.write_file("/priv/s", b"secret").unwrap();
        v.mkdir("/tmp", 0o1777).unwrap();
        v.set_creds(1000, 1000);
        assert_eq!(v.read_file("/priv/s"), Err(Errno::Acces));
        assert_eq!(v.readdir("/priv"), Err(Errno::Acces));
        assert_eq!(v.write_file("/x", b"1"), Err(Errno::Acces));
        v.write_file("/tmp/mine", b"1").unwrap();
        v.set_creds(1001, 1001);
        assert_eq!(v.unlink("/tmp/mine"), Err(Errno::Acces));
        assert_eq!(v.chmod("/tmp/mine", 0o777), Err(Errno::Perm));
        v.set_creds(1000, 1000);
        v.unlink("/tmp/mine").unwrap();
        v.set_creds(0, 0);
        assert_eq!(v.read_file("/priv/s").unwrap(), b"secret");
    }

    #[test]
    fn symlinks() {
        let mut v = Vfs::new();
        v.mkdir("/real", 0o755).unwrap();
        v.write_file("/real/f", b"data").unwrap();
        v.symlink("/real", "/link").unwrap();
        assert_eq!(v.read_file("/link/f").unwrap(), b"data");
        assert_eq!(v.lstat("/link").unwrap().kind, Kind::Symlink);
        assert_eq!(v.stat("/link").unwrap().kind, Kind::Dir);
        assert_eq!(v.readlink("/link").unwrap(), "/real");
        v.symlink("real/f", "/rel").unwrap();
        assert_eq!(v.read_file("/rel").unwrap(), b"data");
        v.symlink("/loop2", "/loop1").unwrap();
        v.symlink("/loop1", "/loop2").unwrap();
        assert_eq!(v.read_file("/loop1"), Err(Errno::Loop));
    }

    #[test]
    fn rename_moves_and_blocks_cycles() {
        let mut v = Vfs::new();
        v.mkdir("/a", 0o755).unwrap();
        v.mkdir("/a/b", 0o755).unwrap();
        v.write_file("/a/f", b"x").unwrap();
        v.rename("/a/f", "/g").unwrap();
        assert_eq!(v.read_file("/g").unwrap(), b"x");
        assert_eq!(v.rename("/a", "/a/b/c"), Err(Errno::Inval));
        v.rename("/a/b", "/b").unwrap();
        assert!(v.stat("/b").is_ok());
    }
}

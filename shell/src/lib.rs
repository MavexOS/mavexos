use mavex_vfs::{Errno, Kind, Vfs};
use std::collections::BTreeMap;

pub const COMMANDS: &[&str] = &[
    "cat", "cd", "clear", "cp", "echo", "env", "exit", "export", "false", "grep", "head", "help",
    "hostname", "id", "ln", "ls", "mkdir", "mv", "pwd", "readlink", "rm", "rmdir", "sort", "stat",
    "tail", "touch", "tree", "true", "type", "uname", "unset", "wc", "which", "whoami",
];

pub const SHELL_ONLY: &[&str] = &["cd", "pwd", "export", "unset", "exit", "help", "clear", "which", "type"];

pub struct ExecReq<'a> {
    pub argv: &'a [String],
    pub env: &'a BTreeMap<String, String>,
    pub cwd: &'a str,
    /// None means stdin is the terminal; Some is piped input.
    pub stdin: Option<&'a str>,
    /// True when stdout is the terminal (stdout and stderr come back merged in `out`).
    pub tty: bool,
}

pub struct ExecRes {
    pub out: String,
    pub err: String,
    pub code: i32,
}

pub trait Host {
    fn with_vfs(&self, f: &mut dyn FnMut(&mut Vfs));
    fn find_exe(&self, name: &str, path: &str, cwd: &str) -> Option<String>;
    fn exec(&self, req: &ExecReq) -> Option<ExecRes>;
}

pub struct Shell {
    pub cwd: String,
    pub env: BTreeMap<String, String>,
    pub last: i32,
    pub exited: bool,
    pub cols: u32,
}

#[derive(Debug, Clone, PartialEq)]
struct Word {
    text: String,
    glob: bool,
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Word(Word),
    Op(&'static str),
}

struct Cmd {
    words: Vec<Word>,
    redir: Option<(Word, bool)>,
}

struct Res {
    out: String,
    err: String,
    code: i32,
}

impl Res {
    fn ok(out: String) -> Self {
        Res { out, err: String::new(), code: 0 }
    }
    fn fail(err: String) -> Self {
        Res { out: String::new(), err, code: 1 }
    }
}

fn errstr(e: Errno) -> &'static str {
    match e {
        Errno::NoEnt => "No such file or directory",
        Errno::Exist => "File exists",
        Errno::NotDir => "Not a directory",
        Errno::IsDir => "Is a directory",
        Errno::NotEmpty => "Directory not empty",
        Errno::Inval => "Invalid argument",
        Errno::Loop => "Too many levels of symbolic links",
        Errno::Acces => "Permission denied",
        Errno::Perm => "Operation not permitted",
    }
}

pub use mavex_vfs::normalize;

fn basename(p: &str) -> &str {
    p.trim_end_matches('/').rsplit('/').next().unwrap_or(p)
}

fn glob_match(p: &[char], t: &[char]) -> bool {
    match p.first() {
        None => t.is_empty(),
        Some(&'*') => (0..=t.len()).any(|k| glob_match(&p[1..], &t[k..])),
        Some(&'?') => !t.is_empty() && glob_match(&p[1..], &t[1..]),
        Some(c) => t.first() == Some(c) && glob_match(&p[1..], &t[1..]),
    }
}

fn split_flags(args: &[String]) -> (String, Vec<String>) {
    let mut flags = String::new();
    let mut rest = Vec::new();
    let mut done = false;
    for a in args {
        if !done && a == "--" {
            done = true;
        } else if !done && a.starts_with('-') && a.len() > 1 {
            flags.push_str(&a[1..]);
        } else {
            rest.push(a.clone());
        }
    }
    (flags, rest)
}

fn mode_string(kind: Kind, mode: u32) -> String {
    let mut s = String::new();
    s.push(match kind {
        Kind::Dir => 'd',
        Kind::Symlink => 'l',
        Kind::File => '-',
    });
    for shift in [6, 3, 0] {
        let b = (mode >> shift) & 7;
        s.push(if b & 4 != 0 { 'r' } else { '-' });
        s.push(if b & 2 != 0 { 'w' } else { '-' });
        s.push(if b & 1 != 0 { 'x' } else { '-' });
    }
    s
}

fn id_name(id: u32) -> String {
    match id {
        0 => "root".into(),
        1000 => "user".into(),
        n => n.to_string(),
    }
}

impl Default for Shell {
    fn default() -> Self {
        Self::new()
    }
}

impl Shell {
    pub fn new() -> Self {
        let mut env = BTreeMap::new();
        for (k, v) in [
            ("HOME", "/home/user"),
            ("USER", "user"),
            ("HOSTNAME", "mavex"),
            ("PATH", "/bin"),
            ("SHELL", "/bin/msh"),
            ("LANG", "en_US.UTF-8"),
        ] {
            env.insert(k.to_string(), v.to_string());
        }
        Shell { cwd: "/home/user".into(), env, last: 0, exited: false, cols: 80 }
    }

    fn p(&self, s: &str) -> String {
        normalize(&self.cwd, s)
    }

    pub fn prompt(&self) -> String {
        let home = self.env.get("HOME").cloned().unwrap_or_default();
        let dir = if self.cwd == home {
            "~".to_string()
        } else if let Some(rest) = self.cwd.strip_prefix(&format!("{}/", home)) {
            format!("~/{}", rest)
        } else {
            self.cwd.clone()
        };
        format!(
            "\x1b[1;32m{}@{}\x1b[0m:\x1b[1;34m{}\x1b[0m$ ",
            self.env.get("USER").map(String::as_str).unwrap_or("user"),
            self.env.get("HOSTNAME").map(String::as_str).unwrap_or("mavex"),
            dir
        )
    }

    fn expand(&self, c: &[char], i: usize, cur: &mut String) -> usize {
        let mut j = i + 1;
        if j < c.len() && c[j] == '?' {
            cur.push('\u{1}');
            return j + 1;
        }
        if j < c.len() && c[j] == '{' {
            if let Some(end) = c[j + 1..].iter().position(|&x| x == '}') {
                let name: String = c[j + 1..j + 1 + end].iter().collect();
                if let Some(v) = self.env.get(&name) {
                    cur.push_str(v);
                }
                return j + 1 + end + 1;
            }
            cur.push('$');
            return i + 1;
        }
        let start = j;
        while j < c.len() && (c[j].is_ascii_alphanumeric() || c[j] == '_') && !(j == start && c[j].is_ascii_digit()) {
            j += 1;
        }
        if j == start {
            cur.push('$');
            return i + 1;
        }
        let name: String = c[start..j].iter().collect();
        if let Some(v) = self.env.get(&name) {
            cur.push_str(v);
        }
        j
    }

    fn tokenize(&self, line: &str) -> Result<Vec<Tok>, String> {
        let c: Vec<char> = line.chars().collect();
        let mut toks = Vec::new();
        let mut cur = String::new();
        let mut in_word = false;
        let mut glob = false;
        let mut i = 0;
        macro_rules! flush {
            () => {
                if in_word {
                    toks.push(Tok::Word(Word { text: std::mem::take(&mut cur), glob }));
                    in_word = false;
                    glob = false;
                }
            };
        }
        while i < c.len() {
            let ch = c[i];
            match ch {
                ' ' | '\t' | '\n' => {
                    flush!();
                    i += 1;
                }
                '\'' => {
                    in_word = true;
                    i += 1;
                    loop {
                        if i >= c.len() {
                            return Err("unterminated quote".into());
                        }
                        if c[i] == '\'' {
                            i += 1;
                            break;
                        }
                        cur.push(c[i]);
                        i += 1;
                    }
                }
                '"' => {
                    in_word = true;
                    i += 1;
                    loop {
                        if i >= c.len() {
                            return Err("unterminated quote".into());
                        }
                        match c[i] {
                            '"' => {
                                i += 1;
                                break;
                            }
                            '\\' if i + 1 < c.len() && matches!(c[i + 1], '"' | '\\' | '$') => {
                                cur.push(c[i + 1]);
                                i += 2;
                            }
                            '$' => i = self.expand(&c, i, &mut cur),
                            other => {
                                cur.push(other);
                                i += 1;
                            }
                        }
                    }
                }
                '\\' => {
                    in_word = true;
                    if i + 1 < c.len() {
                        cur.push(c[i + 1]);
                        i += 2;
                    } else {
                        cur.push('\\');
                        i += 1;
                    }
                }
                '$' => {
                    in_word = true;
                    i = self.expand(&c, i, &mut cur);
                }
                '~' if !in_word && (i + 1 >= c.len() || c[i + 1] == '/' || c[i + 1].is_whitespace()) => {
                    in_word = true;
                    cur.push_str(self.env.get("HOME").map(String::as_str).unwrap_or("/"));
                    i += 1;
                }
                '*' | '?' => {
                    in_word = true;
                    glob = true;
                    cur.push(ch);
                    i += 1;
                }
                '#' if !in_word => break,
                '|' => {
                    flush!();
                    if i + 1 < c.len() && c[i + 1] == '|' {
                        toks.push(Tok::Op("||"));
                        i += 2;
                    } else {
                        toks.push(Tok::Op("|"));
                        i += 1;
                    }
                }
                '&' => {
                    flush!();
                    if i + 1 < c.len() && c[i + 1] == '&' {
                        toks.push(Tok::Op("&&"));
                        i += 2;
                    } else {
                        return Err("background jobs are not supported".into());
                    }
                }
                ';' => {
                    flush!();
                    toks.push(Tok::Op(";"));
                    i += 1;
                }
                '>' => {
                    flush!();
                    if i + 1 < c.len() && c[i + 1] == '>' {
                        toks.push(Tok::Op(">>"));
                        i += 2;
                    } else {
                        toks.push(Tok::Op(">"));
                        i += 1;
                    }
                }
                '<' => return Err("input redirection is not supported yet".into()),
                _ => {
                    in_word = true;
                    cur.push(ch);
                    i += 1;
                }
            }
        }
        flush!();
        Ok(toks)
    }

    fn parse(toks: Vec<Tok>) -> Result<Vec<(&'static str, Vec<Cmd>)>, String> {
        let mut out = Vec::new();
        let mut conn: &'static str = ";";
        let mut pipe: Vec<Cmd> = Vec::new();
        let mut cmd = Cmd { words: vec![], redir: None };
        let mut it = toks.into_iter();
        let syntax = |near: &str| format!("syntax error near '{}'", near);
        while let Some(t) = it.next() {
            match t {
                Tok::Word(w) => cmd.words.push(w),
                Tok::Op(op @ (">" | ">>")) => match it.next() {
                    Some(Tok::Word(w)) => cmd.redir = Some((w, op == ">>")),
                    _ => return Err(syntax(op)),
                },
                Tok::Op("|") => {
                    if cmd.words.is_empty() {
                        return Err(syntax("|"));
                    }
                    pipe.push(std::mem::replace(&mut cmd, Cmd { words: vec![], redir: None }));
                }
                Tok::Op(op) => {
                    if cmd.words.is_empty() {
                        if op == ";" && pipe.is_empty() {
                            continue;
                        }
                        return Err(syntax(op));
                    }
                    pipe.push(std::mem::replace(&mut cmd, Cmd { words: vec![], redir: None }));
                    out.push((conn, std::mem::take(&mut pipe)));
                    conn = op;
                }
            }
        }
        if !cmd.words.is_empty() {
            pipe.push(cmd);
        } else if !pipe.is_empty() {
            return Err(syntax("|"));
        }
        if !pipe.is_empty() {
            out.push((conn, pipe));
        }
        Ok(out)
    }

    pub fn run_line(&mut self, host: &dyn Host, line: &str) -> String {
        let toks = match self.tokenize(line) {
            Ok(t) => t,
            Err(e) => {
                self.last = 2;
                return format!("msh: {}\n", e);
            }
        };
        let list = match Self::parse(toks) {
            Ok(l) => l,
            Err(e) => {
                self.last = 2;
                return format!("msh: {}\n", e);
            }
        };
        let mut display = String::new();
        for (conn, pipe) in list {
            if (conn == "&&" && self.last != 0) || (conn == "||" && self.last == 0) {
                continue;
            }
            self.run_pipeline(host, pipe, &mut display);
            if self.exited {
                break;
            }
        }
        display
    }

    fn expand_globs(&self, vfs: &Vfs, words: &[Word]) -> Vec<String> {
        let mut out = Vec::new();
        for w in words {
            if !w.glob {
                out.push(w.text.clone());
                continue;
            }
            let (dir_part, pat) = match w.text.rfind('/') {
                Some(i) => (&w.text[..=i], &w.text[i + 1..]),
                None => ("", &w.text[..]),
            };
            if dir_part.contains('*') || dir_part.contains('?') {
                out.push(w.text.clone());
                continue;
            }
            let dir_abs = self.p(if dir_part.is_empty() { "." } else { dir_part });
            let pchars: Vec<char> = pat.chars().collect();
            let matches: Vec<String> = match vfs.readdir(&dir_abs) {
                Ok(names) => names
                    .into_iter()
                    .filter(|n| {
                        (pat.starts_with('.') || !n.starts_with('.'))
                            && glob_match(&pchars, &n.chars().collect::<Vec<_>>())
                    })
                    .collect(),
                Err(_) => vec![],
            };
            if matches.is_empty() {
                out.push(w.text.clone());
            } else {
                for n in matches {
                    out.push(format!("{}{}", dir_part, n));
                }
            }
        }
        out
    }

    fn run_pipeline(&mut self, host: &dyn Host, pipe: Vec<Cmd>, display: &mut String) {
        let n = pipe.len();
        let mut stdin: Option<String> = None;
        for (idx, cmd) in pipe.into_iter().enumerate() {
            let to_tty = idx + 1 == n && cmd.redir.is_none();
            let mut globbed = Vec::new();
            host.with_vfs(&mut |vfs| globbed = self.expand_globs(vfs, &cmd.words));
            let last = self.last.to_string();
            let args: Vec<String> = globbed.into_iter().map(|a| a.replace('\u{1}', &last)).collect();
            let name = args[0].clone();
            let input = stdin.take();
            let path = self.env.get("PATH").cloned().unwrap_or_default();
            let external = !SHELL_ONLY.contains(&name.as_str()) && host.find_exe(&name, &path, &self.cwd).is_some();
            let res = if external {
                let mut env = self.env.clone();
                env.insert("COLUMNS".into(), self.cols.to_string());
                env.insert("PWD".into(), self.cwd.clone());
                let req = ExecReq { argv: &args, env: &env, cwd: &self.cwd, stdin: input.as_deref(), tty: to_tty };
                match host.exec(&req) {
                    Some(r) => Res { out: r.out, err: r.err, code: r.code },
                    None => Res { out: String::new(), err: format!("msh: {}: cannot execute\n", name), code: 126 },
                }
            } else {
                let text = input.unwrap_or_default();
                let mut r = None;
                host.with_vfs(&mut |vfs| r = Some(self.run_cmd(vfs, &name, &args[1..], &text)));
                r.unwrap()
            };
            display.push_str(&res.err);
            self.last = res.code;
            let mut out = res.out;
            if let Some((w, append)) = cmd.redir {
                let path = self.p(&w.text);
                let mut failure = None;
                host.with_vfs(&mut |vfs| {
                    let mut data = String::new();
                    if append {
                        if let Ok(old) = vfs.read_file(&path) {
                            data = String::from_utf8_lossy(&old).into_owned();
                        }
                    }
                    data.push_str(&out);
                    failure = vfs.write_file(&path, data.as_bytes()).err();
                });
                if let Some(e) = failure {
                    display.push_str(&format!("msh: {}: {}\n", w.text, errstr(e)));
                    self.last = 1;
                }
                out = String::new();
            }
            if idx + 1 == n {
                display.push_str(&out);
            } else {
                stdin = Some(out);
            }
            if self.exited {
                break;
            }
        }
    }

    fn read_inputs(&self, vfs: &Vfs, files: &[String], stdin: &str, who: &str) -> (String, String, i32) {
        if files.is_empty() {
            return (stdin.to_string(), String::new(), 0);
        }
        let mut text = String::new();
        let mut err = String::new();
        let mut code = 0;
        for f in files {
            if f == "-" {
                text.push_str(stdin);
                continue;
            }
            match vfs.read_file(&self.p(f)) {
                Ok(d) => text.push_str(&String::from_utf8_lossy(&d)),
                Err(e) => {
                    err.push_str(&format!("{}: {}: {}\n", who, f, errstr(e)));
                    code = 1;
                }
            }
        }
        (text, err, code)
    }

    fn run_cmd(&mut self, vfs: &mut Vfs, name: &str, args: &[String], stdin: &str) -> Res {
        match name {
            "echo" => {
                let (nl, rest) = if args.first().map(String::as_str) == Some("-n") {
                    (false, &args[1..])
                } else {
                    (true, args)
                };
                Res::ok(format!("{}{}", rest.join(" "), if nl { "\n" } else { "" }))
            }
            "pwd" => Res::ok(format!("{}\n", self.cwd)),
            "cd" => {
                let target = match args.first() {
                    Some(a) => a.clone(),
                    None => self.env.get("HOME").cloned().unwrap_or_else(|| "/".into()),
                };
                let abs = self.p(&target);
                match vfs.stat(&abs) {
                    Ok(s) if s.kind == Kind::Dir => {
                        self.cwd = abs;
                        Res::ok(String::new())
                    }
                    Ok(_) => Res::fail(format!("cd: {}: Not a directory\n", target)),
                    Err(e) => Res::fail(format!("cd: {}: {}\n", target, errstr(e))),
                }
            }
            "ls" => self.cmd_ls(vfs, args),
            "cat" => {
                let (text, err, code) = self.read_inputs(vfs, args, stdin, "cat");
                Res { out: text, err, code }
            }
            "mkdir" => {
                let (flags, paths) = split_flags(args);
                if paths.is_empty() {
                    return Res::fail("mkdir: missing operand\n".into());
                }
                let mut err = String::new();
                for pth in &paths {
                    let abs = self.p(pth);
                    if flags.contains('p') {
                        let mut acc = String::new();
                        for comp in abs.split('/').filter(|c| !c.is_empty()) {
                            acc.push('/');
                            acc.push_str(comp);
                            match vfs.mkdir(&acc, 0o755) {
                                Ok(()) | Err(Errno::Exist) => {}
                                Err(e) => {
                                    err.push_str(&format!("mkdir: cannot create directory '{}': {}\n", pth, errstr(e)));
                                    break;
                                }
                            }
                        }
                    } else if let Err(e) = vfs.mkdir(&abs, 0o755) {
                        err.push_str(&format!("mkdir: cannot create directory '{}': {}\n", pth, errstr(e)));
                    }
                }
                Res { out: String::new(), code: (!err.is_empty()) as i32, err }
            }
            "rmdir" => {
                if args.is_empty() {
                    return Res::fail("rmdir: missing operand\n".into());
                }
                let mut err = String::new();
                for pth in args {
                    if let Err(e) = vfs.rmdir(&self.p(pth)) {
                        err.push_str(&format!("rmdir: failed to remove '{}': {}\n", pth, errstr(e)));
                    }
                }
                Res { out: String::new(), code: (!err.is_empty()) as i32, err }
            }
            "rm" => {
                let (flags, paths) = split_flags(args);
                let force = flags.contains('f');
                let rec = flags.contains('r') || flags.contains('R');
                if paths.is_empty() && !force {
                    return Res::fail("rm: missing operand\n".into());
                }
                let mut err = String::new();
                for pth in &paths {
                    let abs = self.p(pth);
                    match vfs.lstat(&abs) {
                        Err(Errno::NoEnt) if force => {}
                        Err(e) => err.push_str(&format!("rm: cannot remove '{}': {}\n", pth, errstr(e))),
                        Ok(st) if st.kind == Kind::Dir && !rec => {
                            err.push_str(&format!("rm: cannot remove '{}': Is a directory\n", pth))
                        }
                        Ok(_) => {
                            if let Err(e) = rm_tree(vfs, &abs) {
                                err.push_str(&format!("rm: cannot remove '{}': {}\n", pth, errstr(e)));
                            }
                        }
                    }
                }
                Res { out: String::new(), code: (!err.is_empty()) as i32, err }
            }
            "touch" => {
                if args.is_empty() {
                    return Res::fail("touch: missing file operand\n".into());
                }
                let mut err = String::new();
                for pth in args {
                    let abs = self.p(pth);
                    if vfs.stat(&abs).is_err() {
                        if let Err(e) = vfs.write_file(&abs, b"") {
                            err.push_str(&format!("touch: cannot touch '{}': {}\n", pth, errstr(e)));
                        }
                    }
                }
                Res { out: String::new(), code: (!err.is_empty()) as i32, err }
            }
            "mv" => {
                if args.len() != 2 {
                    return Res::fail("mv: usage: mv SOURCE DEST\n".into());
                }
                let src = self.p(&args[0]);
                let mut dst = self.p(&args[1]);
                if let Ok(s) = vfs.stat(&dst) {
                    if s.kind == Kind::Dir {
                        dst = format!("{}/{}", dst.trim_end_matches('/'), basename(&src));
                    } else if vfs.lstat(&src).map(|x| x.kind != Kind::Dir).unwrap_or(false) {
                        let _ = vfs.unlink(&dst);
                    }
                }
                match vfs.rename(&src, &dst) {
                    Ok(()) => Res::ok(String::new()),
                    Err(e) => Res::fail(format!("mv: cannot move '{}': {}\n", args[0], errstr(e))),
                }
            }
            "cp" => {
                let (flags, paths) = split_flags(args);
                if paths.len() != 2 {
                    return Res::fail("cp: usage: cp [-r] SOURCE DEST\n".into());
                }
                let src = self.p(&paths[0]);
                let mut dst = self.p(&paths[1]);
                if vfs.stat(&dst).map(|s| s.kind == Kind::Dir).unwrap_or(false) {
                    dst = format!("{}/{}", dst.trim_end_matches('/'), basename(&src));
                }
                let rec = flags.contains('r') || flags.contains('R');
                match cp_tree(vfs, &src, &dst, rec) {
                    Ok(()) => Res::ok(String::new()),
                    Err(m) => Res::fail(format!("cp: {}: {}\n", paths[0], m)),
                }
            }
            "ln" => {
                let (flags, paths) = split_flags(args);
                if !flags.contains('s') {
                    return Res::fail("ln: only symbolic links are supported (use -s)\n".into());
                }
                if paths.len() != 2 {
                    return Res::fail("ln: usage: ln -s TARGET LINK\n".into());
                }
                let mut link = self.p(&paths[1]);
                if vfs.stat(&link).map(|s| s.kind == Kind::Dir).unwrap_or(false) {
                    link = format!("{}/{}", link.trim_end_matches('/'), basename(&paths[0]));
                }
                match vfs.symlink(&paths[0], &link) {
                    Ok(()) => Res::ok(String::new()),
                    Err(e) => Res::fail(format!("ln: failed to create symbolic link '{}': {}\n", paths[1], errstr(e))),
                }
            }
            "readlink" => match args.first() {
                None => Res::fail("readlink: missing operand\n".into()),
                Some(a) => match vfs.readlink(&self.p(a)) {
                    Ok(t) => Res::ok(format!("{}\n", t)),
                    Err(_) => Res { out: String::new(), err: String::new(), code: 1 },
                },
            },
            "stat" => {
                if args.is_empty() {
                    return Res::fail("stat: missing operand\n".into());
                }
                let mut out = String::new();
                let mut err = String::new();
                for a in args {
                    match vfs.lstat(&self.p(a)) {
                        Ok(s) => {
                            let kind = match s.kind {
                                Kind::File => "regular file",
                                Kind::Dir => "directory",
                                Kind::Symlink => "symbolic link",
                            };
                            out.push_str(&format!(
                                "  File: {}\n  Size: {}\tType: {}\nInode: {}\tMode: ({:04o}/{})\tUid: ({}/{})\tGid: ({}/{})\n",
                                a, s.size, kind, s.ino, s.mode & 0o7777, mode_string(s.kind, s.mode),
                                s.uid, id_name(s.uid), s.gid, id_name(s.gid)
                            ));
                        }
                        Err(e) => err.push_str(&format!("stat: cannot stat '{}': {}\n", a, errstr(e))),
                    }
                }
                Res { out, code: (!err.is_empty()) as i32, err }
            }
            "uname" => {
                let flags = args.join("");
                let v = env!("CARGO_PKG_VERSION");
                let s = if flags.contains('a') {
                    format!("MavexOS mavex {} wasm32\n", v)
                } else if flags.contains('r') {
                    format!("{}\n", v)
                } else if flags.contains('m') {
                    "wasm32\n".into()
                } else if flags.contains('n') {
                    "mavex\n".into()
                } else {
                    "MavexOS\n".into()
                };
                Res::ok(s)
            }
            "hostname" => Res::ok(format!("{}\n", self.env.get("HOSTNAME").cloned().unwrap_or_default())),
            "whoami" => Res::ok(format!("{}\n", self.env.get("USER").cloned().unwrap_or_default())),
            "id" => Res::ok("uid=1000(user) gid=1000(user) groups=1000(user)\n".into()),
            "clear" => Res::ok("\x1b[2J\x1b[H".into()),
            "env" => Res::ok(self.env.iter().map(|(k, v)| format!("{}={}\n", k, v)).collect()),
            "export" => {
                if args.is_empty() {
                    return Res::ok(self.env.iter().map(|(k, v)| format!("declare -x {}=\"{}\"\n", k, v)).collect());
                }
                for a in args {
                    if let Some((k, v)) = a.split_once('=') {
                        self.env.insert(k.to_string(), v.to_string());
                    }
                }
                Res::ok(String::new())
            }
            "unset" => {
                for a in args {
                    self.env.remove(a);
                }
                Res::ok(String::new())
            }
            "true" => Res::ok(String::new()),
            "false" => Res { out: String::new(), err: String::new(), code: 1 },
            "exit" => {
                self.exited = true;
                let code = args.first().and_then(|a| a.parse().ok()).unwrap_or(self.last);
                Res { out: "logout\n".into(), err: String::new(), code }
            }
            "wc" => self.cmd_wc(vfs, args, stdin),
            "head" | "tail" => self.cmd_headtail(vfs, name, args, stdin),
            "grep" => self.cmd_grep(vfs, args, stdin),
            "sort" => {
                let (flags, files) = split_flags(args);
                let (text, err, code) = self.read_inputs(vfs, &files, stdin, "sort");
                let mut lines: Vec<&str> = text.lines().collect();
                lines.sort();
                if flags.contains('r') {
                    lines.reverse();
                }
                Res { out: lines.iter().map(|l| format!("{}\n", l)).collect(), err, code }
            }
            "tree" => {
                let target = args.first().cloned().unwrap_or_else(|| ".".into());
                let abs = self.p(&target);
                if let Err(e) = vfs.stat(&abs) {
                    return Res::fail(format!("tree: {}: {}\n", target, errstr(e)));
                }
                let mut out = format!("{}\n", target);
                let (mut d, mut f) = (0, 0);
                tree_walk(vfs, &abs, "", &mut out, &mut d, &mut f);
                out.push_str(&format!("\n{} directories, {} files\n", d, f));
                Res::ok(out)
            }
            "type" | "which" => {
                let mut out = String::new();
                let mut err = String::new();
                for a in args {
                    if COMMANDS.contains(&a.as_str()) {
                        out.push_str(&format!("{} is a shell builtin\n", a));
                    } else {
                        err.push_str(&format!("{}: not found\n", a));
                    }
                }
                Res { out, code: (!err.is_empty()) as i32, err }
            }
            "help" => Res::ok(format!(
                "MavexOS msh {} - built-in commands:\n  {}\n\nFeatures: quotes, $VAR, ~, * ? globs, | pipes, > >> redirection, ; && ||\nKeys: Up/Down history, Tab completion, Ctrl+C/L/U/A/E/K/W\n",
                env!("CARGO_PKG_VERSION"),
                COMMANDS.join(" ")
            )),
            other => Res { out: String::new(), err: format!("msh: {}: command not found\n", other), code: 127 },
        }
    }

    fn cmd_ls(&self, vfs: &Vfs, args: &[String]) -> Res {
        let (flags, mut paths) = split_flags(args);
        let all = flags.contains('a');
        let long = flags.contains('l');
        let one = flags.contains('1') || long;
        if paths.is_empty() {
            paths.push(".".into());
        }
        let multi = paths.len() > 1;
        let mut files: Vec<String> = Vec::new();
        let mut dirs: Vec<String> = Vec::new();
        let mut err = String::new();
        for pth in &paths {
            let abs = self.p(pth);
            match vfs.lstat(&abs) {
                Err(e) => err.push_str(&format!("ls: cannot access '{}': {}\n", pth, errstr(e))),
                Ok(_) => {
                    if vfs.stat(&abs).map(|s| s.kind == Kind::Dir).unwrap_or(false) {
                        dirs.push(pth.clone());
                    } else {
                        files.push(pth.clone());
                    }
                }
            }
        }
        let mut out = String::new();
        if !files.is_empty() {
            let items: Vec<String> = files.iter().map(|f| fmt_entry(vfs, &self.p(f), f, long)).collect();
            out.push_str(&join_entries(&items, one));
        }
        for (i, d) in dirs.iter().enumerate() {
            if multi {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(&format!("{}:\n", d));
            }
            let abs = self.p(d);
            let mut names: Vec<String> = Vec::new();
            if all {
                names.push(".".into());
                names.push("..".into());
            }
            if let Ok(list) = vfs.readdir(&abs) {
                names.extend(list.into_iter().filter(|n| all || !n.starts_with('.')));
            }
            let items: Vec<String> = names.iter().map(|n| fmt_entry(vfs, &normalize(&abs, n), n, long)).collect();
            out.push_str(&join_entries(&items, one));
            let _ = i;
        }
        Res { out, code: if err.is_empty() { 0 } else { 2 }, err }
    }

    fn cmd_wc(&self, vfs: &Vfs, args: &[String], stdin: &str) -> Res {
        let (flags, files) = split_flags(args);
        let want = |c: char| flags.is_empty() || flags.contains(c);
        let mut err = String::new();
        let mut out = String::new();
        let fmt = |text: &str, label: &str| {
            let mut parts = Vec::new();
            if want('l') {
                parts.push(text.matches('\n').count().to_string());
            }
            if want('w') {
                parts.push(text.split_whitespace().count().to_string());
            }
            if want('c') {
                parts.push(text.len().to_string());
            }
            let mut s = parts.join(" ");
            if !label.is_empty() {
                s.push(' ');
                s.push_str(label);
            }
            s.push('\n');
            s
        };
        if files.is_empty() {
            out.push_str(&fmt(stdin, ""));
        } else {
            for f in &files {
                match vfs.read_file(&self.p(f)) {
                    Ok(d) => out.push_str(&fmt(&String::from_utf8_lossy(&d), f)),
                    Err(e) => err.push_str(&format!("wc: {}: {}\n", f, errstr(e))),
                }
            }
        }
        Res { out, code: (!err.is_empty()) as i32, err }
    }

    fn cmd_headtail(&self, vfs: &Vfs, name: &str, args: &[String], stdin: &str) -> Res {
        let mut n = 10usize;
        let mut files = Vec::new();
        let mut i = 0;
        while i < args.len() {
            if args[i] == "-n" && i + 1 < args.len() {
                n = args[i + 1].parse().unwrap_or(10);
                i += 2;
                continue;
            }
            if args[i].starts_with('-') && args[i].len() > 1 && args[i][1..].chars().all(|c| c.is_ascii_digit()) {
                n = args[i][1..].parse().unwrap_or(10);
            } else {
                files.push(args[i].clone());
            }
            i += 1;
        }
        let (text, err, code) = self.read_inputs(vfs, &files, stdin, name);
        let lines: Vec<&str> = text.lines().collect();
        let picked: Vec<&str> = if name == "head" {
            lines.iter().take(n).cloned().collect()
        } else {
            lines[lines.len().saturating_sub(n)..].to_vec()
        };
        Res { out: picked.iter().map(|l| format!("{}\n", l)).collect(), err, code }
    }

    fn cmd_grep(&self, vfs: &Vfs, args: &[String], stdin: &str) -> Res {
        let (flags, rest) = split_flags(args);
        if rest.is_empty() {
            return Res { out: String::new(), err: "grep: usage: grep [-ivn] PATTERN [FILE...]\n".into(), code: 2 };
        }
        let ci = flags.contains('i');
        let inv = flags.contains('v');
        let num = flags.contains('n');
        let pat = if ci { rest[0].to_lowercase() } else { rest[0].clone() };
        let files = &rest[1..];
        let multi = files.len() > 1;
        let mut out = String::new();
        let mut err = String::new();
        let mut matched = false;
        let sources: Vec<(String, String)> = if files.is_empty() {
            vec![(String::new(), stdin.to_string())]
        } else {
            files
                .iter()
                .filter_map(|f| match vfs.read_file(&self.p(f)) {
                    Ok(d) => Some((f.clone(), String::from_utf8_lossy(&d).into_owned())),
                    Err(e) => {
                        err.push_str(&format!("grep: {}: {}\n", f, errstr(e)));
                        None
                    }
                })
                .collect()
        };
        for (label, text) in sources {
            for (ln, line) in text.lines().enumerate() {
                let hay = if ci { line.to_lowercase() } else { line.to_string() };
                if hay.contains(&pat) != inv {
                    matched = true;
                    if multi {
                        out.push_str(&format!("{}:", label));
                    }
                    if num {
                        out.push_str(&format!("{}:", ln + 1));
                    }
                    out.push_str(line);
                    out.push('\n');
                }
            }
        }
        Res { out, code: if !err.is_empty() { 2 } else if matched { 0 } else { 1 }, err }
    }

    pub fn complete(&self, vfs: &Vfs, word: &str, is_first: bool) -> Vec<String> {
        if is_first && !word.contains('/') {
            let mut v: Vec<String> = COMMANDS.iter().filter(|c| c.starts_with(word)).map(|s| s.to_string()).collect();
            if let Ok(names) = vfs.readdir("/bin") {
                v.extend(names.into_iter().filter(|n| n.starts_with(word)));
            }
            v.sort();
            v.dedup();
            return v;
        }
        let (dir_part, pat) = match word.rfind('/') {
            Some(i) => (&word[..=i], &word[i + 1..]),
            None => ("", word),
        };
        let dir_abs = self.p(if dir_part.is_empty() { "." } else { dir_part });
        let mut out = Vec::new();
        if let Ok(names) = vfs.readdir(&dir_abs) {
            for n in names {
                if !n.starts_with(pat) || (n.starts_with('.') && !pat.starts_with('.')) {
                    continue;
                }
                let is_dir = vfs.stat(&normalize(&dir_abs, &n)).map(|s| s.kind == Kind::Dir).unwrap_or(false);
                out.push(format!("{}{}{}", dir_part, n, if is_dir { "/" } else { "" }));
            }
        }
        out
    }
}

fn join_entries(items: &[String], one: bool) -> String {
    if items.is_empty() {
        return String::new();
    }
    if one {
        items.iter().map(|i| format!("{}\n", i)).collect()
    } else {
        format!("{}\n", items.join("  "))
    }
}

fn color_name(kind: Kind, mode: u32, name: &str) -> String {
    match kind {
        Kind::Dir => format!("\x1b[1;34m{}\x1b[0m", name),
        Kind::Symlink => format!("\x1b[36m{}\x1b[0m", name),
        Kind::File if mode & 0o111 != 0 => format!("\x1b[1;32m{}\x1b[0m", name),
        Kind::File => name.to_string(),
    }
}

fn fmt_entry(vfs: &Vfs, abs: &str, name: &str, long: bool) -> String {
    let st = match vfs.lstat(abs) {
        Ok(s) => s,
        Err(_) => return name.to_string(),
    };
    let mut shown = color_name(st.kind, st.mode, name);
    if st.kind == Kind::Symlink {
        if let Ok(t) = vfs.readlink(abs) {
            shown.push_str(&format!(" -> {}", t));
        }
    }
    if long {
        format!(
            "{} 1 {:<5} {:<5} {:>6} {}",
            mode_string(st.kind, st.mode),
            id_name(st.uid),
            id_name(st.gid),
            st.size,
            shown
        )
    } else {
        shown
    }
}

fn rm_tree(vfs: &mut Vfs, abs: &str) -> Result<(), Errno> {
    if vfs.lstat(abs)?.kind == Kind::Dir {
        for child in vfs.readdir(abs)? {
            rm_tree(vfs, &format!("{}/{}", abs.trim_end_matches('/'), child))?;
        }
        vfs.rmdir(abs)
    } else {
        vfs.unlink(abs)
    }
}

fn cp_tree(vfs: &mut Vfs, src: &str, dst: &str, rec: bool) -> Result<(), String> {
    let st = vfs.lstat(src).map_err(|e| errstr(e).to_string())?;
    match st.kind {
        Kind::File => {
            let d = vfs.read_file(src).map_err(|e| errstr(e).to_string())?;
            vfs.write_file(dst, &d).map_err(|e| errstr(e).to_string())
        }
        Kind::Symlink => {
            let t = vfs.readlink(src).map_err(|e| errstr(e).to_string())?;
            vfs.symlink(&t, dst).map_err(|e| errstr(e).to_string())
        }
        Kind::Dir => {
            if !rec {
                return Err("-r not specified; omitting directory".into());
            }
            if dst == src || dst.starts_with(&format!("{}/", src.trim_end_matches('/'))) {
                return Err("cannot copy a directory into itself".into());
            }
            match vfs.mkdir(dst, 0o755) {
                Ok(()) | Err(Errno::Exist) => {}
                Err(e) => return Err(errstr(e).to_string()),
            }
            for child in vfs.readdir(src).map_err(|e| errstr(e).to_string())? {
                cp_tree(
                    vfs,
                    &format!("{}/{}", src.trim_end_matches('/'), child),
                    &format!("{}/{}", dst.trim_end_matches('/'), child),
                    true,
                )?;
            }
            Ok(())
        }
    }
}

fn tree_walk(vfs: &Vfs, abs: &str, prefix: &str, out: &mut String, dirs: &mut u32, files: &mut u32) {
    let names = match vfs.readdir(abs) {
        Ok(n) => n,
        Err(_) => return,
    };
    let count = names.len();
    for (i, n) in names.into_iter().enumerate() {
        let last = i + 1 == count;
        let child = format!("{}/{}", abs.trim_end_matches('/'), n);
        let st = vfs.lstat(&child).ok();
        let (kind, mode) = st.map(|s| (s.kind, s.mode)).unwrap_or((Kind::File, 0));
        out.push_str(&format!(
            "{}{} {}\n",
            prefix,
            if last { "└──" } else { "├──" },
            color_name(kind, mode, &n)
        ));
        if kind == Kind::Dir {
            *dirs += 1;
            let np = format!("{}{}", prefix, if last { "    " } else { "│   " });
            tree_walk(vfs, &child, &np, out, dirs, files);
        } else {
            *files += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip(s: &str) -> String {
        let mut out = String::new();
        let mut it = s.chars().peekable();
        while let Some(c) = it.next() {
            if c == '\x1b' {
                for n in it.by_ref() {
                    if n.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    struct TestHost(std::cell::RefCell<Vfs>);

    impl Host for TestHost {
        fn with_vfs(&self, f: &mut dyn FnMut(&mut Vfs)) {
            f(&mut self.0.borrow_mut());
        }
        fn find_exe(&self, _: &str, _: &str, _: &str) -> Option<String> {
            None
        }
        fn exec(&self, _: &ExecReq) -> Option<ExecRes> {
            None
        }
    }

    fn setup() -> (Shell, TestHost) {
        let mut v = Vfs::new();
        for d in ["/home", "/home/user", "/tmp"] {
            v.mkdir(d, 0o755).unwrap();
        }
        v.chown("/home/user", 1000, 1000).unwrap();
        v.chmod("/tmp", 0o1777).unwrap();
        v.set_creds(1000, 1000);
        (Shell::new(), TestHost(std::cell::RefCell::new(v)))
    }

    fn run(sh: &mut Shell, v: &mut TestHost, line: &str) -> String {
        strip(&sh.run_line(v, line))
    }

    #[test]
    fn echo_quotes_and_vars() {
        let (mut sh, mut v) = setup();
        assert_eq!(run(&mut sh, &mut v, "echo hello   world"), "hello world\n");
        assert_eq!(run(&mut sh, &mut v, "echo 'a  b' \"c  d\""), "a  b c  d\n");
        assert_eq!(run(&mut sh, &mut v, "echo $USER ${HOSTNAME} ~"), "user mavex /home/user\n");
        assert_eq!(run(&mut sh, &mut v, "echo '$USER'"), "$USER\n");
        assert_eq!(run(&mut sh, &mut v, "echo -n hi"), "hi");
        assert_eq!(run(&mut sh, &mut v, "echo a # comment"), "a\n");
    }

    #[test]
    fn navigation() {
        let (mut sh, mut v) = setup();
        assert_eq!(run(&mut sh, &mut v, "mkdir -p a/b/c && cd a/b && pwd"), "/home/user/a/b\n");
        assert_eq!(run(&mut sh, &mut v, "cd ../.. && pwd"), "/home/user\n");
        assert!(run(&mut sh, &mut v, "cd nope").contains("No such file or directory"));
        assert!(strip(&sh.prompt()).ends_with("user@mavex:~$ "));
        run(&mut sh, &mut v, "cd a");
        assert!(strip(&sh.prompt()).contains("~/a"));
    }

    #[test]
    fn redirect_and_pipes() {
        let (mut sh, mut v) = setup();
        run(&mut sh, &mut v, "echo one > f");
        run(&mut sh, &mut v, "echo two >> f");
        assert_eq!(run(&mut sh, &mut v, "cat f"), "one\ntwo\n");
        assert_eq!(run(&mut sh, &mut v, "cat f | wc -l"), "2\n");
        assert_eq!(run(&mut sh, &mut v, "cat f | grep tw"), "two\n");
        assert_eq!(run(&mut sh, &mut v, "echo b a c | wc -w"), "3\n");
        run(&mut sh, &mut v, "printf");
        assert_eq!(sh.last, 127);
    }

    #[test]
    fn connectors() {
        let (mut sh, mut v) = setup();
        assert_eq!(run(&mut sh, &mut v, "false && echo no || echo yes"), "yes\n");
        assert_eq!(run(&mut sh, &mut v, "true && echo yes || echo no"), "yes\n");
        assert_eq!(run(&mut sh, &mut v, "echo a; echo b"), "a\nb\n");
        assert_eq!(run(&mut sh, &mut v, "false; echo $?"), "1\n");
    }

    #[test]
    fn globbing() {
        let (mut sh, mut v) = setup();
        run(&mut sh, &mut v, "touch a.txt b.txt c.md");
        assert_eq!(run(&mut sh, &mut v, "echo *.txt"), "a.txt b.txt\n");
        assert_eq!(run(&mut sh, &mut v, "echo *.zzz"), "*.zzz\n");
        assert_eq!(run(&mut sh, &mut v, "echo ?.md"), "c.md\n");
        run(&mut sh, &mut v, "rm *.txt");
        assert_eq!(run(&mut sh, &mut v, "ls"), "c.md\n");
    }

    #[test]
    fn file_commands() {
        let (mut sh, mut v) = setup();
        run(&mut sh, &mut v, "mkdir d && echo x > d/f && cp -r d e && mv e/f e/g");
        assert_eq!(run(&mut sh, &mut v, "cat e/g"), "x\n");
        assert!(run(&mut sh, &mut v, "rmdir d").contains("not empty") || run(&mut sh, &mut v, "rmdir d").contains("Directory not empty"));
        assert!(run(&mut sh, &mut v, "rm d").contains("Is a directory"));
        run(&mut sh, &mut v, "rm -rf d e");
        assert_eq!(run(&mut sh, &mut v, "ls"), "");
        run(&mut sh, &mut v, "echo hi > t && ln -s t l");
        assert_eq!(run(&mut sh, &mut v, "readlink l"), "t\n");
        assert_eq!(run(&mut sh, &mut v, "cat l"), "hi\n");
        assert!(run(&mut sh, &mut v, "ls -l").contains("l -> t"));
    }

    #[test]
    fn text_tools() {
        let (mut sh, mut v) = setup();
        run(&mut sh, &mut v, "echo c > s; echo a >> s; echo b >> s");
        assert_eq!(run(&mut sh, &mut v, "sort s"), "a\nb\nc\n");
        assert_eq!(run(&mut sh, &mut v, "sort -r s"), "c\nb\na\n");
        assert_eq!(run(&mut sh, &mut v, "head -n 1 s"), "c\n");
        assert_eq!(run(&mut sh, &mut v, "tail -1 s"), "b\n");
        assert_eq!(run(&mut sh, &mut v, "grep -n a s"), "2:a\n");
        assert_eq!(run(&mut sh, &mut v, "grep -v a s | wc -l"), "2\n");
    }

    #[test]
    fn errors_and_syntax() {
        let (mut sh, mut v) = setup();
        assert!(run(&mut sh, &mut v, "nosuch").contains("command not found"));
        assert!(run(&mut sh, &mut v, "echo 'oops").contains("unterminated"));
        assert!(run(&mut sh, &mut v, "echo a |").contains("syntax error"));
        assert!(run(&mut sh, &mut v, "cat /nope").contains("No such file or directory"));
        assert!(run(&mut sh, &mut v, "sleep 1 &").contains("not supported"));
    }

    #[test]
    fn misc_commands_and_completion() {
        let (mut sh, mut v) = setup();
        assert_eq!(run(&mut sh, &mut v, "uname -a"), "MavexOS mavex 0.0.8 wasm32\n");
        assert_eq!(run(&mut sh, &mut v, "whoami"), "user\n");
        run(&mut sh, &mut v, "export FOO=bar");
        assert_eq!(run(&mut sh, &mut v, "echo $FOO"), "bar\n");
        run(&mut sh, &mut v, "mkdir docs && touch dog.txt");
        let mut c = sh.complete(&v.0.borrow(), "do", false);
        c.sort();
        assert_eq!(c, vec!["docs/", "dog.txt"]);
        assert_eq!(sh.complete(&v.0.borrow(), "gre", true), vec!["grep"]);
        assert!(run(&mut sh, &mut v, "tree").contains("docs"));
        assert!(run(&mut sh, &mut v, "stat dog.txt").contains("regular file"));
        run(&mut sh, &mut v, "exit");
        assert!(sh.exited);
    }

    struct ExtHost(TestHost);

    impl Host for ExtHost {
        fn with_vfs(&self, f: &mut dyn FnMut(&mut Vfs)) {
            self.0.with_vfs(f)
        }
        fn find_exe(&self, name: &str, _: &str, _: &str) -> Option<String> {
            (name == "ext").then(|| "/bin/ext".to_string())
        }
        fn exec(&self, req: &ExecReq) -> Option<ExecRes> {
            Some(ExecRes {
                out: format!("[{} stdin={:?} tty={} cols={}]\n", req.argv[1..].join(","), req.stdin, req.tty, req.env["COLUMNS"]),
                err: String::new(),
                code: 3,
            })
        }
    }

    #[test]
    fn external_programs() {
        let (mut sh, th) = setup();
        let mut h = ExtHost(th);
        sh.cols = 100;
        assert_eq!(strip(&sh.run_line(&h, "ext a b")), "[a,b stdin=None tty=true cols=100]\n");
        assert_eq!(sh.last, 3);
        assert_eq!(strip(&sh.run_line(&h, "echo hi | ext x")), "[x stdin=Some(\"hi\\n\") tty=true cols=100]\n");
        assert_eq!(strip(&sh.run_line(&h, "ext y | cat")), "[y stdin=None tty=false cols=100]\n");
        sh.run_line(&h, "ext z > out");
        assert_eq!(h.0 .0.borrow().read_file("/home/user/out").unwrap(), b"[z stdin=None tty=false cols=100]\n");
        let _ = &mut h;
    }
}

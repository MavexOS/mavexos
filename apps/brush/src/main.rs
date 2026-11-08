use std::cell::RefCell;

mod kill;
use brush_builtins::{BuiltinSet, ShellBuilderExt};
use brush_core::{Shell, extensions::DefaultShellExtensions};

#[link(wasm_import_module = "mavex")]
unsafe extern "C" {
    fn park();
}

thread_local! {
    static RT: RefCell<Option<tokio::runtime::Runtime>> = const { RefCell::new(None) };
    static SH: RefCell<Option<Shell<DefaultShellExtensions>>> = const { RefCell::new(None) };
    static OUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

fn block<T>(f: impl std::future::Future<Output = T>) -> T {
    RT.with(|r| r.borrow().as_ref().unwrap().block_on(f))
}

fn drive_jobs() {
    brush_core::sys::wasm::mavex::set_driving(true);
    block(async { for _ in 0..32 { tokio::task::yield_now().await; } });
    brush_core::sys::wasm::mavex::set_driving(false);
}

fn main() {
    if let Ok(pwd) = std::env::var("PWD") {
        let _ = std::env::set_current_dir(pwd);
    }
    let rt = tokio::runtime::Builder::new_current_thread().build().expect("rt");
    RT.with(|r| *r.borrow_mut() = Some(rt));
    let shell = block(async {
        Shell::builder()
            .default_builtins(BuiltinSet::BashMode)
            .builtin("kill", brush_core::builtins::builtin::<kill::KillCommand, brush_core::extensions::DefaultShellExtensions>())
            .interactive(true)
            .profile(brush_core::ProfileLoadBehavior::Skip)
            .rc(brush_core::RcLoadBehavior::Skip)
            .shell_name("brush".to_string())
            .build()
            .await
    });
    match shell {
        Ok(s) => SH.with(|c| *c.borrow_mut() = Some(s)),
        Err(e) => {
            eprintln!("brush: init failed: {e}");
            std::process::exit(1);
        }
    }
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 && (args[1] == "--version" || args[1] == "-V") {
        println!("brush {}", env!("CARGO_PKG_VERSION"));
        std::process::exit(0);
    }
    if args.len() > 1 {
        let (script, _rest) = if args[1] == "-c" {
            (args.get(2).cloned().unwrap_or_default(), &args[args.len().min(3)..])
        } else {
            match std::fs::read_to_string(&args[1]) {
                Ok(t) => (t, &args[2..]),
                Err(e) => {
                    eprintln!("brush: {}: {e}", args[1]);
                    std::process::exit(127);
                }
            }
        };
        let code = SH.with(|c| {
            let mut g = c.borrow_mut();
            let sh = g.as_mut().unwrap();
            let params = sh.default_exec_params();
            match block(sh.run_string(script, &brush_core::SourceInfo::from("<cmd>"), &params)) {
                Ok(r) => u8::from(r.exit_code) as i32,
                Err(e) => {
                    eprintln!("brush: {e}");
                    1
                }
            }
        });
        std::process::exit(code);
    }
    SH.with(|c| {
        let mut g = c.borrow_mut();
        let sh = g.as_mut().unwrap();
        let params = sh.default_exec_params();
        let rc = concat!("export BRUSH_VERSION=", env!("CARGO_PKG_VERSION"), "; [ -r /etc/profile ] && . /etc/profile; [ -r \"$HOME/.bashrc\" ] && . \"$HOME/.bashrc\"; true");
        let _ = block(sh.run_string(rc.to_string(), &brush_core::SourceInfo::from("<rc>"), &params));
    });
    unsafe { park() };
}

fn take_out(bytes: Vec<u8>) -> i32 {
    OUT.with(|o| *o.borrow_mut() = bytes);
    OUT.with(|o| o.borrow().as_ptr() as i32)
}

#[unsafe(no_mangle)]
pub extern "C" fn brush_alloc(n: i32) -> i32 {
    let mut v = Vec::<u8>::with_capacity(n as usize);
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn brush_out_len() -> i32 {
    OUT.with(|o| o.borrow().len() as i32)
}

/// Runs one line; returns exit status, or -1000-N if the shell asked to exit with N.
#[unsafe(no_mangle)]
pub extern "C" fn brush_run(ptr: i32, len: i32) -> i32 {
    let line = unsafe { String::from_utf8_lossy(std::slice::from_raw_parts(ptr as *const u8, len as usize)).into_owned() };
    SH.with(|c| {
        let mut g = c.borrow_mut();
        let sh = g.as_mut().unwrap();
        let params = sh.default_exec_params();
        let _ = sh.add_to_history(&line);
        let r = block(sh.run_string(line, &brush_core::SourceInfo::from("<tty>"), &params));
        drive_jobs();
        match r {
            Ok(res) => {
                let code: u8 = res.exit_code.into();
                if matches!(res.next_control_flow, brush_core::ExecutionControlFlow::ExitShell) {
                    -1000 - code as i32
                } else {
                    code as i32
                }
            }
            Err(e) => {
                eprintln!("brush: {e}");
                1
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn brush_prompt() -> i32 {
    let s = SH.with(|c| {
        let mut g = c.borrow_mut();
        drive_jobs();
        brush_core::sys::wasm::mavex::set_driving(true);
        let _ = g.as_mut().unwrap().check_for_completed_jobs();
        brush_core::sys::wasm::mavex::set_driving(false);
        block(g.as_mut().unwrap().compose_prompt()).unwrap_or_else(|_| "$ ".into())
    });
    take_out(s.into_bytes())
}

#[unsafe(no_mangle)]
pub extern "C" fn brush_cwd() -> i32 {
    let s = SH.with(|c| c.borrow().as_ref().unwrap().working_dir().to_string_lossy().into_owned());
    take_out(s.into_bytes())
}

/// Completes at byte `pos` of the line. Output: "insert\tdelete\tfilenames\n" then one candidate per line.
#[unsafe(no_mangle)]
pub extern "C" fn brush_complete(ptr: i32, len: i32, pos: i32) -> i32 {
    let line = unsafe { String::from_utf8_lossy(std::slice::from_raw_parts(ptr as *const u8, len as usize)).into_owned() };
    let s = SH.with(|c| {
        let mut g = c.borrow_mut();
        let sh = g.as_mut().unwrap();
        let cwd = sh.working_dir().to_path_buf();
        match block(sh.complete(&line, pos as usize)) {
            Ok(comp) => {
                let files = comp.options.treat_as_filenames;
                let mut o = format!("{}\t{}\t{}\n", comp.insertion_index, comp.delete_count, files as u8);
                let mut cands = comp.candidates.into_iter().collect::<Vec<_>>();
                cands.sort();
                for mut cand in cands {
                    if files && !cand.ends_with('/') && cwd.join(&cand).is_dir() { cand.push('/'); }
                    o.push_str(&cand);
                    o.push('\n');
                }
                o
            }
            Err(_) => String::new(),
        }
    });
    take_out(s.into_bytes())
}

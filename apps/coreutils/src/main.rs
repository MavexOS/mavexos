use std::ffi::OsString;

const UTILS: &[&str] = &["basename", "cat", "cp", "cut", "date", "dirname", "echo", "env", "false", "head", "ln", "ls", "mkdir", "mv", "printf", "pwd", "readlink", "realpath", "rm", "rmdir", "seq", "sleep", "sort", "tail", "tee", "touch", "tr", "true", "uname", "uniq", "wc", "yes"];

fn main() {
    if let Ok(pwd) = std::env::var("PWD") {
        let _ = std::env::set_current_dir(pwd);
    }
    let mut args: Vec<OsString> = std::env::args_os().collect();
    let mut name = args
        .first()
        .map(|a| a.to_string_lossy().rsplit('/').next().unwrap_or("").to_string())
        .unwrap_or_default();
    if name == "coreutils" {
        if args.len() < 2 {
            println!("usage: coreutils UTILITY [ARGS...]\navailable: {}", UTILS.join(" "));
            std::process::exit(0);
        }
        args.remove(0);
        name = args[0].to_string_lossy().to_string();
    }
    let code = match name.as_str() {
        "basename" => uu_basename::uumain(args.into_iter()),
        "cat" => uu_cat::uumain(args.into_iter()),
        "cp" => uu_cp::uumain(args.into_iter()),
        "cut" => uu_cut::uumain(args.into_iter()),
        "date" => uu_date::uumain(args.into_iter()),
        "dirname" => uu_dirname::uumain(args.into_iter()),
        "echo" => uu_echo::uumain(args.into_iter()),
        "env" => uu_env::uumain(args.into_iter()),
        "false" => uu_false::uumain(args.into_iter()),
        "head" => uu_head::uumain(args.into_iter()),
        "ln" => uu_ln::uumain(args.into_iter()),
        "ls" => uu_ls::uumain(args.into_iter()),
        "mkdir" => uu_mkdir::uumain(args.into_iter()),
        "mv" => uu_mv::uumain(args.into_iter()),
        "printf" => uu_printf::uumain(args.into_iter()),
        "pwd" => uu_pwd::uumain(args.into_iter()),
        "readlink" => uu_readlink::uumain(args.into_iter()),
        "realpath" => uu_realpath::uumain(args.into_iter()),
        "rm" => uu_rm::uumain(args.into_iter()),
        "rmdir" => uu_rmdir::uumain(args.into_iter()),
        "seq" => uu_seq::uumain(args.into_iter()),
        "sleep" => uu_sleep::uumain(args.into_iter()),
        "sort" => uu_sort::uumain(args.into_iter()),
        "tail" => uu_tail::uumain(args.into_iter()),
        "tee" => uu_tee::uumain(args.into_iter()),
        "touch" => uu_touch::uumain(args.into_iter()),
        "tr" => uu_tr::uumain(args.into_iter()),
        "true" => uu_true::uumain(args.into_iter()),
        "uname" => uu_uname::uumain(args.into_iter()),
        "uniq" => uu_uniq::uumain(args.into_iter()),
        "wc" => uu_wc::uumain(args.into_iter()),
        "yes" => uu_yes::uumain(args.into_iter()),
        other => {
            eprintln!("coreutils: unknown utility '{}'", other);
            127
        }
    };
    std::process::exit(code);
}

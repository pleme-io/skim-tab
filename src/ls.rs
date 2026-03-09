//! blx-ls — POSIX ls flag translator for eza.
//!
//! Parses packed POSIX flags like `-ltra` and translates them to eza equivalents.
//! Always adds `--icons --group-directories-first` as defaults.

use std::os::unix::process::CommandExt;
use std::process::Command;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut eza_args: Vec<String> = vec![
        "--icons".into(),
        "--group-directories-first".into(),
    ];
    let mut paths: Vec<String> = Vec::new();

    let mut has_l = false;
    let mut has_a = false;
    let mut has_t = false;
    let mut has_r = false;
    let mut has_1 = false;

    for arg in &args {
        if arg.starts_with("--") {
            eza_args.push(arg.clone());
        } else if arg.starts_with('-') {
            for c in arg[1..].chars() {
                match c {
                    'l' => has_l = true,
                    'a' | 'A' => has_a = true,
                    't' => has_t = true,
                    'r' => has_r = true,
                    '1' => has_1 = true,
                    _ => {} // ignore unknown short flags
                }
            }
        } else {
            paths.push(arg.clone());
        }
    }

    if has_l { eza_args.push("-l".into()); }
    if has_a { eza_args.push("-a".into()); }
    if has_t { eza_args.push("--sort=modified".into()); }
    if has_r { eza_args.push("--reverse".into()); }
    if has_1 { eza_args.push("-1".into()); }

    eza_args.extend(paths);

    let err = Command::new("eza").args(&eza_args).exec();
    eprintln!("blx-ls: failed to exec eza: {err}");
    std::process::exit(1);
}

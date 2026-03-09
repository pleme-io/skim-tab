//! blx-preview-git — git diff/log previewer for fzf-tab git completion.
//!
//! Usage: blx-preview-git <subcommand> <word> [group]
//!
//! subcommand: "diff" (for add/diff/restore), "log", "checkout"
//! For checkout: group determines diff vs log preview.

use std::process::{Command, Stdio, exit};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let subcmd = args.get(1).map(String::as_str).unwrap_or("");
    let word = args.get(2).map(String::as_str).unwrap_or("");
    let group = args.get(3).map(String::as_str).unwrap_or("");

    if word.is_empty() {
        exit(0);
    }

    let width = std::env::var("FZF_PREVIEW_COLUMNS").unwrap_or_else(|_| "80".into());

    match subcmd {
        "diff" => pipe_diff(word, &width),
        "log" => exec_log(word),
        "checkout" => {
            if group == "modified file" {
                pipe_diff(word, &width);
            } else {
                exec_log(word);
            }
        }
        _ => exit(0),
    }
}

fn pipe_diff(word: &str, width: &str) {
    let git = Command::new("git")
        .args(["diff", word])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();

    match git {
        Ok(child) => {
            let status = Command::new("delta")
                .args(["--width", width])
                .stdin(child.stdout.unwrap())
                .stderr(Stdio::null())
                .status();
            exit(status.map_or(1, |s| s.code().unwrap_or(1)));
        }
        Err(_) => exit(1),
    }
}

fn exec_log(word: &str) -> ! {
    use std::os::unix::process::CommandExt;
    let err = Command::new("git")
        .args(["log", "--oneline", "--graph", "--color=always", word])
        .exec();
    eprintln!("blx-preview-git: {err}");
    exit(1);
}

//! blx-preview-proc — process detail previewer for kill/ps completion.
//!
//! Usage: blx-preview-proc <group> <word>
//!
//! If group is "[process ID]", shows ps details for the given PID.

use std::os::unix::process::CommandExt;
use std::process::Command;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let group = args.get(1).map(String::as_str).unwrap_or("");
    let word = args.get(2).map(String::as_str).unwrap_or("");

    if group == "[process ID]" && !word.is_empty() {
        let err = Command::new("ps")
            .args(["-p", word, "-o", "comm,pid,ppid,%cpu,%mem,start,time,command"])
            .exec();
        eprintln!("blx-preview-proc: {err}");
        std::process::exit(1);
    }
}

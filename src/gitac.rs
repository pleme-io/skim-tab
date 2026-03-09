//! git-ac — git add --all && git commit -m "<message>".
//!
//! Usage: git-ac <message words...>

use std::process::{Command, exit};

fn main() {
    let msg: String = std::env::args().skip(1).collect::<Vec<_>>().join(" ");
    if msg.is_empty() {
        eprintln!("usage: git-ac <commit message>");
        exit(1);
    }

    let add = Command::new("git").args(["add", "--all"]).status();
    if !add.map_or(false, |s| s.success()) {
        eprintln!("git-ac: git add --all failed");
        exit(1);
    }

    let commit = Command::new("git").args(["commit", "-m", &msg]).status();
    if !commit.map_or(false, |s| s.success()) {
        exit(1);
    }
}

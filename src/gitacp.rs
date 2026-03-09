//! git-acp — git add --all && git commit -m && git push.
//!
//! Usage: git-acp <message words...>

use std::process::{Command, exit};

fn main() {
    let msg: String = std::env::args().skip(1).collect::<Vec<_>>().join(" ");
    if msg.is_empty() {
        eprintln!("usage: git-acp <commit message>");
        exit(1);
    }

    let add = Command::new("git").args(["add", "--all"]).status();
    if !add.map_or(false, |s| s.success()) {
        eprintln!("git-acp: git add --all failed");
        exit(1);
    }

    let commit = Command::new("git").args(["commit", "-m", &msg]).status();
    if !commit.map_or(false, |s| s.success()) {
        exit(1);
    }

    let push = Command::new("git").args(["push"]).status();
    if !push.map_or(false, |s| s.success()) {
        eprintln!("git-acp: git push failed");
        exit(1);
    }
}

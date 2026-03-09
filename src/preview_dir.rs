//! blx-preview-dir — directory tree previewer for fzf-tab cd/z/pushd.
//!
//! Usage: blx-preview-dir <path>
//!
//! Shows eza tree view. Falls back to ls -la if eza unavailable.

use std::os::unix::process::CommandExt;
use std::process::Command;

fn main() {
    let path = match std::env::args().nth(1) {
        Some(p) if !p.is_empty() => p,
        _ => std::process::exit(0),
    };

    let err = Command::new("eza")
        .args(["--tree", "--level=2", "--icons", "--color=always", &path])
        .exec();
    // eza not found — fall back to ls
    let _ = Command::new("ls").args(["-la", &path]).exec();
    eprintln!("blx-preview-dir: {err}");
    std::process::exit(1);
}

//! blx-preview — universal file/directory previewer for fzf-tab.
//!
//! Usage: blx-preview <path>
//!
//! Directories: eza tree view. Files: bat with syntax highlighting.
//! Empty/missing path: silent exit 0.

use std::os::unix::process::CommandExt;
use std::process::Command;

fn main() {
    let path = match std::env::args().nth(1) {
        Some(p) if !p.is_empty() => p,
        _ => std::process::exit(0),
    };

    let meta = match std::fs::metadata(&path) {
        Ok(m) => m,
        Err(_) => std::process::exit(0),
    };

    if meta.is_dir() {
        let err = Command::new("eza")
            .args(["--tree", "--level=2", "--icons", "--color=always", &path])
            .exec();
        eprintln!("blx-preview: eza: {err}");
    } else if meta.is_file() {
        let err = Command::new("bat")
            .args([
                "--color=always",
                "--style=numbers",
                "--line-range=:200",
                &path,
            ])
            .exec();
        eprintln!("blx-preview: bat: {err}");
    }
}

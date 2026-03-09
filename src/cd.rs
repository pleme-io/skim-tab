//! skim-cd — Rust-native fuzzy directory navigator for zsh.
//!
//! Runs `fd` to discover directories, presents them via skim with
//! eza tree preview, and prints the selected directory to stdout.

use std::env;
use std::io;
use std::process::Command;

use anyhow::{Context, Result};
use skim::options::MatchScheme;
use skim::prelude::SkimItemReader;
use skim::tui::options::PreviewLayout;
use skim::Skim;
use skim_tab::{base_options, parse_query, shell_quote, ICON_CD};

/// Run fd to discover directories.
fn discover_dirs() -> Result<String> {
    let output = Command::new("fd")
        .args([
            "--type",
            "d",
            "--hidden",
            "--follow",
            "--exclude",
            ".git",
            "--exclude",
            "node_modules",
            "--exclude",
            "target",
            "--exclude",
            "__pycache__",
            "--exclude",
            ".direnv",
            "--strip-cwd-prefix",
        ])
        .output()
        .context("failed to run fd — is it installed?")?;

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Preview command: eza tree.
fn preview_command() -> String {
    "eza --tree --level=2 --icons --color=always {} 2>/dev/null".to_string()
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    let query = parse_query(&args);

    let entries = discover_dirs()?;
    if entries.is_empty() {
        return Ok(());
    }

    let item_reader = SkimItemReader::default();
    let items = item_reader.of_bufread(io::Cursor::new(entries));

    let options = base_options(query)
        .scheme(MatchScheme::Path)
        .prompt(ICON_CD.to_string())
        .preview(preview_command())
        .preview_window(PreviewLayout::from("right:50%:wrap"))
        .header("Directories | CTRL-/: Toggle Preview | ESC: Cancel".to_string())
        .build()
        .expect("failed to build skim options");

    match Skim::run_with(options, Some(items)) {
        Ok(out) if !out.is_abort => {
            if let Some(item) = out.selected_items.first() {
                print!("{}", shell_quote(&item.output()));
            }
        }
        _ => {}
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_command_uses_eza() {
        let cmd = preview_command();
        assert!(cmd.contains("eza"));
        assert!(cmd.contains("--tree"));
    }
}

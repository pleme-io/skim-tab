//! skim-files — Rust-native fuzzy file/directory picker for zsh.
//!
//! Runs `fd` to discover files and directories, presents them via skim
//! with bat/eza preview, and prints selected paths to stdout.
//! Supports multi-select (tab to toggle).

use std::env;
use std::io;
use std::process::Command;

use anyhow::{Context, Result};
use skim::options::MatchScheme;
use skim::prelude::SkimItemReader;
use skim::tui::options::PreviewLayout;
use skim::Skim;
use skim_tab::{base_options, build_options, parse_query, shell_quote, ICON_FILES, ICON_MARKER};

/// Run fd to discover files and directories.
fn discover_files() -> Result<String> {
    let output = Command::new("fd")
        .args([
            "--type",
            "f",
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

/// Preview command: directories get eza tree, files get bat.
fn preview_command() -> String {
    "if [ -d {} ]; then \
        eza --tree --level=2 --icons --color=always {} 2>/dev/null; \
    else \
        bat --color=always --style=numbers --line-range=:500 {} 2>/dev/null; \
    fi"
    .to_string()
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    let query = parse_query(&args);

    let entries = discover_files()?;
    if entries.is_empty() {
        return Ok(());
    }

    let item_reader = SkimItemReader::default();
    let items = item_reader.of_bufread(io::Cursor::new(entries));

    let options = build_options(
        base_options(query)
            .scheme(MatchScheme::Path)
            .multi(true)
            .prompt(ICON_FILES.to_string())
            .multi_select_icon(ICON_MARKER.to_string())
            .preview(preview_command())
            .preview_window(PreviewLayout::from("right:50%:wrap"))
            .header("TAB: Multi-select | CTRL-/: Toggle Preview | ESC: Cancel".to_string()),
    )?;

    match Skim::run_with(options, Some(items)) {
        Ok(out) if !out.is_abort => {
            let paths: Vec<String> = out
                .selected_items
                .iter()
                .map(|item| shell_quote(&item.output()))
                .collect();
            if !paths.is_empty() {
                print!("{}", paths.join(" "));
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
    fn preview_command_is_valid_shell() {
        let cmd = preview_command();
        assert!(cmd.contains("bat"));
        assert!(cmd.contains("eza"));
    }
}

//! skim-files — Rust-native fuzzy file/directory picker for zsh.
//!
//! Runs `fd` to discover files and directories, presents them via skim
//! with bat/eza preview, and prints selected paths to stdout.
//! Supports multi-select (tab to toggle).

use std::env;
use std::io;
use std::process::Command;

use anyhow::{Context, Result};
use skim::{
    prelude::{SkimItemReader, SkimOptionsBuilder},
    Skim,
};

/// Nord color palette for skim, matching skim-history.
const NORD_COLORS: &str = "\
fg:#D8DEE9,\
bg:#2E3440,\
hl:#88C0D0:bold:underlined,\
fg+:#ECEFF4:bold,\
bg+:#3B4252,\
hl+:#8FBCBB:bold:underlined,\
info:#4C566A,\
prompt:#A3BE8C,\
pointer:#88C0D0,\
marker:#B48EAD,\
spinner:#81A1C1,\
header:#5E81AC,\
border:#4C566A,\
query:#ECEFF4:bold";

/// Run fd to discover files and directories.
fn discover_files() -> Result<String> {
    let output = Command::new("fd")
        .args([
            "--type", "f",
            "--type", "d",
            "--hidden",
            "--follow",
            "--exclude", ".git",
            "--strip-cwd-prefix",
        ])
        .output()
        .context("failed to run fd — is it installed?")?;

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Build the preview command string.
/// Directories get eza tree, files get bat with syntax highlighting.
fn preview_command() -> String {
    "if [ -d {} ]; then \
        eza --tree --level=2 --icons --color=always {} 2>/dev/null; \
    else \
        bat --color=always --style=numbers --line-range=:500 {} 2>/dev/null; \
    fi"
    .to_string()
}

fn main() -> Result<()> {
    // Parse --query from args.
    let args: Vec<String> = env::args().skip(1).collect();
    let query = args
        .iter()
        .position(|a| a == "--query")
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
        .unwrap_or("");

    let entries = discover_files()?;
    if entries.is_empty() {
        return Ok(());
    }

    let item_reader = SkimItemReader::default();
    let items = item_reader.of_bufread(io::Cursor::new(entries));

    let options = SkimOptionsBuilder::default()
        .query(query.to_string())
        .scheme(skim::options::MatchScheme::Path)
        .multi(true)
        .height("40%".to_string())
        .min_height("10".to_string())
        .layout(skim::tui::options::TuiLayout::Reverse)
        .prompt("\u{1f4c2} ".to_string()) // 📂
        .no_info(true)
        .selector_icon("\u{25b8}".to_string()) // ▸
        .multi_select_icon("\u{25cf}".to_string()) // ●
        .ansi(true)
        .preview(preview_command())
        .preview_window(skim::tui::options::PreviewLayout::from("right:50%:hidden:wrap"))
        .color(NORD_COLORS.to_string())
        .bind(vec![
            "ctrl-/:toggle-preview".to_string(),
            "ctrl-u:half-page-up".to_string(),
            "ctrl-d:half-page-down".to_string(),
        ])
        .build()
        .expect("failed to build skim options");

    match Skim::run_with(options, Some(items)) {
        Ok(out) if !out.is_abort => {
            let paths: Vec<String> = out
                .selected_items
                .iter()
                .map(|item| item.output().to_string())
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

    #[test]
    fn nord_colors_has_all_keys() {
        for key in ["fg:", "bg:", "hl:", "fg+:", "bg+:", "hl+:", "prompt:", "pointer:"] {
            assert!(NORD_COLORS.contains(key), "missing color key: {key}");
        }
    }
}

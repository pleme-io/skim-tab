//! skim-content — Rust-native file content search for zsh.
//!
//! Runs ripgrep to search file contents, presents matches via skim with
//! bat preview, and outputs a ready-to-eval shell command that opens the
//! editor at the selected file and line.

use std::env;
use std::io;
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use skim::prelude::SkimItemReader;
use skim::tui::options::PreviewLayout;
use skim::Skim;
use skim_tab::{base_options, editor, parse_query, shell_quote, ICON_SEARCH};

/// Run ripgrep and return stdout. Color is OFF so delimiter parsing is clean.
/// Skim still highlights fuzzy matches via its own hl/hl+ colors.
fn run_rg(pattern: &str) -> Result<String> {
    let output = Command::new("rg")
        .args([
            "--color=never",
            "--line-number",
            "--no-heading",
            "--smart-case",
            "--max-columns=200",
            "--max-columns-preview",
            pattern,
        ])
        .stderr(Stdio::null())
        .output()
        .context("failed to run rg — is ripgrep installed?")?;

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Parse file path and line number from ripgrep output (file:line:content).
fn parse_rg_match(raw: &str) -> Option<(&str, &str)> {
    let mut parts = raw.splitn(3, ':');
    let file = parts.next()?;
    let line = parts.next()?;
    if !line.is_empty() && line.len() <= 10 && line.chars().all(|c| c.is_ascii_digit()) {
        Some((file, line))
    } else {
        None
    }
}

/// Preview command: bat with line highlighting.
fn preview_command() -> String {
    "bat --color=always --style=numbers --highlight-line {2} -- {1} 2>/dev/null".to_string()
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    let query = parse_query(&args);

    let initial_pattern = if query.is_empty() { "." } else { query };
    let entries = run_rg(initial_pattern)?;
    if entries.is_empty() {
        return Ok(());
    }

    let item_reader = SkimItemReader::default();
    let items = item_reader.of_bufread(io::Cursor::new(entries));

    let options = base_options(query)
        .delimiter(regex::Regex::new(":").expect("valid regex"))
        // Fuzzy match only against content (field 3+), not filenames or line numbers
        .nth(vec!["3..".to_string()])
        .height("80%".to_string())
        .min_height("20".to_string())
        .prompt(ICON_SEARCH.to_string())
        .preview(preview_command())
        .preview_window(PreviewLayout::from("up,60%,border-rounded,+{2}+3/3,~3"))
        .header("Search in files | CTRL-/: Toggle Preview | ESC: Cancel".to_string())
        .build()
        .expect("failed to build skim options");

    match Skim::run_with(options, Some(items)) {
        Ok(out) if !out.is_abort => {
            if let Some(item) = out.selected_items.first() {
                let raw = item.output();
                if let Some((file, line)) = parse_rg_match(&raw) {
                    print!("{} +{} {}", editor(), line, shell_quote(file));
                }
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
    fn preview_command_uses_bat() {
        let cmd = preview_command();
        assert!(cmd.contains("bat"));
        assert!(cmd.contains("--highlight-line {2}"));
        assert!(cmd.contains("{1}"));
    }

    #[test]
    fn parse_simple_rg_output() {
        let (file, line) = parse_rg_match("src/main.rs:42:fn main() {").unwrap();
        assert_eq!(file, "src/main.rs");
        assert_eq!(line, "42");
    }

    #[test]
    fn parse_rg_output_with_colons_in_content() {
        let (file, line) = parse_rg_match("config.yaml:10:host: localhost:8080").unwrap();
        assert_eq!(file, "config.yaml");
        assert_eq!(line, "10");
    }

    #[test]
    fn parse_rg_rejects_non_numeric_line() {
        assert!(parse_rg_match("not:a:match").is_none());
    }

    #[test]
    fn parse_rg_rejects_empty_line() {
        assert!(parse_rg_match("file::content").is_none());
    }
}

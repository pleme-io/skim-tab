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

    // ── parse_rg_match edge cases ────────────────────────────────────

    #[test]
    fn parse_rg_match_large_line_number() {
        let (file, line) = parse_rg_match("file.rs:9999999999:content").unwrap();
        assert_eq!(file, "file.rs");
        assert_eq!(line, "9999999999");
    }

    #[test]
    fn parse_rg_match_line_number_too_long() {
        // 11+ digit line numbers rejected (len > 10)
        assert!(parse_rg_match("file.rs:12345678901:content").is_none());
    }

    #[test]
    fn parse_rg_match_path_with_dots() {
        let (file, line) = parse_rg_match("src/k8s.rs:100:fn test()").unwrap();
        assert_eq!(file, "src/k8s.rs");
        assert_eq!(line, "100");
    }

    #[test]
    fn parse_rg_match_empty_file() {
        // Empty file path portion
        let result = parse_rg_match(":42:content");
        // First field is empty string, but line is valid
        assert!(result.is_some());
        assert_eq!(result.unwrap(), ("", "42"));
    }

    #[test]
    fn parse_rg_match_no_content() {
        // file:line: with empty content portion
        let (file, line) = parse_rg_match("file.rs:42:").unwrap();
        assert_eq!(file, "file.rs");
        assert_eq!(line, "42");
    }

    #[test]
    fn parse_rg_match_single_colon() {
        assert!(parse_rg_match("only_one_colon:content").is_none());
    }

    #[test]
    fn parse_rg_match_no_colons() {
        assert!(parse_rg_match("nocolonsatall").is_none());
    }

    #[test]
    fn preview_command_has_required_placeholders() {
        let cmd = preview_command();
        assert!(cmd.contains("{1}"), "should have file placeholder");
        assert!(cmd.contains("{2}"), "should have line placeholder");
    }
}

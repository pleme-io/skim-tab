//! skim-history — Rust-native fuzzy history search for zsh.
//!
//! Reads ~/.zsh_history (extended format), deduplicates keeping most recent,
//! presents via skim, and prints the selected command to stdout.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{self, BufRead};
use std::path::PathBuf;

use anyhow::{Context, Result};
use skim::{
    options::MatchScheme,
    prelude::{SkimItemReader, SkimOptionsBuilder},
    Skim,
};

/// Parse a zsh extended history line.
/// Format: `: TIMESTAMP:0;COMMAND` or just `COMMAND` (plain format).
fn parse_history_line(line: &str) -> &str {
    if line.starts_with(": ") {
        if let Some(pos) = line.find(';') {
            return &line[pos + 1..];
        }
    }
    line
}

/// Resolve the history file path.
fn history_path() -> PathBuf {
    if let Ok(path) = env::var("HISTFILE") {
        return PathBuf::from(path);
    }
    let home = env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".zsh_history")
}

/// Read history, deduplicate (most recent wins), return in reverse order.
fn read_history() -> Result<Vec<String>> {
    let path = history_path();
    let file =
        fs::File::open(&path).with_context(|| format!("failed to open {}", path.display()))?;
    let reader = io::BufReader::new(file);

    // Track insertion order with a counter; higher = more recent.
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut counter: usize = 0;

    // Handle multi-line commands (lines ending with backslash).
    let mut continuation = String::new();

    for line in reader.lines() {
        let line = line.context("failed to read history line")?;
        if !continuation.is_empty() {
            continuation.push('\n');
            continuation.push_str(&line);
            if !line.ends_with('\\') {
                let cmd = parse_history_line(&continuation).to_owned();
                if !cmd.is_empty() {
                    seen.insert(cmd, counter);
                    counter += 1;
                }
                continuation.clear();
            }
            continue;
        }
        if line.ends_with('\\') {
            continuation = line;
            continue;
        }

        let cmd = parse_history_line(&line);
        if !cmd.is_empty() {
            seen.insert(cmd.to_owned(), counter);
            counter += 1;
        }
    }

    // Sort by counter descending (most recent first).
    let mut entries: Vec<(String, usize)> = seen.into_iter().collect();
    entries.sort_unstable_by(|a, b| b.1.cmp(&a.1));

    Ok(entries.into_iter().map(|(cmd, _)| cmd).collect())
}

fn main() -> Result<()> {
    let entries = read_history()?;
    if entries.is_empty() {
        return Ok(());
    }

    // Parse args.
    let args: Vec<String> = env::args().skip(1).collect();
    let query = args
        .iter()
        .position(|a| a == "--query")
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
        .unwrap_or("");
    let show_count = args.iter().any(|a| a == "--count");

    let input = entries.join("\n");
    let item_reader = SkimItemReader::default();
    let items = item_reader.of_bufread(io::Cursor::new(input));

    let header = if show_count {
        format!(" {} commands", entries.len())
    } else {
        String::new()
    };

    let mut builder = SkimOptionsBuilder::default();
    builder
        .query(query.to_string())
        .no_sort(true)
        .scheme(MatchScheme::History)
        .height("40%".to_string())
        .min_height("10".to_string())
        .layout(skim::tui::options::TuiLayout::Reverse)
        .border(skim::tui::BorderType::Rounded)
        .prompt("\u{276f} ".to_string())
        .info(skim::tui::statusline::InfoDisplay::Inline)
        .selector_icon("\u{25b8}".to_string())
        .ansi(true)
        .color(
            [
                "fg:#D8DEE9",
                "bg:#2E3440",
                "hl:#88C0D0",
                "fg+:#ECEFF4",
                "bg+:#3B4252",
                "hl+:#8FBCBB",
                "info:#81A1C1",
                "prompt:#A3BE8C",
                "pointer:#BF616A",
                "marker:#B48EAD",
                "spinner:#81A1C1",
                "header:#5E81AC",
                "border:#4C566A",
                "query:#ECEFF4",
            ]
            .join(","),
        )
        .bind(vec![
            "ctrl-/:toggle-preview".to_string(),
            "ctrl-u:half-page-up".to_string(),
            "ctrl-d:half-page-down".to_string(),
        ]);

    if !header.is_empty() {
        builder.header(header);
    }

    let options = builder.build().expect("failed to build skim options");

    match Skim::run_with(options, Some(items)) {
        Ok(out) if !out.is_abort => {
            if let Some(item) = out.selected_items.first() {
                print!("{}", item.output());
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
    fn parse_extended_format() {
        assert_eq!(
            parse_history_line(": 1709876543:0;git status"),
            "git status"
        );
    }

    #[test]
    fn parse_plain_format() {
        assert_eq!(parse_history_line("ls -la"), "ls -la");
    }

    #[test]
    fn parse_extended_with_semicolons() {
        assert_eq!(
            parse_history_line(": 1709876543:0;echo foo; echo bar"),
            "echo foo; echo bar"
        );
    }

    #[test]
    fn parse_empty_command() {
        assert_eq!(parse_history_line(": 1709876543:0;"), "");
    }
}

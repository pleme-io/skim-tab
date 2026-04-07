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
use skim::options::MatchScheme;
use skim::prelude::SkimItemReader;
use skim::Skim;
use skim_tab::{base_options, build_options, parse_query, ICON_HISTORY};

/// Parse a zsh extended history line.
/// Format: `: TIMESTAMP:0;COMMAND` or just `COMMAND` (plain format).
fn parse_history_line(line: &str) -> &str {
    if line.starts_with(": ")
        && let Some(pos) = line.find(';')
    {
        return &line[pos + 1..];
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
///
/// Uses byte-level reading with lossy UTF-8 conversion so corrupted
/// history entries are cleaned up rather than crashing the search.
fn read_history() -> Result<Vec<String>> {
    let path = history_path();
    let file =
        fs::File::open(&path).with_context(|| format!("failed to open {}", path.display()))?;
    let mut reader = io::BufReader::new(file);

    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut counter: usize = 0;
    let mut continuation = String::new();
    let mut buf = Vec::new();

    loop {
        buf.clear();
        let bytes_read = reader
            .read_until(b'\n', &mut buf)
            .context("failed to read history file")?;
        if bytes_read == 0 {
            break;
        }
        if buf.last() == Some(&b'\n') {
            buf.pop();
        }
        let line = String::from_utf8_lossy(&buf);

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
            continuation = line.into_owned();
            continue;
        }

        let cmd = parse_history_line(&line);
        if !cmd.is_empty() {
            seen.insert(cmd.to_owned(), counter);
            counter += 1;
        }
    }

    let mut entries: Vec<(String, usize)> = seen.into_iter().collect();
    entries.sort_unstable_by_key(|e| std::cmp::Reverse(e.1));

    Ok(entries.into_iter().map(|(cmd, _)| cmd).collect())
}

fn main() -> Result<()> {
    let entries = read_history()?;
    if entries.is_empty() {
        return Ok(());
    }

    let args: Vec<String> = env::args().skip(1).collect();
    let query = parse_query(&args);

    let input = entries.join("\n");
    let item_reader = SkimItemReader::default();
    let items = item_reader.of_bufread(io::Cursor::new(input));

    let options = build_options(
        base_options(query)
            .no_sort(true)
            .scheme(MatchScheme::History)
            .prompt(ICON_HISTORY.to_string())
            .header("History | ESC: Cancel".to_string()),
    )?;

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

    // ── parse_history_line edge cases ────────────────────────────────

    #[test]
    fn parse_colon_space_but_no_semicolon() {
        // Starts with ": " but has no semicolon → should return full line
        assert_eq!(parse_history_line(": no semicolon here"), ": no semicolon here");
    }

    #[test]
    fn parse_empty_line() {
        assert_eq!(parse_history_line(""), "");
    }

    #[test]
    fn parse_multiline_command_single() {
        // A single line ending with backslash — the parser sees it as one line
        assert_eq!(
            parse_history_line(": 1709876543:0;echo hello\\"),
            "echo hello\\"
        );
    }

    #[test]
    fn parse_line_with_only_colon_space() {
        assert_eq!(parse_history_line(": "), ": ");
    }

    #[test]
    fn parse_plain_command_with_semicolons() {
        assert_eq!(
            parse_history_line("echo a; echo b; echo c"),
            "echo a; echo b; echo c"
        );
    }

    #[test]
    fn parse_extended_with_duration() {
        // Extended format with duration
        assert_eq!(
            parse_history_line(": 1709876543:123;long running cmd"),
            "long running cmd"
        );
    }

    // ── history_path ─────────────────────────────────────────────────

    #[test]
    fn history_path_uses_histfile_env() {
        unsafe { env::set_var("HISTFILE", "/tmp/test_history") };
        assert_eq!(history_path(), PathBuf::from("/tmp/test_history"));
        unsafe { env::remove_var("HISTFILE") };
    }

    #[test]
    fn history_path_defaults_to_zsh_history() {
        unsafe { env::remove_var("HISTFILE") };
        let path = history_path();
        assert!(path.to_str().unwrap().ends_with(".zsh_history"));
    }
}

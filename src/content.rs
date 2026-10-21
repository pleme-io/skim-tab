//! skim-content — Rust-native file content search for zsh.
//!
//! Runs ripgrep to search file contents, presents matches via skim with
//! bat preview, and outputs a ready-to-eval shell command that opens the
//! editor at the selected file and line.

use std::env;
use std::io::{self, BufRead, BufReader, Read};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use skim::prelude::SkimItemReader;
use skim::tui::options::PreviewLayout;
use skim_tab::{
    ICON_SEARCH, base_options, build_options, editor, parse_query, run_skim_on_tty, shell_quote,
};

/// Upper bound on the match lines this picker loads.
///
/// The same reasoning as `skim_tab::FD_MAX_RESULTS`: a picker is a human
/// selection surface, and past a few thousand rows every further match is
/// pure cost. The bound matters most where ignore rules do not apply —
/// measured at the workspace root, `rg` returns 103 lines in 0.011s with
/// ignore files honoured and 104,862,344 lines in 48.9s without them.
/// We never pass `--no-ignore`, and this cap means even reaching that
/// direction by accident cannot freeze the terminal.
const RG_MAX_MATCHES: usize = 50_000;

/// One ripgrep run: the matches it produced and whether the cap cut it short.
struct RgRun {
    /// Matching `file:line:content` rows, at most [`RG_MAX_MATCHES`] of them.
    entries: String,
    /// True when [`RG_MAX_MATCHES`] stopped the read before `rg` was done.
    truncated: bool,
}

/// Build ripgrep's argument vector for `pattern`.
///
/// Color is OFF so delimiter parsing is clean — skim still highlights fuzzy
/// matches via its own hl/hl+ colors.
fn rg_args(pattern: &str) -> Vec<String> {
    let mut args: Vec<String> = [
        "--color=never",
        "--line-number",
        "--no-heading",
        "--smart-case",
        "--max-columns=200",
        "--max-columns-preview",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect();

    // The pattern is whatever was sitting in the shell line editor — text the
    // operator typed, not a regex they authored. `--fixed-strings` makes
    // `echo (foo` a literal search instead of "regex parse error: unclosed
    // group", and `--regexp` keeps a query starting with `-` from being read
    // as a flag.
    if pattern.is_empty() {
        // An empty buffer means "show me everything". A literal empty pattern
        // would also match every blank line, so this one case stays a regex.
        args.push("--regexp".to_string());
        args.push(".".to_string());
    } else {
        args.push("--fixed-strings".to_string());
        args.push("--regexp".to_string());
        args.push(pattern.to_string());
    }

    args
}

/// Run ripgrep and return its matches, bounded by [`RG_MAX_MATCHES`].
///
/// `rg`'s stderr is **captured and reported**, never discarded: it is where
/// ripgrep puts unreadable paths and its own usage errors, and a picker that
/// silently shows nothing is indistinguishable from one that was never given
/// a chance to run.
fn run_rg(pattern: &str) -> Result<RgRun> {
    let mut child = Command::new("rg")
        .args(rg_args(pattern))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to run rg — is ripgrep installed?")?;

    // Drain stderr on its own thread. Reading stdout to completion first would
    // deadlock the moment rg's error stream (one line per unreadable path)
    // filled the pipe buffer: rg blocks writing stderr, we block reading stdout.
    let mut stderr = child.stderr.take().expect("stderr was piped");
    let stderr_pump = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        buf
    });

    let stdout = child.stdout.take().expect("stdout was piped");
    let mut reader = BufReader::new(stdout);
    let mut entries = String::new();
    let mut matches = 0usize;
    let mut truncated = false;
    let mut line: Vec<u8> = Vec::new();
    loop {
        if matches >= RG_MAX_MATCHES {
            truncated = true;
            let _ = child.kill();
            break;
        }
        line.clear();
        let read = reader
            .read_until(b'\n', &mut line)
            .context("failed to read rg output")?;
        if read == 0 {
            break;
        }
        entries.push_str(&String::from_utf8_lossy(&line));
        matches += 1;
    }
    drop(reader);

    let status = child.wait().context("rg did not exit cleanly")?;
    let stderr = stderr_pump.join().unwrap_or_default();

    // rg's exit codes: 0 = matched, 1 = no match (not an error), 2 = failure.
    // A killed child reports neither, and the cap is not an error either.
    if !truncated && !matches!(status.code(), Some(0 | 1)) {
        let message = String::from_utf8_lossy(&stderr);
        let message = message.trim();
        if message.is_empty() {
            anyhow::bail!("rg exited with {status}");
        }
        anyhow::bail!("rg failed: {message}");
    }

    Ok(RgRun { entries, truncated })
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

    let RgRun { entries, truncated } = run_rg(query)?;
    if entries.is_empty() {
        // Silence here used to be indistinguishable from success: an invalid
        // regex made rg exit 2, its stderr was discarded, and the empty result
        // returned Ok(()) — so C-f on `echo (foo` did nothing, said nothing.
        // Report it on stderr, the way skim-kpod reports "No pods found".
        if query.is_empty() {
            eprintln!("skim-tab: nothing to search here");
        } else {
            eprintln!("skim-tab: no matches for {query:?}");
        }
        return Ok(());
    }

    let item_reader = SkimItemReader::default();
    let items = item_reader.of_bufread(io::Cursor::new(entries));

    // Honest header: rg ran ONCE, before the picker opened. Typing in the
    // picker filters that frozen set — it never re-runs the search — so a
    // query can only ever narrow these results, never broaden them.
    let header = if truncated {
        format!(
            "Filter matches (fixed at launch, capped at {RG_MAX_MATCHES}) | CTRL-/: Toggle Preview | ESC: Cancel"
        )
    } else {
        "Filter matches (fixed at launch) | CTRL-/: Toggle Preview | ESC: Cancel".to_string()
    };

    let colon_delim = regex::Regex::new(":").context("failed to compile delimiter regex")?;
    let options = build_options(
        base_options(query)
            .delimiter(colon_delim)
            .nth(vec!["3..".to_string()])
            .height("80%".to_string())
            .min_height("20".to_string())
            .prompt(ICON_SEARCH.to_string())
            .preview(preview_command())
            .preview_window(PreviewLayout::from("up,60%,border-rounded,+{2}+3/3,~3"))
            .header(header),
    )?;

    match run_skim_on_tty(options, Some(items)) {
        Some(out) if !out.is_abort => {
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

    // ── rg argument construction ─────────────────────────────────────

    /// The whole point of the literal default: `echo (foo` is a shell buffer,
    /// not a regex. Passed as a regex it makes rg exit 2 with "unclosed
    /// group" and the picker showed nothing at all.
    #[test]
    fn rg_args_treat_the_query_as_a_literal() {
        let args = rg_args("echo (foo");
        assert!(
            args.iter().any(|a| a == "--fixed-strings"),
            "query must be literal by default: {args:?}"
        );
        assert_eq!(args.last().unwrap(), "echo (foo");
    }

    /// A query is always introduced by `--regexp`, so one starting with `-`
    /// is a search term and never a flag.
    #[test]
    fn rg_args_pass_the_query_behind_regexp() {
        let args = rg_args("--hidden");
        let idx = args.iter().position(|a| a == "--hidden").unwrap();
        assert_eq!(args[idx - 1], "--regexp");
    }

    /// The empty buffer means "everything", and stays a regex: a literal
    /// empty pattern would also match every blank line in the tree.
    #[test]
    fn rg_args_empty_query_matches_every_nonblank_line() {
        let args = rg_args("");
        assert!(!args.iter().any(|a| a == "--fixed-strings"));
        assert_eq!(args.last().unwrap(), ".");
    }

    #[test]
    fn rg_args_never_lose_the_parse_critical_flags() {
        for pattern in ["", "needle"] {
            let args = rg_args(pattern);
            for required in ["--color=never", "--line-number", "--no-heading"] {
                assert!(
                    args.iter().any(|a| a == required),
                    "{pattern:?} lost {required}: {args:?}"
                );
            }
        }
    }

    /// An invalid regex is now a literal search that simply finds nothing —
    /// and `run_rg` reports rg's own failures instead of swallowing them.
    #[test]
    fn run_rg_survives_a_shell_buffer_that_is_not_a_regex() {
        match run_rg("echo (foo") {
            Ok(run) => assert!(!run.truncated),
            // ripgrep absent from the test environment — that error is the
            // only one this call is allowed to produce.
            Err(e) => assert!(
                e.to_string().contains("is ripgrep installed"),
                "unexpected failure: {e:#}"
            ),
        }
    }

    #[test]
    fn preview_command_has_required_placeholders() {
        let cmd = preview_command();
        assert!(cmd.contains("{1}"), "should have file placeholder");
        assert!(cmd.contains("{2}"), "should have line placeholder");
    }
}

//! skim-history — Rust-native fuzzy history search for zsh.
//!
//! Reads ~/.zsh_history (extended format), deduplicates keeping most recent,
//! presents via skim, and prints the selected command to stdout.

use std::env;
use std::fs;
use std::io::{self, BufRead};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use skim::options::MatchScheme;
use skim::prelude::SkimItemReader;
use skim_tab::tags::TagSet;
use skim_tab::usage_rank::{self, UsageEntry};
use skim_tab::{ICON_HISTORY, base_options, build_options, parse_query, run_skim_on_tty};

/// Parse a zsh extended history line, keeping only the command.
///
/// Delegates to [`usage_rank::parse_history_entry`], which additionally
/// recovers the timestamp this function threw away — that discarded timestamp
/// is precisely why Ctrl-R could not rank by usage.
///
/// Test-only: it survives at its original signature purely so the parser tests
/// below keep asserting the command-extraction contract against unedited
/// expectations. The picker itself no longer discards the timestamp.
#[cfg(test)]
fn parse_history_line(line: &str) -> &str {
    usage_rank::parse_history_entry(line).1
}

/// Current wall-clock as unix-epoch seconds.
fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Resolve the history file path.
///
/// `$HISTFILE` is taken VERBATIM, including a relative one, and that is
/// deliberate: zsh itself resolves a relative `HISTFILE` against the cwd, so
/// rejecting one here would make skim-tab disagree with the shell whose
/// history it is reading. Faithfulness to the shell beats the absolute rule on
/// this specific variable — see `theory/MASKED-BRANCH.md` §V on not changing
/// what a valid configuration resolves to.
///
/// `$HOME` is different. It used to fall back to `"."`, which is not a
/// plausible home directory — it made the path `./.zsh_history`, so the
/// ranking silently read the cwd's history, or nothing at all, depending on
/// where the process started. An unusable `$HOME` now yields an absolute path
/// that simply does not exist, which the caller already handles as "no
/// history" instead of quietly reading the wrong file.
fn history_path() -> PathBuf {
    if let Some(path) = env::var_os("HISTFILE") {
        return PathBuf::from(path);
    }
    if let Some(home) = env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
    {
        return home.join(".zsh_history");
    }
    std::env::temp_dir().join("skim-tab-no-home.zsh_history")
}

/// Read the resolved `$HISTFILE` as accumulated per-command usage.
fn read_usage() -> Result<Vec<UsageEntry>> {
    read_usage_from(&history_path())
}

/// Read a history file into **logical** lines, joining `\`-continuations and
/// leaving the `: <epoch>:<duration>;` prefix intact so the timestamp survives
/// as far as the accumulator.
///
/// Uses byte-level reading with lossy UTF-8 conversion so corrupted history
/// entries are cleaned up rather than crashing the search.
fn read_lines_from(path: &std::path::Path) -> Result<Vec<String>> {
    let file =
        fs::File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let mut reader = io::BufReader::new(file);

    let mut lines: Vec<String> = Vec::new();
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
                lines.push(std::mem::take(&mut continuation));
            }
            continue;
        }
        if line.ends_with('\\') {
            continuation = line.into_owned();
            continue;
        }
        lines.push(line.into_owned());
    }

    // An unterminated continuation at EOF is discarded, matching the behavior
    // this refactor replaced. Recovering it is a real (if rare) fix — a
    // dangling `\` loses the operator's last command — but it is deliberately
    // NOT folded in here, so `read_history_from`'s tests stay a clean parity
    // oracle for this change rather than an edited one.

    Ok(lines)
}

/// Accumulate a history file into one [`UsageEntry`] per distinct command,
/// preserving occurrence counts and per-occurrence timestamps.
fn read_usage_from(path: &std::path::Path) -> Result<Vec<UsageEntry>> {
    let lines = read_lines_from(path)?;
    Ok(usage_rank::accumulate(lines.iter().map(String::as_str)))
}

/// Deduplicated commands, **most-recent-first** — the pre-frecency feed.
///
/// A converting shim over [`read_usage_from`], kept at its original signature
/// so the ordering tests below still pin the recency contract they were written
/// for. They are this refactor's parity oracle: they assert what the old
/// hand-rolled dedup produced, and they are not edited by it.
///
/// Test-only — the picker reads [`read_usage`] and ranks it. `RankMode::Recency`
/// is the live equivalent for an operator who wants this order back.
#[cfg(test)]
fn read_history_from(path: &std::path::Path) -> Result<Vec<String>> {
    Ok(usage_rank::rank(
        read_usage_from(path)?,
        usage_rank::RankOptions {
            mode: usage_rank::RankMode::Recency,
            pin_most_recent: false,
        },
        0,
    ))
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    let cfg = skim_tab::config::load();

    // `skim-history --explain <line>` — why did that rank where it did?
    // A re-ranker you cannot interrogate is a re-ranker you stop trusting,
    // and the picker itself is the worst place to debug one.
    if let Some(pos) = args.iter().position(|a| a == "--explain") {
        let set = TagSet::new(&cfg.tags);
        for warning in TagSet::lint(&cfg.tags) {
            eprintln!("warning: tags: {warning}");
        }
        let subject = args[pos + 1..].join(" ");
        let verdict = set.explain(&subject);
        println!(
            "{}\t{}\tweight={}",
            subject,
            verdict.tag.as_deref().unwrap_or("<untagged>"),
            verdict.weight
        );
        return Ok(());
    }

    // `skim-history --rank-report [N]` — the feed the picker WOULD show, with
    // the score and occurrence count that put each line there, alongside the
    // pre-frecency order for comparison. Same reasoning as `--explain`: a
    // re-ranker you cannot interrogate is a re-ranker you stop trusting, and
    // this is the surface that answers "is it actually ranking my real
    // history, or only my fixtures?".
    if let Some(pos) = args.iter().position(|a| a == "--rank-report") {
        let top: usize = args.get(pos + 1).and_then(|s| s.parse().ok()).unwrap_or(15);
        let usage = read_usage()?;
        let now = now_unix();
        let opts = cfg.history.rank_options();

        let old = usage_rank::rank(
            usage.clone(),
            usage_rank::RankOptions {
                mode: usage_rank::RankMode::Recency,
                pin_most_recent: false,
            },
            now,
        );
        let new = usage_rank::rank_entries(usage.clone(), opts, now);

        println!(
            "{} distinct commands | mode={:?} pin_most_recent={}",
            usage.len(),
            opts.mode,
            opts.pin_most_recent
        );
        println!("{:>4}  {:>9}  {:>4}  {}", "#", "score", "uses", "command");
        for (i, e) in new.iter().take(top).enumerate() {
            println!(
                "{:>4}  {:>9.4}  {:>4}  {}",
                i + 1,
                e.frecency(now),
                e.occurrences,
                e.command
            );
        }
        println!("\n-- previous (pure positional recency) top {top} --");
        for (i, c) in old.iter().take(top).enumerate() {
            println!("{:>4}  {c}", i + 1);
        }
        return Ok(());
    }

    let usage = read_usage()?;
    if usage.is_empty() {
        return Ok(());
    }

    let query = parse_query(&args);

    // ── Usage frecency ──────────────────────────────────────────────
    // Time-decayed usage frequency, so a command the operator leans on rises
    // over one they ran once. Both inputs come free from the history file: the
    // `: <epoch>:0;` timestamp (988 of 1008 lines on the live fleet) and the
    // repeat count that the old `HashMap::insert` dedup destroyed.
    let mut entries = usage_rank::rank(usage, cfg.history.rank_options(), now_unix());

    // ── Tag priority ────────────────────────────────────────────────
    // Applied to the FEED, because the picker runs `no_sort(true)` —
    // skim will not re-rank, so this order is literally what the operator
    // sees. Stable, so the frecency ordering above survives intact *inside*
    // each tag tier: tags choose the tier, frecency chooses the order within
    // one. Must therefore run AFTER the frecency pass, never before.
    let tags = TagSet::new(&cfg.tags);
    if tags.applies_to_history() {
        tags.rank(&mut entries, String::as_str);
    }

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

    match run_skim_on_tty(options, Some(items)) {
        Some(out) if !out.is_abort => {
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
        assert_eq!(
            parse_history_line(": no semicolon here"),
            ": no semicolon here"
        );
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

    /// Both `history_path` branches in ONE test, deliberately.
    ///
    /// They used to be two, and `$HISTFILE` is process-global while cargo
    /// runs tests on parallel threads — so one test's `set_var` raced the
    /// other's `remove_var` and whichever lost reported a real-looking
    /// failure with a real-looking diff. It failed roughly every other
    /// run on this machine, in both directions.
    ///
    /// A mutex would serialise the pair; folding them into one test means
    /// there is no second writer to race with at all. Order matters: set
    /// first, then clear, so the env is left the way the rest of the
    /// suite expects to find it.
    #[test]
    fn history_path_prefers_histfile_then_falls_back_to_zsh_history() {
        unsafe { env::set_var("HISTFILE", "/tmp/test_history") };
        assert_eq!(history_path(), PathBuf::from("/tmp/test_history"));

        unsafe { env::remove_var("HISTFILE") };
        let path = history_path();
        assert!(
            path.to_str().unwrap().ends_with(".zsh_history"),
            "with no HISTFILE the path falls back under $HOME: {path:?}"
        );
    }

    // ── read_history_from ordering (the Ctrl-R "most recent on top" bug) ──

    fn write_hist(lines: &[&str]) -> tempfile::NamedTempFile {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
        f.flush().unwrap();
        f
    }

    /// The core contract: the LAST line in the file (the most-recently-run
    /// command, as frost/reedline write it: plain newline-separated, append
    /// order = run order) is index 0 of `read_history_from` — i.e. the TOP,
    /// pre-selected item in the Ctrl-R picker. Regression for the operator
    /// report "Ctrl-R doesn't prioritize my most-recent command".
    #[test]
    fn most_recent_command_is_first() {
        let f = write_hist(&["echo one", "echo two", "echo three"]);
        let got = read_history_from(f.path()).unwrap();
        assert_eq!(
            got,
            vec![
                "echo three".to_string(),
                "echo two".to_string(),
                "echo one".to_string(),
            ],
            "most-recently-run command (last line) must be first in the picker feed"
        );
        // The single most important guarantee, stated bluntly:
        assert_eq!(got.first().map(String::as_str), Some("echo three"));
    }

    /// frost's real on-disk format is plain (one command per line, no zsh
    /// `: ts:0;` prefix). Confirm the parser leaves plain lines intact and
    /// still orders most-recent-first.
    #[test]
    fn frost_plain_format_orders_most_recent_first() {
        let f = write_hist(&["git status", "cargo build", "ls -la"]);
        let got = read_history_from(f.path()).unwrap();
        assert_eq!(got.first().map(String::as_str), Some("ls -la"));
        assert_eq!(got.last().map(String::as_str), Some("git status"));
    }

    /// Re-running an OLD command moves it to the top: dedup keeps the
    /// most-recent occurrence's position. After `ls; cd; ls`, a Ctrl-R
    /// shows the just-run `ls` first (not stale at the bottom).
    #[test]
    fn rerun_moves_command_to_top() {
        let f = write_hist(&["ls", "cd /tmp", "ls"]);
        let got = read_history_from(f.path()).unwrap();
        assert_eq!(got.first().map(String::as_str), Some("ls"));
        // Deduped: `ls` appears once.
        assert_eq!(got.iter().filter(|c| *c == "ls").count(), 1);
        assert_eq!(got, vec!["ls".to_string(), "cd /tmp".to_string()]);
    }

    /// The actual Ctrl-R contract, end to end: read the file, then rank.
    /// Neither half is interesting alone — the composition is, because
    /// `no_sort(true)` means whatever comes out of here IS what the
    /// operator sees, and the tag pass must promote git without
    /// scrambling the recency ordering `most_recent_command_is_first`
    /// exists to protect.
    #[test]
    fn tagged_feed_promotes_git_and_keeps_recency_inside_each_tier() {
        use skim_tab::tags::{TagRule, TagSet, TagsConfig};

        let f = write_hist(&[
            "git clone x", // oldest
            "ls -la",
            "cargo build",
            "git status",
            "echo hi", // newest
        ]);
        let mut got = read_history_from(f.path()).unwrap();
        // Precondition: plain recency, newest first.
        assert_eq!(got.first().map(String::as_str), Some("echo hi"));

        let set = TagSet::new(&TagsConfig {
            enable: true,
            rules: vec![TagRule {
                tag: "git".into(),
                weight: 100,
                command: vec!["git".into()],
                ..TagRule::default()
            }],
            ..TagsConfig::default()
        });
        set.rank(&mut got, String::as_str);

        assert_eq!(
            got,
            vec![
                // git tier — still newest-first among themselves
                "git status".to_string(),
                "git clone x".to_string(),
                // untagged tier — untouched relative order
                "echo hi".to_string(),
                "cargo build".to_string(),
                "ls -la".to_string(),
            ],
            "tags choose the tier; recency still chooses the order within one"
        );
    }

    /// Mixed plain + zsh-extended lines still order most-recent-first and
    /// dedup across formats so a command written once plain and once
    /// extended doesn't split the most-recent wins logic.
    #[test]
    fn extended_and_plain_dedup_consistently() {
        let f = write_hist(&["echo hi", ": 1709876543:0;echo hi", "echo bye"]);
        let got = read_history_from(f.path()).unwrap();
        assert_eq!(got.first().map(String::as_str), Some("echo bye"));
        // `echo hi` deduped to one entry despite two on-disk encodings.
        assert_eq!(got.iter().filter(|c| *c == "echo hi").count(), 1);
    }
}

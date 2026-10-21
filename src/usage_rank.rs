//! Usage-frecency ranking for the picker feeds.
//!
//! ## Why this module exists
//!
//! `skim-history` (Ctrl-R) used to throw the ranking signal away **twice**:
//! the parser discarded the unix timestamp zsh's extended format carries on
//! nearly every line (988 of 1008 on the live fleet history), and the reader
//! deduplicated with `HashMap::insert`, destroying the repeat count. What was
//! left was pure positional recency.
//!
//! ## The ranking lives in `wadachi_spec::usage`
//!
//! Accumulation, [`RankMode`], [`RankOptions`] (including the pin that keeps
//! "re-run what I just ran" first) and the ordering were born here and now
//! live in [`wadachi_spec::usage`], so every history picker in the fleet
//! (this one, arnes's prompt history) orders its feed one way. This module
//! keeps what is zsh-specific: reading a history line into a
//! `(timestamp, command)` pair.

pub use wadachi_spec::usage::{RankMode, RankOptions, UsageEntry, rank, rank_entries};

/// Split a zsh history line into its optional timestamp and its command.
///
/// Extended format is `: <epoch>:<duration>;<command>`; plain format is the
/// bare command. A line that merely *starts* with `": "` but is not
/// well-formed extended is returned whole, with no timestamp — the historical
/// behavior, kept so a stray `: ` line is never silently truncated.
#[must_use]
pub fn parse_history_entry(line: &str) -> (Option<i64>, &str) {
    let Some(rest) = line.strip_prefix(": ") else {
        return (None, line);
    };
    let Some(semi) = line.find(';') else {
        return (None, line);
    };
    let command = &line[semi + 1..];

    // `rest` up to the ';' must be `<digits>:<digits>` for the timestamp to be
    // real. Anything else is a command that happens to open with ": ".
    let meta = &rest[..rest.len() - (line.len() - semi)];
    let Some((epoch, duration)) = meta.split_once(':') else {
        return (None, command);
    };
    if epoch.is_empty()
        || !epoch.bytes().all(|b| b.is_ascii_digit())
        || duration.is_empty()
        || !duration.bytes().all(|b| b.is_ascii_digit())
    {
        return (None, command);
    }
    (epoch.parse::<i64>().ok(), command)
}

/// Accumulate raw history lines (oldest → newest) into one entry per distinct
/// command, preserving occurrence counts and timestamps.
#[must_use]
pub fn accumulate<'a>(lines: impl IntoIterator<Item = &'a str>) -> Vec<UsageEntry> {
    wadachi_spec::usage::accumulate(lines.into_iter().map(parse_history_entry))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_history_entry ──────────────────────────────────────────

    #[test]
    fn extended_format_yields_timestamp_and_command() {
        // The exact shape measured in the live fleet history file.
        assert_eq!(
            parse_history_entry(": 1771238081:0;cd nexus/pkgs"),
            (Some(1_771_238_081), "cd nexus/pkgs")
        );
    }

    #[test]
    fn plain_format_yields_no_timestamp() {
        assert_eq!(parse_history_entry("ls -la"), (None, "ls -la"));
    }

    #[test]
    fn command_containing_semicolons_survives() {
        assert_eq!(
            parse_history_entry(": 1709876543:0;echo foo; echo bar"),
            (Some(1_709_876_543), "echo foo; echo bar")
        );
    }

    #[test]
    fn colon_space_without_valid_meta_is_returned_whole() {
        // Historical behavior: not extended format, so never truncated.
        assert_eq!(
            parse_history_entry(": no semicolon here"),
            (None, ": no semicolon here")
        );
    }

    #[test]
    fn non_numeric_meta_is_not_read_as_a_timestamp() {
        let (ts, cmd) = parse_history_entry(": abc:0;real command");
        assert_eq!(ts, None, "a non-numeric epoch is not a timestamp");
        assert_eq!(cmd, "real command");
    }

    #[test]
    fn duration_field_is_ignored_but_required_numeric() {
        assert_eq!(
            parse_history_entry(": 1709876543:123;long running cmd"),
            (Some(1_709_876_543), "long running cmd")
        );
    }

    // ── accumulate: the repeat count that dedup used to destroy ───────

    #[test]
    fn repeated_command_accumulates_instead_of_overwriting() {
        let entries = accumulate(["ls", "cd /tmp", "ls", "ls"]);
        let ls = entries.iter().find(|e| e.command == "ls").unwrap();
        assert_eq!(ls.occurrences, 3, "three runs must count as three");
        assert_eq!(
            entries.len(),
            2,
            "still deduplicated into distinct commands"
        );
    }

    #[test]
    fn timestamps_are_collected_per_occurrence() {
        let entries = accumulate([": 100:0;git status", ": 200:0;git status"]);
        assert_eq!(entries[0].timestamps, vec![100, 200]);
        assert_eq!(entries[0].occurrences, 2);
    }

    #[test]
    fn mixed_formats_dedup_together_and_count_both() {
        let entries = accumulate(["echo hi", ": 1709876543:0;echo hi"]);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].occurrences, 2);
        assert_eq!(
            entries[0].timestamps.len(),
            1,
            "only the extended line carried a timestamp"
        );
    }

    #[test]
    fn empty_commands_are_skipped() {
        let entries = accumulate([": 1709876543:0;", "real"]);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].command, "real");
    }

    // ── frecency scoring ─────────────────────────────────────────────

    #[test]
    fn frecent_command_outscores_a_stale_one() {
        let now = 1_000_000_000i64;
        let day = 86_400i64;
        let entries = accumulate([": 999913600:0;fresh"]); // ~1 day old
        let fresh = entries[0].frecency(now);

        let stale = accumulate([": 991360000:0;stale"])[0].frecency(now); // ~100 days
        assert!(
            fresh > stale,
            "recent must outscore stale: {fresh} vs {stale}"
        );
        let _ = day;
    }

    #[test]
    fn frequency_accumulates_into_the_score() {
        let now = 1_000_000_000i64;
        let once = accumulate([": 999913600:0;cmd"])[0].frecency(now);
        let thrice = accumulate([
            ": 999913600:0;cmd",
            ": 999913600:0;cmd",
            ": 999913600:0;cmd",
        ])[0]
            .frecency(now);
        assert!(
            thrice > once * 2.0,
            "three occurrences must score far above one: {thrice} vs {once}"
        );
    }

    #[test]
    fn untimestamped_entry_scores_zero_but_is_not_dropped() {
        let entries = accumulate(["plain"]);
        assert_eq!(entries[0].frecency(1_000_000_000), 0.0);
        let feed = rank(entries, RankOptions::default(), 1_000_000_000);
        assert_eq!(feed, vec!["plain".to_string()], "still fed to the picker");
    }

    // ── ranking ──────────────────────────────────────────────────────

    /// The operator-facing point of the whole change: a heavily-used command
    /// rises above a rarely-used one, where before both were ordered purely
    /// by position.
    #[test]
    fn frequently_used_command_rises_above_a_rare_recent_one() {
        let now = 2_000_000_000i64;
        let old = now - 3 * 86_400; // 3 days ago
        let lines = vec![
            format!(": {old}:0;git commit --amend"),
            format!(": {old}:0;git commit --amend"),
            format!(": {old}:0;git commit --amend"),
            format!(": {old}:0;git commit --amend"),
            format!(": {}:0;obscure-one-off", now - 2 * 86_400),
        ];
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let feed = rank(
            accumulate(refs),
            RankOptions {
                mode: RankMode::Frecency,
                pin_most_recent: false,
            },
            now,
        );
        assert_eq!(
            feed.first().map(String::as_str),
            Some("git commit --amend"),
            "four uses must beat one, even though the one-off is newer"
        );
    }

    /// The guard on the dominant Ctrl-R reflex. Without pinning, the 40×
    /// command wins and "re-run what I just ran" regresses.
    #[test]
    fn most_recent_is_pinned_above_a_far_more_frequent_command() {
        let now = 2_000_000_000i64;
        let old = now - 86_400;
        let mut lines: Vec<String> = (0..40).map(|_| format!(": {old}:0;git status")).collect();
        lines.push(format!(": {now}:0;cargo test -p the-crate-im-editing"));
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();

        let pinned = rank(accumulate(refs.clone()), RankOptions::default(), now);
        assert_eq!(
            pinned.first().map(String::as_str),
            Some("cargo test -p the-crate-im-editing"),
            "the just-run command must stay first"
        );

        // And prove the pin is what did it — unpinned, frequency wins.
        let unpinned = rank(
            accumulate(refs),
            RankOptions {
                mode: RankMode::Frecency,
                pin_most_recent: false,
            },
            now,
        );
        assert_eq!(
            unpinned.first().map(String::as_str),
            Some("git status"),
            "without the pin, 40 uses outrank the newest — this is the regression the pin prevents"
        );
    }

    /// `Recency` mode must reproduce the pre-frecency order exactly.
    #[test]
    fn recency_mode_is_newest_first() {
        let feed = rank(
            accumulate(["echo one", "echo two", "echo three"]),
            RankOptions {
                mode: RankMode::Recency,
                pin_most_recent: false,
            },
            0,
        );
        assert_eq!(
            feed,
            vec![
                "echo three".to_string(),
                "echo two".to_string(),
                "echo one".to_string()
            ]
        );
    }

    #[test]
    fn rerun_moves_command_to_top_in_recency_mode() {
        let feed = rank(
            accumulate(["ls", "cd /tmp", "ls"]),
            RankOptions {
                mode: RankMode::Recency,
                pin_most_recent: false,
            },
            0,
        );
        assert_eq!(feed, vec!["ls".to_string(), "cd /tmp".to_string()]);
    }

    #[test]
    fn empty_feed_ranks_to_empty() {
        assert!(rank(Vec::new(), RankOptions::default(), 0).is_empty());
    }

    /// A slash-bearing command must not be distorted. This is the defect that
    /// routing through `wadachi_spec::apply`'s path matcher would reintroduce:
    /// two identical-count commands differing only by a `/` scored 4.0 vs 0.5.
    #[test]
    fn slash_in_a_command_does_not_change_its_score() {
        let now = 2_000_000_000i64;
        let t = now - 86_400;
        let plain = accumulate([&format!(": {t}:0;grep -rn foo lib")[..]])[0].frecency(now);
        let slashed =
            accumulate([&format!(": {t}:0;grep -rn foo src/lib.rs")[..]])[0].frecency(now);
        assert_eq!(
            plain, slashed,
            "a '/' is command text, not a path separator, and must not reweight the score"
        );
    }
}

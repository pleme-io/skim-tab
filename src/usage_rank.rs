//! Usage-frecency ranking for the picker feeds.
//!
//! ## Why this module exists
//!
//! `skim-history` (Ctrl-R) used to throw the ranking signal away **twice**:
//!
//! 1. [`crate::usage_rank::parse_history_entry`]'s predecessor returned the
//!    slice *after* the `;`, discarding the unix timestamp that zsh's extended
//!    format carries on nearly every line (measured on the live fleet history:
//!    988 of 1008 lines).
//! 2. The reader deduplicated into a `HashMap<String, usize>` with
//!    `insert`, so N occurrences of a command collapsed to one and the
//!    **repeat count was destroyed at read time**.
//!
//! What was left was pure *positional recency* over deduplicated commands. A
//! command run forty times ranked exactly like a command run once.
//!
//! ## The formula is borrowed, not reinvented
//!
//! Scoring goes through [`wadachi_spec::DecayKind`] — the fleet's one frecency
//! decay, already a dependency of this crate. We deliberately do **not** route
//! through `wadachi_spec::apply`/`MatchProfile`: that matcher is *path-shaped*
//! (it reasons in components/basename/ancestors), and a command line is not a
//! path. Feeding command text to it mis-tokenizes on `/` — measured as an 8×
//! ranking distortion between two commands with identical visit counts
//! (`grep -rn foo lib` → 4.0 versus `grep -rn foo src/lib.rs` → 0.5). Needle
//! matching stays where it already works: inside skim.
//!
//! So this module borrows the **decay**, and owns only the accumulation and
//! the ordering.
//!
//! ## The muscle-memory invariant
//!
//! Frequency ranking competes with the dominant Ctrl-R use — *re-run the thing
//! I ran two commands ago*. Because hyperbolic-days decay is nearly flat
//! within a day (age 5min → 0.9965, age 1h → 0.976), a command run forty times
//! last week will outscore the command you just ran. Ranking on frecency alone
//! would therefore make the most common interaction **worse**.
//!
//! [`RankOptions::pin_most_recent`] resolves that: the single most-recently
//! executed command is placed first unconditionally, and frecency orders
//! everything beneath it. `most_recent_is_pinned_above_a_far_more_frequent_command`
//! is the regression test for exactly that.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Seconds in a day, as `f64` — the unit [`wadachi_spec::DecayKind`] decays in.
const SECS_PER_DAY: f64 = 86_400.0;

/// How a picker feed is ordered.
///
/// `Recency` is retained as a *named selection* rather than deleted, so the
/// pre-frecency ordering stays reachable for an operator who prefers it
/// (MODULARIZE, DON'T DELETE).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RankMode {
    /// Time-decayed frequency: `Σ 1/(1 + age_days)` over every occurrence.
    /// A command used often *and* recently rises; one used once long ago sinks.
    #[default]
    Frecency,
    /// Pure positional recency over deduplicated commands — the behavior
    /// shipped before frecency landed.
    Recency,
}

/// Knobs for one ranking pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RankOptions {
    /// Ordering to apply.
    pub mode: RankMode,
    /// Place the most-recently-executed entry first regardless of score.
    /// Protects the "re-run what I just ran" reflex from frequency.
    pub pin_most_recent: bool,
}

impl Default for RankOptions {
    fn default() -> Self {
        Self {
            mode: RankMode::default(),
            pin_most_recent: true,
        }
    }
}

/// One distinct command and every time it was observed.
///
/// Unlike the `HashMap<String, usize>` this replaced, an entry **accumulates**:
/// re-running a command appends rather than overwrites, which is what makes
/// frequency recoverable at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageEntry {
    /// The command text, exactly as it will be fed to the picker.
    pub command: String,
    /// Unix-epoch seconds for each occurrence that carried a timestamp,
    /// ascending. Empty when the history file is in plain format.
    pub timestamps: Vec<i64>,
    /// File position of the most recent occurrence. Orders `Recency` mode and
    /// breaks ties in `Frecency` mode, so an untimestamped history still
    /// ranks sensibly instead of collapsing to an arbitrary order.
    pub last_ordinal: usize,
    /// Total occurrences, including any that carried no timestamp. Always
    /// `>= timestamps.len()`.
    pub occurrences: usize,
}

impl UsageEntry {
    /// `Σ 1/(1 + age_days)` across every timestamped occurrence, using the
    /// fleet's shared decay from [`wadachi_spec::DecayKind::HyperbolicDays`].
    ///
    /// Returns `0.0` when nothing is timestamped — such entries fall back to
    /// `last_ordinal` ordering rather than being dropped.
    #[must_use]
    pub fn frecency(&self, now_unix: i64) -> f64 {
        self.timestamps
            .iter()
            .map(|ts| {
                let age_days = ((now_unix - ts) as f64 / SECS_PER_DAY).max(0.0);
                wadachi_spec::DecayKind::HyperbolicDays.decay(age_days, 0.0)
            })
            .sum()
    }
}

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
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut entries: Vec<UsageEntry> = Vec::new();
    let mut ordinal = 0usize;

    for line in lines {
        let (ts, command) = parse_history_entry(line);
        if command.is_empty() {
            continue;
        }
        let slot = match index.get(command) {
            Some(&i) => i,
            None => {
                entries.push(UsageEntry {
                    command: command.to_owned(),
                    timestamps: Vec::new(),
                    last_ordinal: ordinal,
                    occurrences: 0,
                });
                index.insert(command.to_owned(), entries.len() - 1);
                entries.len() - 1
            }
        };
        let entry = &mut entries[slot];
        entry.occurrences += 1;
        entry.last_ordinal = ordinal;
        if let Some(ts) = ts {
            entry.timestamps.push(ts);
        }
        ordinal += 1;
    }

    entries
}

/// Order `entries` into the feed the picker displays, best-first, keeping the
/// entries themselves so a caller can report *why* an item landed where it did.
///
/// The picker runs with `no_sort(true)`, so this order **is** what the operator
/// sees — there is no downstream re-sort to correct a mistake here.
#[must_use]
pub fn rank_entries(
    mut entries: Vec<UsageEntry>,
    opts: RankOptions,
    now_unix: i64,
) -> Vec<UsageEntry> {
    match opts.mode {
        RankMode::Recency => {
            entries.sort_by(|a, b| b.last_ordinal.cmp(&a.last_ordinal));
        }
        RankMode::Frecency => {
            // Descending score; `last_ordinal` breaks ties so equal-score
            // entries stay recency-ordered rather than arbitrary.
            entries.sort_by(|a, b| {
                b.frecency(now_unix)
                    .total_cmp(&a.frecency(now_unix))
                    .then_with(|| b.last_ordinal.cmp(&a.last_ordinal))
            });
        }
    }

    if opts.pin_most_recent
        && let Some(newest) = entries
            .iter()
            .enumerate()
            .max_by_key(|(_, e)| e.last_ordinal)
            .map(|(i, _)| i)
        && newest != 0
    {
        let entry = entries.remove(newest);
        entries.insert(0, entry);
    }

    entries
}

/// [`rank_entries`], reduced to the command strings the picker is fed.
#[must_use]
pub fn rank(entries: Vec<UsageEntry>, opts: RankOptions, now_unix: i64) -> Vec<String> {
    rank_entries(entries, opts, now_unix)
        .into_iter()
        .map(|e| e.command)
        .collect()
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
        assert_eq!(entries.len(), 2, "still deduplicated into distinct commands");
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
        let thrice =
            accumulate([": 999913600:0;cmd", ": 999913600:0;cmd", ": 999913600:0;cmd"])[0]
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
        let mut lines: Vec<String> = (0..40)
            .map(|_| format!(": {old}:0;git status"))
            .collect();
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

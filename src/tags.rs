//! Tag-priority ranking — declare what matters, it floats to the top.
//!
//! Both of skim-tab's pickers end the same way: build a `Vec`, hand it to
//! skim with `no_sort(true)`, and let **feed order be display order**.
//! Ctrl-R's feed is history most-recent-first; Tab's is the candidate list
//! after the optional frecency pass. Neither had any way to say "I care
//! about git more than I care about what I typed 30 seconds ago".
//!
//! This module is that say-so, as ONE primitive both seams call rather than
//! two rankers that drift. A tag is a named bucket with a weight; an entry
//! lands in the first bucket whose pattern matches; the list is then sorted
//! by weight, descending, **stably** — so recency (history) and frecency
//! (completions) survive untouched *inside* each tier. Tags decide the
//! tiers; whatever ranked the list before still decides the order within
//! one.
//!
//! ## Why first-match-wins and not sum-of-weights
//!
//! Summing reads as more expressive and is worse to live with: two
//! unrelated rules silently compose into a third ranking nobody declared,
//! and "why is this here" stops having a short answer. First-match-in-
//! declaration-order makes the config file itself the priority list —
//! move a rule up, it wins — and every placement has exactly one cause,
//! which [`TagSet::explain`] can name. Predictability beats expressiveness
//! for a thing that reorders what you see under your fingers.
//!
//! ## Patterns
//!
//! Three matchers, no regex — deliberately. A history feed is re-ranked on
//! every Ctrl-R and the matcher runs over every entry, so the cost has to
//! be a substring scan, and the failure mode of a mistyped regex (silently
//! matching everything, or nothing) is exactly the kind of thing you cannot
//! see happening in a picker.
//!
//! * `command` — the first whitespace-separated word, matched exactly.
//!   `git` catches `git status` but never `legit`.
//! * `prefix`  — the line starts with this string. For families a bare
//!   command can't express: `nix-` catches `nix-shell`/`nix-build`.
//! * `contains` — anywhere in the line. The escape hatch, and the one to
//!   reach for last.
//!
//! A rule with no patterns matches nothing (and is reported by
//! [`TagSet::lint`]) — an empty rule silently swallowing the whole feed
//! would be a spectacular way to break Ctrl-R.
//!
//! ## Default is identity
//!
//! `rules: []` ships by default and [`TagSet::rank`] returns immediately,
//! leaving the feed byte-identical to the untagged build. Nothing about
//! your Ctrl-R changes until you declare a rule.

use serde::{Deserialize, Serialize};

// ── Config surface ──────────────────────────────────────────────────

/// One declared tag: a name, a weight, and the patterns that match it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TagRule {
    /// Tag name. Free-form; surfaced by [`TagSet::explain`] so a ranking
    /// can be traced back to the rule that caused it.
    pub tag: String,

    /// Sort key. Higher floats up; **negative sinks**, which is how you
    /// bury `rm -rf` or a noisy one-off without deleting it from history.
    /// Untagged entries sit at 0, so a positive weight promotes above
    /// everything untagged and a negative one demotes below it.
    pub weight: i64,

    /// First word of the line, matched exactly.
    pub command: Vec<String>,

    /// The line starts with this string.
    pub prefix: Vec<String>,

    /// The string appears anywhere in the line.
    pub contains: Vec<String>,
}

impl Default for TagRule {
    fn default() -> Self {
        Self {
            tag: String::new(),
            weight: 0,
            command: Vec::new(),
            prefix: Vec::new(),
            contains: Vec::new(),
        }
    }
}

impl TagRule {
    /// Does this rule claim `line`?
    ///
    /// Cheapest matcher first: `command` is a split-once plus an equality
    /// test, `prefix` is a bounded compare, `contains` is the full scan.
    #[must_use]
    pub fn matches(&self, line: &str) -> bool {
        if !self.command.is_empty() {
            let first = first_word(line);
            if self.command.iter().any(|c| c == first) {
                return true;
            }
        }
        if self
            .prefix
            .iter()
            .any(|p| !p.is_empty() && line.starts_with(p.as_str()))
        {
            return true;
        }
        if self
            .contains
            .iter()
            .any(|c| !c.is_empty() && line.contains(c.as_str()))
        {
            return true;
        }
        false
    }

    /// A rule that can never match — no patterns at all. Kept as its own
    /// predicate so [`TagSet::lint`] can report it rather than having it
    /// sit in a config file looking effective.
    #[must_use]
    pub fn is_inert(&self) -> bool {
        self.command.is_empty() && self.prefix.is_empty() && self.contains.is_empty()
    }
}

/// Which pickers the tag rules apply to.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TagApplyTo {
    /// Ctrl-R history picker.
    pub history: bool,
    /// Tab completion picker (applied AFTER the frecency pass, so frecency
    /// keeps deciding order inside a tier).
    pub completions: bool,
}

impl Default for TagApplyTo {
    fn default() -> Self {
        Self {
            history: true,
            completions: true,
        }
    }
}

/// The declared tag set.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TagsConfig {
    /// Master switch. On by default — with no rules that is already
    /// identity, so a second opt-in would only be a way to be surprised
    /// that your rules do nothing.
    pub enable: bool,

    /// Rules in priority order. **Order is the semantics**: the first rule
    /// that matches an entry wins.
    pub rules: Vec<TagRule>,

    /// Which pickers to re-rank.
    pub apply_to: TagApplyTo,
}

impl Default for TagsConfig {
    /// `enable: true` with `rules: []` — already identity, so this is not
    /// a behaviour change on an unconfigured machine. Deriving `Default`
    /// here would give `enable: false` and make every new rule a
    /// two-line change for no safety gained.
    fn default() -> Self {
        Self {
            enable: true,
            rules: Vec::new(),
            apply_to: TagApplyTo::default(),
        }
    }
}

// ── The ranker ──────────────────────────────────────────────────────

/// A compiled, ready-to-apply tag set.
#[derive(Debug, Clone)]
pub struct TagSet {
    rules: Vec<TagRule>,
    enabled: bool,
    apply_to: TagApplyTo,
}

/// Why one entry ranked where it did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagVerdict {
    /// Matched tag name, or `None` when nothing claimed the line.
    pub tag: Option<String>,
    /// The weight applied. `0` for an unmatched line.
    pub weight: i64,
}

impl TagSet {
    /// Build from config, dropping inert rules — a rule with no patterns
    /// would match nothing anyway, and carrying it would make `rules.len()`
    /// lie about how many are live.
    #[must_use]
    pub fn new(cfg: &TagsConfig) -> Self {
        Self {
            rules: cfg
                .rules
                .iter()
                .filter(|r| !r.is_inert())
                .cloned()
                .collect(),
            enabled: cfg.enable,
            apply_to: cfg.apply_to.clone(),
        }
    }

    /// True when applying this set could reorder anything. Both seams
    /// check it so the no-rules path costs one bool, not a sort.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.enabled && !self.rules.is_empty()
    }

    #[must_use]
    pub fn applies_to_history(&self) -> bool {
        self.is_active() && self.apply_to.history
    }

    #[must_use]
    pub fn applies_to_completions(&self) -> bool {
        self.is_active() && self.apply_to.completions
    }

    /// The first rule claiming `line`, and its weight.
    #[must_use]
    pub fn explain(&self, line: &str) -> TagVerdict {
        for rule in &self.rules {
            if rule.matches(line) {
                return TagVerdict {
                    tag: Some(rule.tag.clone()),
                    weight: rule.weight,
                };
            }
        }
        TagVerdict {
            tag: None,
            weight: 0,
        }
    }

    /// Weight only — the hot path.
    #[must_use]
    pub fn weight_of(&self, line: &str) -> i64 {
        for rule in &self.rules {
            if rule.matches(line) {
                return rule.weight;
            }
        }
        0
    }

    /// Re-rank in place by tag weight, descending.
    ///
    /// **Stable by contract, not by accident.** `sort_by_key` is
    /// guaranteed stable in Rust, and that guarantee is the whole design:
    /// it is what lets history stay most-recent-first and completions stay
    /// frecency-ordered *within* a tier. Swapping this for
    /// `sort_unstable_by_key` would scramble both and the tests below
    /// would catch it.
    pub fn rank<T, F>(&self, items: &mut [T], key: F)
    where
        F: Fn(&T) -> &str,
    {
        if !self.is_active() {
            return;
        }
        items.sort_by_key(|item| std::cmp::Reverse(self.weight_of(key(item))));
    }

    /// Rules that can never fire. Config mistakes should be findable
    /// without reading a picker's mind.
    #[must_use]
    pub fn lint(cfg: &TagsConfig) -> Vec<String> {
        let mut out = Vec::new();
        for (i, rule) in cfg.rules.iter().enumerate() {
            if rule.is_inert() {
                out.push(format!(
                    "rule #{i} (tag {:?}) declares no command/prefix/contains — it can never match",
                    rule.tag
                ));
            }
            if rule.tag.trim().is_empty() {
                out.push(format!(
                    "rule #{i} has an empty tag name — explain output will be blank"
                ));
            }
        }
        out
    }
}

/// First whitespace-separated word, or `""` for a blank line.
fn first_word(line: &str) -> &str {
    line.split_whitespace().next().unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(tag: &str, weight: i64, command: &[&str]) -> TagRule {
        TagRule {
            tag: tag.to_owned(),
            weight,
            command: command.iter().map(|s| (*s).to_owned()).collect(),
            ..TagRule::default()
        }
    }

    fn cfg(rules: Vec<TagRule>) -> TagsConfig {
        TagsConfig {
            enable: true,
            rules,
            apply_to: TagApplyTo::default(),
        }
    }

    // ── The headline ask ────────────────────────────────────────────

    /// "All things involving git should come up first." The literal
    /// request, as a test: git entries float above everything regardless
    /// of how old they are, and the non-git entries keep their own order.
    #[test]
    fn git_floats_to_the_top_and_everything_else_holds_its_place() {
        let set = TagSet::new(&cfg(vec![rule("git", 100, &["git", "gst", "gco"])]));
        // Most-recent-first, as read_history hands it over.
        let mut feed = vec![
            "ls -la",
            "cargo build",
            "gst",
            "cd /tmp",
            "git commit -m wip",
            "echo hi",
        ];
        set.rank(&mut feed, |s| *s);
        assert_eq!(
            feed,
            vec![
                // git tier, in the recency order they arrived in
                "gst",
                "git commit -m wip",
                // untagged tier, likewise undisturbed
                "ls -la",
                "cargo build",
                "cd /tmp",
                "echo hi",
            ]
        );
    }

    /// The property the whole design rests on: ranking must not disturb
    /// order WITHIN a tier, or it silently destroys the recency ordering
    /// skim-history fought a bug to get right (`no_sort(true)` means this
    /// order is what the operator sees).
    #[test]
    fn ranking_is_stable_within_a_tier() {
        let set = TagSet::new(&cfg(vec![rule("git", 10, &["git"])]));
        let mut feed = vec!["git a", "git b", "git c", "ls 1", "ls 2", "ls 3"];
        set.rank(&mut feed, |s| *s);
        assert_eq!(
            feed,
            vec!["git a", "git b", "git c", "ls 1", "ls 2", "ls 3"]
        );
    }

    /// Negative weights sink below untagged entries — the "bury it without
    /// deleting it" case.
    #[test]
    fn negative_weight_sinks_below_untagged() {
        let set = TagSet::new(&cfg(vec![TagRule {
            tag: "danger".into(),
            weight: -1000,
            contains: vec!["rm -rf".into()],
            ..TagRule::default()
        }]));
        let mut feed = vec!["rm -rf /tmp/x", "ls", "cargo test"];
        set.rank(&mut feed, |s| *s);
        assert_eq!(feed, vec!["ls", "cargo test", "rm -rf /tmp/x"]);
    }

    /// Declaration order IS priority: the first matching rule wins, so a
    /// line matching two rules is placed by whichever the user listed
    /// first. Moving a rule up in the file moves its entries up.
    #[test]
    fn first_matching_rule_wins_so_config_order_is_the_priority_list() {
        let danger = TagRule {
            tag: "danger".into(),
            weight: -100,
            contains: vec!["--force".into()],
            ..TagRule::default()
        };
        let git = rule("git", 100, &["git"]);

        let danger_first = TagSet::new(&cfg(vec![danger.clone(), git.clone()]));
        assert_eq!(danger_first.explain("git push --force").weight, -100);
        assert_eq!(
            danger_first.explain("git push --force").tag.as_deref(),
            Some("danger")
        );

        let git_first = TagSet::new(&cfg(vec![git, danger]));
        assert_eq!(git_first.explain("git push --force").weight, 100);
        assert_eq!(
            git_first.explain("git push --force").tag.as_deref(),
            Some("git")
        );
    }

    // ── Matchers ────────────────────────────────────────────────────

    /// `command` is the FIRST WORD, exactly — not a substring. `legit` is
    /// not git, and an entry that merely mentions git in an argument is
    /// not a git command.
    #[test]
    fn command_matches_the_first_word_exactly() {
        let set = TagSet::new(&cfg(vec![rule("git", 10, &["git"])]));
        assert_eq!(set.weight_of("git status"), 10);
        assert_eq!(
            set.weight_of("  git status"),
            10,
            "leading space is not a different command"
        );
        assert_eq!(
            set.weight_of("legit --help"),
            0,
            "substring of the command name must not match"
        );
        assert_eq!(
            set.weight_of("echo git"),
            0,
            "git as an argument is not a git command"
        );
        assert_eq!(set.weight_of("gitk"), 0);
    }

    #[test]
    fn prefix_and_contains_match_where_command_cannot() {
        let set = TagSet::new(&cfg(vec![
            TagRule {
                tag: "nix".into(),
                weight: 50,
                prefix: vec!["nix-".into()],
                ..TagRule::default()
            },
            TagRule {
                tag: "kubeconfig".into(),
                weight: 20,
                contains: vec!["--kubeconfig".into()],
                ..TagRule::default()
            },
        ]));
        assert_eq!(set.weight_of("nix-shell -p jq"), 50);
        assert_eq!(set.weight_of("kubectl get po --kubeconfig /tmp/kc"), 20);
        assert_eq!(set.weight_of("kubectl get po"), 0);
    }

    // ── Identity + safety ───────────────────────────────────────────

    /// The untagged build must be byte-identical. No rules ⇒ no reorder,
    /// and `is_active` is false so neither seam pays for a sort.
    #[test]
    fn no_rules_is_exactly_identity() {
        let set = TagSet::new(&TagsConfig::default());
        assert!(!set.is_active());
        let original = vec!["c", "a", "b"];
        let mut feed = original.clone();
        set.rank(&mut feed, |s| *s);
        assert_eq!(feed, original);
    }

    /// Disabled with rules present is also identity — the master switch
    /// has to actually switch.
    #[test]
    fn disabled_is_identity_even_with_rules() {
        let mut c = cfg(vec![rule("git", 100, &["git"])]);
        c.enable = false;
        let set = TagSet::new(&c);
        assert!(!set.is_active());
        let mut feed = vec!["ls", "git status"];
        set.rank(&mut feed, |s| *s);
        assert_eq!(feed, vec!["ls", "git status"]);
    }

    /// A rule with no patterns is dropped at construction rather than
    /// left to match nothing — otherwise `is_active` would claim a live
    /// rule set that cannot reorder anything.
    #[test]
    fn inert_rules_are_dropped_not_carried() {
        let inert = TagRule {
            tag: "typo".into(),
            weight: 999,
            ..TagRule::default()
        };
        let set = TagSet::new(&cfg(vec![inert]));
        assert!(
            !set.is_active(),
            "a rule set of only inert rules is not active"
        );
        assert_eq!(set.weight_of("anything"), 0);
    }

    #[test]
    fn lint_names_the_config_mistakes() {
        let c = cfg(vec![
            TagRule {
                tag: "typo".into(),
                weight: 5,
                ..TagRule::default()
            },
            TagRule {
                tag: String::new(),
                weight: 5,
                command: vec!["ls".into()],
                ..TagRule::default()
            },
        ]);
        let found = TagSet::lint(&c);
        assert_eq!(found.len(), 2, "both mistakes reported: {found:?}");
        assert!(found[0].contains("can never match"));
        assert!(found[1].contains("empty tag name"));
    }

    #[test]
    fn empty_pattern_strings_never_match_everything() {
        // An empty prefix would `starts_with("")` == true on every line —
        // a config typo that would silently tag the entire feed.
        let set = TagSet::new(&cfg(vec![TagRule {
            tag: "oops".into(),
            weight: 999,
            prefix: vec![String::new()],
            contains: vec![String::new()],
            ..TagRule::default()
        }]));
        assert_eq!(set.weight_of("literally anything"), 0);
    }

    #[test]
    fn blank_and_unicode_lines_do_not_panic() {
        let set = TagSet::new(&cfg(vec![rule("git", 10, &["git"])]));
        assert_eq!(set.weight_of(""), 0);
        assert_eq!(set.weight_of("   "), 0);
        assert_eq!(set.weight_of("echo 日本語 ❄"), 0);
        assert_eq!(set.weight_of("git commit -m 'ünïcödé ❄'"), 10);
    }

    /// Tiers order by weight, not by declaration order — declaration order
    /// only breaks ties in MATCHING, not in ranking.
    #[test]
    fn higher_weight_outranks_earlier_declaration() {
        let set = TagSet::new(&cfg(vec![
            rule("low", 1, &["a"]),
            rule("high", 100, &["b"]),
        ]));
        let mut feed = vec!["a x", "b y"];
        set.rank(&mut feed, |s| *s);
        assert_eq!(feed, vec!["b y", "a x"], "weight decides tiers");
    }

    /// `rank` is generic over the item type so both seams share it — the
    /// completion path ranks structs by a field, not bare strings.
    #[test]
    fn rank_works_over_structs_via_the_key_fn() {
        struct Candidate {
            word: String,
        }
        let set = TagSet::new(&cfg(vec![rule("git", 10, &["git"])]));
        let mut cands = vec![
            Candidate { word: "ls".into() },
            Candidate {
                word: "git status".into(),
            },
        ];
        set.rank(&mut cands, |c| c.word.as_str());
        assert_eq!(cands[0].word, "git status");
    }
}

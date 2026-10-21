//! Frecency-parity property test (L3 brick) — skim-tab vs wadachi-spec.
//!
//! The fleet invariant: skim-tab's ranking == the wadachi-spec interpreter's
//! ranking, ∀ visit histories. skim-tab's `frecency_score` delegates the decay
//! to `wadachi_spec::DecayKind::HyperbolicDays`, and `FrecencyRankingSpec::
//! skimtab_parity()` (`Σ 1/(1+age_days)`, recency-only) is the named spec
//! instance that mirrors it — this test pins the two together so they can
//! never drift, even if either side's internals are refactored.
//!
//! Two assertions per generated history set:
//! 1. **Score parity** — for every entry, the score skim-tab computes
//!    (Σ `frecency_score(age_days, 1)` over occurrences, exactly the
//!    accumulation `HistoryDb::frecency_scores` performs per word) equals the
//!    score the spec interpreter computes for the same visit history, within
//!    1e-9. Per-entry scores come from the interpreter via a singleton
//!    `apply` so the spec's own `TopK` phase cannot hide any entry.
//! 2. **Ranking parity** — the full ranking the interpreter emits (stable
//!    sort descending + `TopK`) equals the ranking induced by skim-tab's
//!    scores under the same tie-break rule (stable sort preserves insertion
//!    order on ties), truncated to the spec's declared `TopK`.
//!
//! Ages are generated in whole **seconds** because that is the granularity
//! the interpreter derives ages at (`(now - then).num_seconds() / 86 400`);
//! feeding skim-tab's path the identically-derived `age_days` keeps the
//! comparison about the *formula*, not about timestamp quantization.
//!
//! Generator choice: proptest (new dev-dep) over a hand-rolled exhaustive
//! generator — it is the fleet's Pillar-10 property-testing idiom, ships
//! shrinking on failure, and the input space (200 entries × 50 visits ×
//! 3650 days) is far too large to enumerate exhaustively anyway.

use std::cmp::Ordering;
use std::path::PathBuf;

use chrono::{Duration, NaiveDate, NaiveDateTime};
use proptest::prelude::*;
use skim_tab::history_db::frecency_score;
use wadachi_spec::{DirEntry, FrecencyRankingSpec, MockEnvironment, RankPhase, apply};

/// 3650 days in seconds — the upper bound on a visit's age.
const MAX_AGE_SECS: i64 = 3650 * 86_400;

/// A frozen "now" so the whole comparison is deterministic. The exact value
/// is irrelevant — only visit *ages* enter the formula.
fn fixed_now() -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 6, 10)
        .unwrap()
        .and_hms_opt(12, 0, 0)
        .unwrap()
}

/// skim-tab's side of the invariant: the per-word accumulation
/// `HistoryDb::frecency_scores` performs — one `frecency_score` contribution
/// per occurrence, summed in occurrence order.
fn skim_tab_score(visit_ages_secs: &[i64]) -> f64 {
    visit_ages_secs
        .iter()
        .map(|secs| frecency_score(*secs as f64 / 86_400.0, 1))
        .sum()
}

/// Build the wadachi-spec `DirEntry` for the same history: visit timestamps
/// at exactly the generated ages before the frozen now.
fn spec_entry(index: usize, visit_ages_secs: &[i64], now: NaiveDateTime) -> DirEntry {
    DirEntry {
        path: PathBuf::from(format!("/entry/{index}")),
        visits: visit_ages_secs
            .iter()
            .map(|secs| now - Duration::seconds(*secs))
            .collect(),
        discovered_only: false,
    }
}

/// The spec's declared `TopK` bound — read from the spec itself so the test
/// tracks the spec-of-record, never a hardcoded copy.
fn top_k(spec: &FrecencyRankingSpec) -> usize {
    spec.phases
        .iter()
        .find_map(|p| match p {
            RankPhase::TopK { n } => Some(*n),
            _ => None,
        })
        .expect("skimtab_parity spec declares a TopK phase")
}

/// One visit history: 1..=50 occurrences ("counts"), each aged 0..=3650 days
/// (expressed in whole seconds — see module docs).
fn arb_history() -> impl Strategy<Value = Vec<i64>> {
    prop::collection::vec(0..=MAX_AGE_SECS, 1..=50)
}

proptest! {
    /// ∀ visit-history sets: skim-tab's frecency_score path and the
    /// wadachi-spec interpreter agree on every score (within 1e-9) and induce
    /// the identical ranking (ties broken consistently by stable sort).
    #[test]
    fn skim_tab_matches_wadachi_spec(histories in prop::collection::vec(arb_history(), 1..=200)) {
        let now = fixed_now();
        let env = MockEnvironment::at(now);
        let spec = FrecencyRankingSpec::skimtab_parity();

        // ── 1. Score parity, per entry ───────────────────────────────
        let mut skim_scores: Vec<f64> = Vec::with_capacity(histories.len());
        for (i, ages) in histories.iter().enumerate() {
            let skim = skim_tab_score(ages);
            let ranked = apply(&spec, vec![spec_entry(i, ages, now)], &env)
                .expect("singleton apply must succeed");
            prop_assert_eq!(ranked.len(), 1);
            prop_assert!(
                (ranked[0].score - skim).abs() < 1e-9,
                "score drift on entry {}: skim-tab={} wadachi-spec={}",
                i, skim, ranked[0].score
            );
            skim_scores.push(skim);
        }

        // ── 2. Ranking parity, whole set ─────────────────────────────
        let entries: Vec<DirEntry> = histories
            .iter()
            .enumerate()
            .map(|(i, ages)| spec_entry(i, ages, now))
            .collect();
        let spec_ranked = apply(&spec, entries, &env).expect("apply must succeed");

        // skim-tab's induced ranking: stable sort descending by score —
        // the same tie-break the interpreter's SortDesc uses (Rust's
        // sort_by is stable, so equal scores keep insertion order) —
        // truncated to the spec's declared TopK.
        let mut skim_ranked: Vec<(PathBuf, f64)> = skim_scores
            .iter()
            .enumerate()
            .map(|(i, score)| (PathBuf::from(format!("/entry/{i}")), *score))
            .collect();
        skim_ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(Ordering::Equal));
        skim_ranked.truncate(top_k(&spec));

        prop_assert_eq!(
            spec_ranked.len(),
            skim_ranked.len(),
            "ranking lengths diverge"
        );
        for (rank, (got, want)) in spec_ranked.iter().zip(skim_ranked.iter()).enumerate() {
            prop_assert_eq!(
                &got.path, &want.0,
                "ranking drift at position {}: wadachi-spec={:?} skim-tab={:?}",
                rank, got.path, want.0
            );
            prop_assert!(
                (got.score - want.1).abs() < 1e-9,
                "ranked-score drift at position {}: wadachi-spec={} skim-tab={}",
                rank, got.score, want.1
            );
        }
    }
}

// ── Deterministic anchors ────────────────────────────────────────────
// Fixed boundary cases that must hold regardless of proptest's sampling —
// cheap regression pins for the edges of the generated domain.

/// Age 0 → both sides score exactly 1.0 per visit.
#[test]
fn parity_at_zero_age() {
    let now = fixed_now();
    let env = MockEnvironment::at(now);
    let spec = FrecencyRankingSpec::skimtab_parity();
    let ages = vec![0_i64; 3];
    let ranked = apply(&spec, vec![spec_entry(0, &ages, now)], &env).unwrap();
    let skim = skim_tab_score(&ages);
    assert!(
        (skim - 3.0).abs() < 1e-9,
        "3 visits at age 0 sum to 3.0: {skim}"
    );
    assert!((ranked[0].score - skim).abs() < 1e-9);
}

/// Age 3650 days (the generator's max) — the deep-tail of the decay.
#[test]
fn parity_at_max_age() {
    let now = fixed_now();
    let env = MockEnvironment::at(now);
    let spec = FrecencyRankingSpec::skimtab_parity();
    let ages = vec![MAX_AGE_SECS];
    let ranked = apply(&spec, vec![spec_entry(0, &ages, now)], &env).unwrap();
    let skim = skim_tab_score(&ages);
    assert!((ranked[0].score - skim).abs() < 1e-9);
    assert!(
        skim > 0.0 && skim < 0.001,
        "10-year-old visit is near zero: {skim}"
    );
}

/// Exact ties (identical histories) keep insertion order on both sides.
#[test]
fn parity_breaks_ties_by_insertion_order() {
    let now = fixed_now();
    let env = MockEnvironment::at(now);
    let spec = FrecencyRankingSpec::skimtab_parity();
    let ages = vec![86_400_i64, 7 * 86_400]; // same history for every entry
    let entries: Vec<DirEntry> = (0..4).map(|i| spec_entry(i, &ages, now)).collect();
    let ranked = apply(&spec, entries, &env).unwrap();
    let paths: Vec<&str> = ranked.iter().map(|r| r.path.to_str().unwrap()).collect();
    // Stable SortDesc on equal scores == insertion order; skim-tab's induced
    // ranking uses the identical stable rule, so parity demands this order.
    assert_eq!(paths, vec!["/entry/0", "/entry/1", "/entry/2", "/entry/3"]);
}

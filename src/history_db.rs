//! Selection history — SQLite-backed frecency ranking for completions.
//!
//! Records (command, cwd, selected_word, timestamp) on each selection.
//! Queries frecency scores to boost frequently/recently selected candidates.

use std::collections::HashMap;
use std::path::PathBuf;

use rusqlite::{params, Connection};

// ── Frecency scoring ─────────────────────────────────────────────────

/// Compute a single frecency contribution from one historical event.
///
/// The formula is: `1.0 / (1.0 + age_days)` where `age_days` is clamped
/// to non-negative. Recent events (age near 0) contribute close to 1.0;
/// older events decay hyperbolically.
///
/// For an entry with multiple occurrences, sum the per-occurrence scores.
#[must_use]
pub fn frecency_score(age_days: f64, _frequency: u32) -> f64 {
    1.0 / (1.0 + age_days.max(0.0))
}

// ── HistoryStore trait ───────────────────────────────────────────────

/// Abstraction over selection history storage for testability.
pub trait HistoryStore {
    /// Record a selection event for the given command, cwd, and word.
    fn record(&self, command: &str, cwd: &str, word: &str) -> anyhow::Result<()>;

    /// Query frecency scores for all words previously selected for the
    /// given command + cwd combination. Returns a map of word to score.
    fn frecency_scores(
        &self,
        command: &str,
        cwd: &str,
    ) -> anyhow::Result<HashMap<String, f64>>;
}

// ── SQLite-backed store ──────────────────────────────────────────────

/// SQLite-backed selection history for frecency ranking.
pub struct HistoryDb {
    conn: Connection,
}

impl HistoryDb {
    /// Open (or create) the selection history database.
    ///
    /// Default path: `~/.local/share/skim-tab/selections.db`
    pub fn open() -> Result<Self, rusqlite::Error> {
        let path = Self::db_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = Connection::open(&path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS selections (
                id        INTEGER PRIMARY KEY AUTOINCREMENT,
                command   TEXT NOT NULL,
                cwd       TEXT NOT NULL,
                word      TEXT NOT NULL,
                timestamp INTEGER NOT NULL DEFAULT (unixepoch())
            );
            CREATE INDEX IF NOT EXISTS idx_selections_cmd_cwd
                ON selections (command, cwd);
            CREATE INDEX IF NOT EXISTS idx_selections_timestamp
                ON selections (timestamp);",
        )?;
        Ok(Self { conn })
    }

    /// Record a selection event.
    pub fn record(&self, command: &str, cwd: &str, word: &str) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO selections (command, cwd, word) VALUES (?1, ?2, ?3)",
            params![command, cwd, word],
        )?;
        Ok(())
    }

    /// Query frecency scores for all words previously selected for the given
    /// command + cwd combination.
    ///
    /// Frecency formula: `score = sum(1.0 / (1.0 + days_since_selection))`
    /// for each occurrence. Recent selections contribute more.
    pub fn frecency_scores(
        &self,
        command: &str,
        cwd: &str,
    ) -> Result<HashMap<String, f64>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT word, (julianday('now') - julianday(timestamp, 'unixepoch')) AS days_ago
             FROM selections
             WHERE command = ?1 AND cwd = ?2",
        )?;

        let mut scores: HashMap<String, f64> = HashMap::new();
        let rows = stmt.query_map(params![command, cwd], |row| {
            let word: String = row.get(0)?;
            let days_ago: f64 = row.get(1)?;
            Ok((word, days_ago))
        })?;

        for row in rows {
            let (word, days_ago) = row?;
            let contribution = frecency_score(days_ago, 1);
            *scores.entry(word).or_insert(0.0) += contribution;
        }

        Ok(scores)
    }

    /// Delete entries older than `max_age_days`.
    pub fn cleanup(&self, max_age_days: u32) -> Result<usize, rusqlite::Error> {
        let deleted = self.conn.execute(
            "DELETE FROM selections WHERE timestamp < unixepoch() - ?1 * 86400",
            params![max_age_days],
        )?;
        Ok(deleted)
    }

    /// Resolve the database path.
    fn db_path() -> PathBuf {
        if let Ok(data_dir) = std::env::var("XDG_DATA_HOME") {
            PathBuf::from(data_dir)
                .join("skim-tab")
                .join("selections.db")
        } else if let Ok(home) = std::env::var("HOME") {
            PathBuf::from(home)
                .join(".local/share/skim-tab")
                .join("selections.db")
        } else {
            PathBuf::from("/tmp/skim-tab-selections.db")
        }
    }
}

impl HistoryStore for HistoryDb {
    fn record(&self, command: &str, cwd: &str, word: &str) -> anyhow::Result<()> {
        self.record(command, cwd, word)?;
        Ok(())
    }

    fn frecency_scores(
        &self,
        command: &str,
        cwd: &str,
    ) -> anyhow::Result<HashMap<String, f64>> {
        Ok(self.frecency_scores(command, cwd)?)
    }
}

// ── In-memory store (test only) ─────────────────────────────────────

#[cfg(test)]
pub struct MemHistoryStore {
    entries: std::sync::Mutex<Vec<(String, String, String, std::time::Instant)>>,
}

#[cfg(test)]
impl MemHistoryStore {
    pub fn new() -> Self {
        Self {
            entries: std::sync::Mutex::new(Vec::new()),
        }
    }
}

#[cfg(test)]
impl HistoryStore for MemHistoryStore {
    fn record(&self, command: &str, cwd: &str, word: &str) -> anyhow::Result<()> {
        let mut entries = self.entries.lock().unwrap();
        entries.push((
            command.to_string(),
            cwd.to_string(),
            word.to_string(),
            std::time::Instant::now(),
        ));
        Ok(())
    }

    fn frecency_scores(
        &self,
        command: &str,
        cwd: &str,
    ) -> anyhow::Result<HashMap<String, f64>> {
        let entries = self.entries.lock().unwrap();
        let now = std::time::Instant::now();
        let mut scores: HashMap<String, f64> = HashMap::new();
        for (cmd, dir, word, ts) in entries.iter() {
            if cmd == command && dir == cwd {
                let age_secs = now.duration_since(*ts).as_secs_f64();
                let age_days = age_secs / 86400.0;
                let contribution = frecency_score(age_days, 1);
                *scores.entry(word.clone()).or_insert(0.0) += contribution;
            }
        }
        Ok(scores)
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Create an in-memory database for testing.
    fn test_db() -> HistoryDb {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS selections (
                id        INTEGER PRIMARY KEY AUTOINCREMENT,
                command   TEXT NOT NULL,
                cwd       TEXT NOT NULL,
                word      TEXT NOT NULL,
                timestamp INTEGER NOT NULL DEFAULT (unixepoch())
            );
            CREATE INDEX IF NOT EXISTS idx_selections_cmd_cwd
                ON selections (command, cwd);
            CREATE INDEX IF NOT EXISTS idx_selections_timestamp
                ON selections (timestamp);",
        )
        .unwrap();
        HistoryDb { conn }
    }

    #[test]
    fn record_and_query() {
        let db = test_db();
        db.record("kubectl", "/code/k8s", "pods").unwrap();
        db.record("kubectl", "/code/k8s", "pods").unwrap();
        db.record("kubectl", "/code/k8s", "services").unwrap();

        let scores = db.frecency_scores("kubectl", "/code/k8s").unwrap();
        assert!(scores.contains_key("pods"));
        assert!(scores.contains_key("services"));
        // pods was selected twice, so its score should be higher
        assert!(scores["pods"] > scores["services"]);
    }

    #[test]
    fn frecency_scores_empty() {
        let db = test_db();
        let scores = db.frecency_scores("cd", "/tmp").unwrap();
        assert!(scores.is_empty());
    }

    #[test]
    fn different_command_different_scores() {
        let db = test_db();
        db.record("kubectl", "/code/k8s", "pods").unwrap();
        db.record("helm", "/code/k8s", "install").unwrap();

        let kubectl_scores = db.frecency_scores("kubectl", "/code/k8s").unwrap();
        let helm_scores = db.frecency_scores("helm", "/code/k8s").unwrap();

        assert!(kubectl_scores.contains_key("pods"));
        assert!(!kubectl_scores.contains_key("install"));
        assert!(helm_scores.contains_key("install"));
        assert!(!helm_scores.contains_key("pods"));
    }

    #[test]
    fn different_cwd_different_scores() {
        let db = test_db();
        db.record("cd", "/code/nix", "modules").unwrap();
        db.record("cd", "/code/k8s", "shared").unwrap();

        let nix_scores = db.frecency_scores("cd", "/code/nix").unwrap();
        let k8s_scores = db.frecency_scores("cd", "/code/k8s").unwrap();

        assert!(nix_scores.contains_key("modules"));
        assert!(!nix_scores.contains_key("shared"));
        assert!(k8s_scores.contains_key("shared"));
        assert!(!k8s_scores.contains_key("modules"));
    }

    #[test]
    fn cleanup_removes_old_entries() {
        let db = test_db();
        // Insert an entry with a very old timestamp
        db.conn
            .execute(
                "INSERT INTO selections (command, cwd, word, timestamp)
                 VALUES ('cd', '/tmp', 'old', unixepoch() - 400 * 86400)",
                [],
            )
            .unwrap();
        // Insert a recent entry
        db.record("cd", "/tmp", "recent").unwrap();

        let deleted = db.cleanup(365).unwrap();
        assert_eq!(deleted, 1);

        let scores = db.frecency_scores("cd", "/tmp").unwrap();
        assert!(!scores.contains_key("old"));
        assert!(scores.contains_key("recent"));
    }

    #[test]
    fn cleanup_zero_when_nothing_old() {
        let db = test_db();
        db.record("cd", "/tmp", "recent").unwrap();
        let deleted = db.cleanup(365).unwrap();
        assert_eq!(deleted, 0);
    }

    #[test]
    fn db_path_uses_xdg() {
        // Just verify the function doesn't panic
        let path = HistoryDb::db_path();
        assert!(path.to_str().unwrap().contains("skim-tab"));
    }

    #[test]
    fn recent_selections_score_higher() {
        let db = test_db();
        // Insert an old selection
        db.conn
            .execute(
                "INSERT INTO selections (command, cwd, word, timestamp)
                 VALUES ('cd', '/tmp', 'old_dir', unixepoch() - 30 * 86400)",
                [],
            )
            .unwrap();
        // Insert a recent selection
        db.record("cd", "/tmp", "new_dir").unwrap();

        let scores = db.frecency_scores("cd", "/tmp").unwrap();
        assert!(scores["new_dir"] > scores["old_dir"]);
    }

    // ── New frecency_score function tests ────────────────────────────

    #[test]
    fn frecency_score_recent_higher() {
        let recent = frecency_score(0.1, 1);
        let old = frecency_score(30.0, 1);
        assert!(
            recent > old,
            "recent score ({recent}) should be higher than old score ({old})"
        );
    }

    #[test]
    fn frecency_score_zero_age() {
        let score = frecency_score(0.0, 1);
        assert!(
            (score - 1.0).abs() < f64::EPSILON,
            "score at age 0 should be 1.0, got {score}"
        );
    }

    #[test]
    fn frecency_score_negative_age_clamped() {
        // Negative age should be clamped to 0, producing a score of 1.0
        let score = frecency_score(-5.0, 1);
        assert!(
            (score - 1.0).abs() < f64::EPSILON,
            "negative age should clamp to 0 => score 1.0, got {score}"
        );
    }

    // ── MemHistoryStore tests ────────────────────────────────────────

    #[test]
    fn mem_history_store_record_and_query() {
        let store = MemHistoryStore::new();
        HistoryStore::record(&store, "kubectl", "/code/k8s", "pods").unwrap();
        HistoryStore::record(&store, "kubectl", "/code/k8s", "pods").unwrap();
        HistoryStore::record(&store, "kubectl", "/code/k8s", "services").unwrap();

        let scores = HistoryStore::frecency_scores(&store, "kubectl", "/code/k8s").unwrap();
        assert!(scores.contains_key("pods"), "should contain 'pods'");
        assert!(scores.contains_key("services"), "should contain 'services'");
        // pods was recorded twice, so its score should be higher
        assert!(
            scores["pods"] > scores["services"],
            "pods ({}) should score higher than services ({})",
            scores["pods"],
            scores["services"]
        );
    }

    #[test]
    fn mem_history_store_isolates_commands() {
        let store = MemHistoryStore::new();
        HistoryStore::record(&store, "kubectl", "/code", "pods").unwrap();
        HistoryStore::record(&store, "helm", "/code", "install").unwrap();

        let kubectl_scores = HistoryStore::frecency_scores(&store, "kubectl", "/code").unwrap();
        let helm_scores = HistoryStore::frecency_scores(&store, "helm", "/code").unwrap();

        assert!(kubectl_scores.contains_key("pods"));
        assert!(!kubectl_scores.contains_key("install"));
        assert!(helm_scores.contains_key("install"));
        assert!(!helm_scores.contains_key("pods"));
    }

    #[test]
    fn mem_history_store_empty_query() {
        let store = MemHistoryStore::new();
        let scores = HistoryStore::frecency_scores(&store, "cd", "/tmp").unwrap();
        assert!(scores.is_empty());
    }

    // ── frecency_score additional cases ──────────────────────────────

    #[test]
    fn frecency_score_large_age() {
        let score = frecency_score(365.0, 1);
        assert!(score > 0.0);
        assert!(score < 0.01, "very old entries should have near-zero score: {score}");
    }

    #[test]
    fn frecency_score_one_day() {
        let score = frecency_score(1.0, 1);
        assert!((score - 0.5).abs() < f64::EPSILON, "1-day-old score should be 0.5, got {score}");
    }

    #[test]
    fn frecency_score_half_day() {
        let score = frecency_score(0.5, 1);
        let expected = 1.0 / 1.5;
        assert!(
            (score - expected).abs() < f64::EPSILON,
            "half-day score should be {expected}, got {score}"
        );
    }

    // ── HistoryDb multiple records same word ─────────────────────────

    #[test]
    fn multiple_records_accumulate_score() {
        let db = test_db();
        db.record("cd", "/tmp", "docs").unwrap();
        db.record("cd", "/tmp", "docs").unwrap();
        db.record("cd", "/tmp", "docs").unwrap();
        let scores = db.frecency_scores("cd", "/tmp").unwrap();
        let score = scores["docs"];
        // 3 records at age ~0 each contribute ~1.0 → sum ~3.0
        assert!(score > 2.5 && score <= 3.0, "expected ~3.0, got {score}");
    }

    // ── HistoryDb cleanup with boundary ──────────────────────────────

    #[test]
    fn cleanup_with_zero_days() {
        let db = test_db();
        // Insert an entry 1 second in the past to ensure it's < unixepoch()
        db.conn
            .execute(
                "INSERT INTO selections (command, cwd, word, timestamp)
                 VALUES ('cd', '/tmp', 'old', unixepoch() - 1)",
                [],
            )
            .unwrap();
        let deleted = db.cleanup(0).unwrap();
        assert_eq!(deleted, 1);
    }

    // ── db_path resolution ───────────────────────────────────────────

    #[test]
    fn db_path_with_xdg_data_home() {
        unsafe { std::env::set_var("XDG_DATA_HOME", "/tmp/xdg-test") };
        let path = HistoryDb::db_path();
        assert!(path.starts_with("/tmp/xdg-test/skim-tab"));
        assert!(path.ends_with("selections.db"));
        unsafe { std::env::remove_var("XDG_DATA_HOME") };
    }

    #[test]
    fn db_path_without_xdg_uses_home() {
        unsafe { std::env::remove_var("XDG_DATA_HOME") };
        let path = HistoryDb::db_path();
        let path_str = path.to_str().unwrap();
        assert!(path_str.contains("skim-tab") && path_str.contains("selections.db"));
    }

    // ── MemHistoryStore isolation ─────────────────────────────────────

    #[test]
    fn mem_history_store_different_cwd() {
        let store = MemHistoryStore::new();
        HistoryStore::record(&store, "cd", "/dir/a", "docs").unwrap();
        HistoryStore::record(&store, "cd", "/dir/b", "src").unwrap();

        let a_scores = HistoryStore::frecency_scores(&store, "cd", "/dir/a").unwrap();
        let b_scores = HistoryStore::frecency_scores(&store, "cd", "/dir/b").unwrap();

        assert!(a_scores.contains_key("docs"));
        assert!(!a_scores.contains_key("src"));
        assert!(b_scores.contains_key("src"));
        assert!(!b_scores.contains_key("docs"));
    }

    // ── HistoryStore trait through HistoryDb ─────────────────────────

    #[test]
    fn history_db_as_trait_object() {
        let db = test_db();
        let store: &dyn HistoryStore = &db;
        store.record("git", "/repo", "commit").unwrap();
        let scores = store.frecency_scores("git", "/repo").unwrap();
        assert!(scores.contains_key("commit"));
    }
}

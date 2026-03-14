//! Selection history — SQLite-backed frecency ranking for completions.
//!
//! Records (command, cwd, selected_word, timestamp) on each selection.
//! Queries frecency scores to boost frequently/recently selected candidates.

use std::collections::HashMap;
use std::path::PathBuf;

use rusqlite::{params, Connection};

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
            let contribution = 1.0 / (1.0 + days_ago.max(0.0));
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
}

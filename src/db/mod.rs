pub mod ai_commits;
pub mod events;
pub mod migrations;
pub mod otel;
pub mod projects;
pub mod sessions;
pub mod stats;
pub mod turns;

use rusqlite::Connection;

use crate::paths;

pub fn collect_rows<T>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>>,
) -> Result<Vec<T>, String> {
    let mut result = Vec::new();
    for r in rows {
        result.push(r.map_err(|e| format!("row error: {e}"))?);
    }
    Ok(result)
}

/// Handle to the oobo local SQLite database.
pub struct Db {
    pub conn: Connection,
}

impl Db {
    /// Open (or create) the database at `~/.oobo/db/oobo.db`.
    pub fn open() -> Result<Self, String> {
        let db_dir = paths::oobo_db_dir();
        paths::ensure_dir(&db_dir)?;

        let db_path = paths::oobo_db_path();
        let conn = Connection::open(&db_path)
            .map_err(|e| format!("cannot open database {}: {e}", db_path.display()))?;

        let db = Self { conn };
        db.init_with_path(Some(&db_path))?;
        Ok(db)
    }

    /// Open an in-memory DB. Useful for tests (including
    /// integration tests outside the crate) and for ephemeral
    /// one-shot tools that don't want to touch the user's DB file.
    pub fn open_in_memory() -> Result<Self, String> {
        let conn =
            Connection::open_in_memory().map_err(|e| format!("cannot open in-memory db: {e}"))?;
        let db = Self { conn };
        db.init_with_path(None)?;
        Ok(db)
    }

    fn init_with_path(&self, db_path: Option<&std::path::Path>) -> Result<(), String> {
        self.conn
            .execute_batch(
                "PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;",
            )
            .map_err(|e| format!("cannot set pragmas: {e}"))?;
        migrations::run_with_path(&self.conn, db_path)?;
        // Migration v9 may have temporarily disabled FKs — restore them.
        self.conn
            .execute_batch("PRAGMA foreign_keys=ON;")
            .map_err(|e| format!("cannot restore fk pragma: {e}"))?;
        Ok(())
    }

    /// Read a value from `oobo_state`. Returns `None` when the key
    /// is absent or the table doesn't exist yet (pre-v12 DBs).
    pub fn state_get(&self, key: &str) -> Option<String> {
        self.conn
            .query_row(
                "SELECT value FROM oobo_state WHERE key = ?1",
                rusqlite::params![key],
                |row| row.get::<_, String>(0),
            )
            .ok()
    }

    /// Set a value in `oobo_state`. Silent no-op if the table is
    /// missing (shouldn't happen once migrations run, but keeps
    /// callers from panicking on partially-initialised DBs).
    pub fn state_set(&self, key: &str, value: &str) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO oobo_state (key, value) VALUES (?1, ?2)",
                rusqlite::params![key, value],
            )
            .map(|_| ())
            .map_err(|e| format!("state_set {key}: {e}"))
    }

    /// Remove a key from `oobo_state`.
    pub fn state_clear(&self, key: &str) -> Result<(), String> {
        self.conn
            .execute(
                "DELETE FROM oobo_state WHERE key = ?1",
                rusqlite::params![key],
            )
            .map(|_| ())
            .map_err(|e| format!("state_clear {key}: {e}"))
    }

    /// Check if this project has been hydrated recently (within `max_age_secs`).
    pub fn needs_hydration(&self, project_root: &str, max_age_secs: i64) -> bool {
        let result: Option<i64> = self
            .conn
            .query_row(
                "SELECT last_hydrated_at FROM hydration_state WHERE project_root = ?1",
                rusqlite::params![project_root],
                |row| row.get(0),
            )
            .ok();
        match result {
            Some(ts) => (chrono::Utc::now().timestamp() - ts) > max_age_secs,
            None => true,
        }
    }

    /// Mark a project as hydrated.
    pub fn mark_hydrated(&self, project_root: &str, anchor_count: usize) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO hydration_state (project_root, last_hydrated_at, anchor_count)
                 VALUES (?1, ?2, ?3)",
                rusqlite::params![
                    project_root,
                    chrono::Utc::now().timestamp(),
                    anchor_count as i64,
                ],
            )
            .map_err(|e| format!("mark_hydrated: {e}"))?;
        Ok(())
    }

    /// Check if an anchor already exists in the local database.
    pub fn anchor_exists(&self, commit_hash: &str) -> Result<bool, String> {
        let exists: bool = self
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM anchors WHERE commit_hash = ?1)",
                rusqlite::params![commit_hash],
                |row| row.get(0),
            )
            .map_err(|e| format!("anchor_exists: {e}"))?;
        Ok(exists)
    }

    /// Insert an anchor (enriched commit) into the local database.
    pub fn insert_anchor(&self, commit_hash: &str, raw_json: &str) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO anchors (commit_hash, raw_json, created_at)
                 VALUES (?1, ?2, ?3)",
                rusqlite::params![commit_hash, raw_json, chrono::Utc::now().timestamp(),],
            )
            .map_err(|e| format!("insert anchor: {e}"))?;
        Ok(())
    }

    /// Insert a session link for an anchor into `anchor_sessions`.
    #[allow(clippy::too_many_arguments)]
    pub fn insert_anchor_session(
        &self,
        commit_hash: &str,
        session_id: &str,
        agent: &str,
        model: Option<&str>,
        link_type: &str,
        files_touched: Option<&[String]>,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        cache_read_tokens: Option<u64>,
        cache_creation_tokens: Option<u64>,
        duration_secs: Option<u64>,
        tool_calls: Option<u32>,
        is_subagent: bool,
        parent_session_id: Option<&str>,
        subagent_type: Option<&str>,
    ) -> Result<(), String> {
        let ft_json = files_touched.map(|ft| serde_json::to_string(ft).unwrap_or_default());
        self.conn
            .execute(
                "INSERT OR REPLACE INTO anchor_sessions
                 (commit_hash, session_id, agent, model, link_type, files_touched,
                  input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                  duration_secs, tool_calls, is_subagent, parent_session_id, subagent_type)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                rusqlite::params![
                    commit_hash,
                    session_id,
                    agent,
                    model,
                    link_type,
                    ft_json,
                    input_tokens.map(|v| v as i64),
                    output_tokens.map(|v| v as i64),
                    cache_read_tokens.map(|v| v as i64),
                    cache_creation_tokens.map(|v| v as i64),
                    duration_secs.map(|v| v as i64),
                    tool_calls.map(|v| v as i64),
                    is_subagent as i64,
                    parent_session_id,
                    subagent_type,
                ],
            )
            .map_err(|e| format!("insert anchor_session: {e}"))?;
        Ok(())
    }

    /// Cross-project summary stats for the bare `oobo` view.
    ///
    /// Returns aggregate anchor count, token total, AI percentage, and the
    /// last-activity timestamp for a project. All queries degrade gracefully
    /// to zero on error (this is a best-effort summary, not a critical path).
    pub fn anchor_stats_for_project(
        &self,
        project_id: &str,
    ) -> Result<crate::db::projects::AnchorStats, String> {
        use rusqlite::params;
        let mut stats = crate::db::projects::AnchorStats::default();

        // Anchor count via ai_commits → anchors join.
        let anchors: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM ai_commits WHERE project_id = ?1",
                params![project_id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        stats.anchors = anchors;

        // Tokens via anchor_sessions joined to ai_commits.
        let tokens: i64 = self
            .conn
            .query_row(
                "SELECT COALESCE(SUM(COALESCE(input_tokens,0)+COALESCE(output_tokens,0)+\
                         COALESCE(cache_read_tokens,0)+COALESCE(cache_creation_tokens,0)), 0)
                 FROM anchor_sessions s
                 JOIN ai_commits c ON c.commit_hash = s.commit_hash
                 WHERE c.project_id = ?1",
                params![project_id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        stats.tokens = tokens;

        // AI% average from ai_commits.
        let ai_pct: f64 = self
            .conn
            .query_row(
                "SELECT COALESCE(AVG(ai_percentage), 0.0)
                 FROM ai_commits
                 WHERE project_id = ?1 AND ai_percentage IS NOT NULL",
                params![project_id],
                |r| r.get(0),
            )
            .unwrap_or(0.0);
        stats.ai_pct = ai_pct.round() as i64;

        // Most recent activity.
        let last: i64 = self
            .conn
            .query_row(
                "SELECT COALESCE(MAX(updated_at), 0) FROM sessions WHERE project_id = ?1",
                params![project_id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        stats.last_activity = last;

        Ok(stats)
    }

    /// Update a session's first_message field.
    pub fn update_session_first_message(
        &self,
        session_id: &str,
        first_message: &str,
    ) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE sessions SET first_message = ?1 WHERE id = ?2",
                rusqlite::params![first_message, session_id],
            )
            .map_err(|e| format!("update first_message: {e}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_open_in_memory() {
        let db = Db::open_in_memory().unwrap();
        let version: i32 = db
            .conn
            .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert!(version >= 1);
    }

    #[test]
    fn test_wal_mode() {
        let db = Db::open_in_memory().unwrap();
        let mode: String = db
            .conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        // In-memory databases use "memory" mode, not WAL
        assert!(!mode.is_empty());
    }

    #[test]
    fn test_busy_timeout_set() {
        let db = Db::open_in_memory().unwrap();
        let timeout: i32 = db
            .conn
            .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
            .unwrap();
        assert_eq!(timeout, 5000);
    }

    fn seed_project_and_session(db: &Db) {
        db.conn
            .execute_batch(
                "INSERT INTO projects (id, path, name, discovered_at, last_seen_at)
                 VALUES ('proj-1', '/tmp/proj', 'test-proj', 1000, 1000);
                 INSERT INTO sessions (id, source, project_id, message_count, indexed_at)
                 VALUES ('sess-1', 'cursor', 'proj-1', 3, 1000);",
            )
            .unwrap();
    }

    #[test]
    fn test_insert_anchor_session_and_verify() {
        let db = Db::open_in_memory().unwrap();
        db.insert_anchor("abc123", r#"{"commit":"abc123"}"#)
            .unwrap();

        let files = vec!["src/main.rs".to_string(), "src/lib.rs".to_string()];
        db.insert_anchor_session(
            "abc123",
            "sess-42",
            "cursor",
            Some("claude-opus"),
            "explicit",
            Some(&files),
            Some(1500),
            Some(3000),
            None,
            None,
            Some(120),
            Some(5),
            false,
            None,
            None,
        )
        .unwrap();

        #[allow(clippy::type_complexity)]
        let row: (
            String,
            String,
            String,
            Option<String>,
            String,
            Option<String>,
            Option<i64>,
            Option<i64>,
            Option<i64>,
            Option<i64>,
        ) = db
            .conn
            .query_row(
                "SELECT commit_hash, session_id, agent, model, link_type, files_touched,
                        input_tokens, output_tokens, duration_secs, tool_calls
                 FROM anchor_sessions WHERE commit_hash = ?1 AND session_id = ?2",
                rusqlite::params!["abc123", "sess-42"],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                        r.get(8)?,
                        r.get(9)?,
                    ))
                },
            )
            .unwrap();

        assert_eq!(row.0, "abc123");
        assert_eq!(row.1, "sess-42");
        assert_eq!(row.2, "cursor");
        assert_eq!(row.3.as_deref(), Some("claude-opus"));
        assert_eq!(row.4, "explicit");
        assert!(row.5.is_some());
        let ft: Vec<String> = serde_json::from_str(&row.5.unwrap()).unwrap();
        assert_eq!(ft, files);
        assert_eq!(row.6, Some(1500));
        assert_eq!(row.7, Some(3000));
        assert_eq!(row.8, Some(120));
        assert_eq!(row.9, Some(5));
    }

    #[test]
    fn test_insert_anchor_session_without_model() {
        let db = Db::open_in_memory().unwrap();
        db.insert_anchor("def456", "{}").unwrap();

        db.insert_anchor_session(
            "def456", "sess-99", "claude", None, "inferred", None, None, None, None, None, None,
            None, false, None, None,
        )
        .unwrap();

        let model: Option<String> = db
            .conn
            .query_row(
                "SELECT model FROM anchor_sessions WHERE commit_hash = ?1",
                rusqlite::params!["def456"],
                |r| r.get(0),
            )
            .unwrap();
        assert!(model.is_none());
    }

    #[test]
    fn test_update_session_first_message() {
        let db = Db::open_in_memory().unwrap();
        seed_project_and_session(&db);

        let before: Option<String> = db
            .conn
            .query_row(
                "SELECT first_message FROM sessions WHERE id = 'sess-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(before.is_none());

        db.update_session_first_message("sess-1", "Fix the login bug")
            .unwrap();

        let after: Option<String> = db
            .conn
            .query_row(
                "SELECT first_message FROM sessions WHERE id = 'sess-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(after.as_deref(), Some("Fix the login bug"));
    }

    #[test]
    fn test_update_session_first_message_overwrites() {
        let db = Db::open_in_memory().unwrap();
        seed_project_and_session(&db);

        db.update_session_first_message("sess-1", "original")
            .unwrap();
        db.update_session_first_message("sess-1", "updated")
            .unwrap();

        let msg: Option<String> = db
            .conn
            .query_row(
                "SELECT first_message FROM sessions WHERE id = 'sess-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(msg.as_deref(), Some("updated"));
    }
}

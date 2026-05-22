use rusqlite::{params, OptionalExtension};

use super::Db;

#[derive(Debug, Clone)]
pub struct SessionRow {
    pub id: String,
    pub source: String,
    pub project_id: String,
    pub name: Option<String>,
    pub mode: Option<String>,
    pub model: Option<String>,
    pub created_at: Option<i64>,
    pub updated_at: Option<i64>,
    pub message_count: i32,
    pub first_message: Option<String>,
    pub indexed_at: i64,
}

impl Db {
    pub fn upsert_session(&self, session: &SessionRow) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO sessions (id, source, project_id, name, mode, model, created_at, updated_at, message_count, first_message, indexed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT(id, source) DO UPDATE SET
                     name = COALESCE(excluded.name, sessions.name),
                     mode = COALESCE(excluded.mode, sessions.mode),
                     model = COALESCE(excluded.model, sessions.model),
                     updated_at = COALESCE(excluded.updated_at, sessions.updated_at),
                     message_count = excluded.message_count,
                     first_message = COALESCE(excluded.first_message, sessions.first_message),
                     indexed_at = excluded.indexed_at",
                params![
                    session.id,
                    session.source,
                    session.project_id,
                    session.name,
                    session.mode,
                    session.model,
                    session.created_at,
                    session.updated_at,
                    session.message_count,
                    session.first_message,
                    session.indexed_at,
                ],
            )
            .map_err(|e| format!("cannot upsert session: {e}"))?;
        Ok(())
    }

    pub fn get_session(&self, id: &str, source: &str) -> Result<Option<SessionRow>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, source, project_id, name, mode, model, created_at, updated_at, message_count, first_message, indexed_at
                 FROM sessions WHERE id = ?1 AND source = ?2",
            )
            .map_err(|e| format!("cannot prepare: {e}"))?;

        stmt.query_row(params![id, source], |row| Ok(row_to_session(row)))
            .optional()
            .map_err(|e| format!("cannot query session: {e}"))
    }

    pub fn list_sessions_by_project(&self, project_id: &str) -> Result<Vec<SessionRow>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, source, project_id, name, mode, model, created_at, updated_at, message_count, first_message, indexed_at
                 FROM sessions WHERE project_id = ?1
                 ORDER BY COALESCE(updated_at, created_at) DESC",
            )
            .map_err(|e| format!("cannot prepare: {e}"))?;

        let rows = stmt
            .query_map(params![project_id], |row| Ok(row_to_session(row)))
            .map_err(|e| format!("cannot list sessions: {e}"))?;

        collect_rows(rows)
    }

    #[cfg(test)]
    pub fn list_sessions_by_source(&self, source: &str) -> Result<Vec<SessionRow>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, source, project_id, name, mode, model, created_at, updated_at, message_count, first_message, indexed_at
                 FROM sessions WHERE source = ?1
                 ORDER BY COALESCE(updated_at, created_at) DESC",
            )
            .map_err(|e| format!("cannot prepare: {e}"))?;

        let rows = stmt
            .query_map(params![source], |row| Ok(row_to_session(row)))
            .map_err(|e| format!("cannot list sessions: {e}"))?;

        collect_rows(rows)
    }

    pub fn count_sessions_by_project(&self, project_id: &str) -> Result<i64, String> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE project_id = ?1",
                params![project_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("cannot count sessions: {e}"))
    }

    pub fn list_all_sessions(&self) -> Result<Vec<SessionRow>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, source, project_id, name, mode, model, created_at, updated_at, message_count, first_message, indexed_at
                 FROM sessions
                 ORDER BY COALESCE(updated_at, created_at) DESC",
            )
            .map_err(|e| format!("cannot prepare: {e}"))?;

        let rows = stmt
            .query_map([], |row| Ok(row_to_session(row)))
            .map_err(|e| format!("cannot list sessions: {e}"))?;

        collect_rows(rows)
    }

    /// Return sessions that need indexing: either no stats yet, or stats are
    /// stale (session updated after stats were computed).
    /// The staleness SQL mirrors [`super::stats::StatsRow::is_stale`].
    pub fn list_unindexed_sessions(&self) -> Result<Vec<SessionRow>, String> {
        let sql = format!(
            "SELECT s.id, s.source, s.project_id, s.name, s.mode, s.model,
                    s.created_at, s.updated_at, s.message_count, s.first_message, s.indexed_at
             FROM sessions s
             LEFT JOIN session_stats ss ON s.id = ss.session_id AND s.source = ss.source
             WHERE ss.session_id IS NULL
                OR (s.updated_at IS NOT NULL AND {expr} > ss.computed_at)
             ORDER BY COALESCE(s.updated_at, s.created_at) DESC",
            expr = super::stats::UPDATED_AT_EPOCH_SECS_SQL,
        );
        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| format!("cannot prepare: {e}"))?;

        let rows = stmt
            .query_map([], |row| Ok(row_to_session(row)))
            .map_err(|e| format!("cannot list unindexed sessions: {e}"))?;

        collect_rows(rows)
    }

    /// Return sessions for a project that need indexing: either no stats yet,
    /// or stats are stale.
    /// The staleness SQL mirrors [`super::stats::StatsRow::is_stale`].
    pub fn list_unindexed_sessions_by_project(
        &self,
        project_id: &str,
    ) -> Result<Vec<SessionRow>, String> {
        let sql = format!(
            "SELECT s.id, s.source, s.project_id, s.name, s.mode, s.model,
                    s.created_at, s.updated_at, s.message_count, s.first_message, s.indexed_at
             FROM sessions s
             LEFT JOIN session_stats ss ON s.id = ss.session_id AND s.source = ss.source
             WHERE s.project_id = ?1
               AND (ss.session_id IS NULL
                    OR (s.updated_at IS NOT NULL AND {expr} > ss.computed_at))
             ORDER BY COALESCE(s.updated_at, s.created_at) DESC",
            expr = super::stats::UPDATED_AT_EPOCH_SECS_SQL,
        );
        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| format!("cannot prepare: {e}"))?;

        let rows = stmt
            .query_map(params![project_id], |row| Ok(row_to_session(row)))
            .map_err(|e| format!("cannot list unindexed sessions: {e}"))?;

        collect_rows(rows)
    }

    #[cfg(test)]
    pub fn delete_sessions_by_project(&self, project_id: &str) -> Result<(), String> {
        self.conn
            .execute(
                "DELETE FROM sessions WHERE project_id = ?1",
                params![project_id],
            )
            .map_err(|e| format!("cannot delete sessions: {e}"))?;
        Ok(())
    }
}

fn row_to_session(row: &rusqlite::Row) -> SessionRow {
    SessionRow {
        id: row.get(0).unwrap_or_default(),
        source: row.get(1).unwrap_or_default(),
        project_id: row.get(2).unwrap_or_default(),
        name: row.get(3).unwrap_or_default(),
        mode: row.get(4).unwrap_or_default(),
        model: row.get(5).unwrap_or_default(),
        created_at: row.get(6).unwrap_or_default(),
        updated_at: row.get(7).unwrap_or_default(),
        message_count: row.get(8).unwrap_or(0),
        first_message: row.get(9).unwrap_or_default(),
        indexed_at: row.get(10).unwrap_or(0),
    }
}

fn collect_rows(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<SessionRow>>,
) -> Result<Vec<SessionRow>, String> {
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| format!("row error: {e}"))?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::projects::ProjectRow;

    fn test_db() -> Db {
        let db = Db::open_in_memory().unwrap();
        db.upsert_project(&ProjectRow {
            id: "test-project".into(),
            path: "/test".into(),
            name: "test".into(),
            git_remote: None,
            initial_commit_sha: None,
            historical_paths: Vec::new(),
            discovered_at: 1000,
            last_seen_at: 1000,
            last_scanned_at: 0,
            tools: vec![],
        })
        .unwrap();
        db
    }

    fn sample_session() -> SessionRow {
        SessionRow {
            id: "session-abc".into(),
            source: "cursor".into(),
            project_id: "test-project".into(),
            name: Some("Fix auth bug".into()),
            mode: Some("agent".into()),
            model: None,
            created_at: Some(1000),
            updated_at: Some(2000),
            message_count: 5,
            first_message: Some("Fix the auth bug in login.rs".into()),
            indexed_at: 3000,
        }
    }

    #[test]
    fn test_upsert_and_get() {
        let db = test_db();
        db.upsert_session(&sample_session()).unwrap();

        let found = db.get_session("session-abc", "cursor").unwrap().unwrap();
        assert_eq!(found.name.as_deref(), Some("Fix auth bug"));
        assert_eq!(found.message_count, 5);
    }

    #[test]
    fn test_upsert_updates() {
        let db = test_db();
        let mut s = sample_session();
        db.upsert_session(&s).unwrap();

        s.message_count = 10;
        s.model = Some("claude-opus".into());
        db.upsert_session(&s).unwrap();

        let found = db.get_session("session-abc", "cursor").unwrap().unwrap();
        assert_eq!(found.message_count, 10);
        assert_eq!(found.model.as_deref(), Some("claude-opus"));
    }

    #[test]
    fn test_list_by_project() {
        let db = test_db();
        db.upsert_session(&sample_session()).unwrap();

        let mut s2 = sample_session();
        s2.id = "session-def".into();
        s2.source = "claude".into();
        s2.updated_at = Some(5000);
        db.upsert_session(&s2).unwrap();

        let list = db.list_sessions_by_project("test-project").unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, "session-def"); // more recent first
    }

    #[test]
    fn test_list_by_source() {
        let db = test_db();
        db.upsert_session(&sample_session()).unwrap();

        let list = db.list_sessions_by_source("cursor").unwrap();
        assert_eq!(list.len(), 1);

        let list = db.list_sessions_by_source("claude").unwrap();
        assert!(list.is_empty());
    }

    #[test]
    fn test_count_by_project() {
        let db = test_db();
        db.upsert_session(&sample_session()).unwrap();
        let count = db.count_sessions_by_project("test-project").unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_delete_by_project() {
        let db = test_db();
        db.upsert_session(&sample_session()).unwrap();
        db.delete_sessions_by_project("test-project").unwrap();
        let count = db.count_sessions_by_project("test-project").unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_get_nonexistent() {
        let db = test_db();
        let found = db.get_session("nope", "cursor").unwrap();
        assert!(found.is_none());
    }

    #[test]
    fn test_list_all_sessions() {
        let db = test_db();
        db.upsert_session(&sample_session()).unwrap();

        let mut s2 = sample_session();
        s2.id = "session-xyz".into();
        s2.source = "claude".into();
        s2.updated_at = Some(9000);
        db.upsert_session(&s2).unwrap();

        let all = db.list_all_sessions().unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].id, "session-xyz");
    }

    #[test]
    fn test_list_unindexed_sessions() {
        let db = test_db();
        db.upsert_session(&sample_session()).unwrap();

        let unindexed = db.list_unindexed_sessions().unwrap();
        assert_eq!(unindexed.len(), 1);

        use crate::db::stats::StatsRow;
        db.upsert_stats(&StatsRow {
            session_id: "session-abc".into(),
            source: "cursor".into(),
            model: Some("gpt-4o".into()),
            input_tokens: Some(100),
            output_tokens: Some(50),
            computed_at: 3000,
            ..Default::default()
        })
        .unwrap();

        let unindexed = db.list_unindexed_sessions().unwrap();
        assert!(unindexed.is_empty());
    }

    #[test]
    fn test_list_unindexed_sessions_by_project() {
        let db = test_db();
        db.upsert_session(&sample_session()).unwrap();

        let unindexed = db
            .list_unindexed_sessions_by_project("test-project")
            .unwrap();
        assert_eq!(unindexed.len(), 1);

        let unindexed = db
            .list_unindexed_sessions_by_project("other-project")
            .unwrap();
        assert!(unindexed.is_empty());
    }

    #[test]
    fn test_upsert_preserves_model_on_null() {
        let db = test_db();
        let mut s = sample_session();
        s.model = Some("claude-opus".into());
        db.upsert_session(&s).unwrap();

        s.model = None;
        s.message_count = 20;
        db.upsert_session(&s).unwrap();

        let found = db.get_session("session-abc", "cursor").unwrap().unwrap();
        assert_eq!(found.model.as_deref(), Some("claude-opus"));
        assert_eq!(found.message_count, 20);
    }

    #[test]
    fn test_upsert_preserves_first_message_on_null() {
        let db = test_db();
        let mut s = sample_session();
        s.first_message = Some("hello world".into());
        db.upsert_session(&s).unwrap();

        s.first_message = None;
        db.upsert_session(&s).unwrap();

        let found = db.get_session("session-abc", "cursor").unwrap().unwrap();
        assert_eq!(found.first_message.as_deref(), Some("hello world"));
    }

    #[test]
    fn test_composite_key_different_sources() {
        let db = test_db();
        let mut s1 = sample_session();
        s1.name = Some("cursor session".into());
        db.upsert_session(&s1).unwrap();

        let mut s2 = sample_session();
        s2.source = "claude".into();
        s2.name = Some("claude session".into());
        db.upsert_session(&s2).unwrap();

        let cursor = db.get_session("session-abc", "cursor").unwrap().unwrap();
        let claude = db.get_session("session-abc", "claude").unwrap().unwrap();
        assert_eq!(cursor.name.as_deref(), Some("cursor session"));
        assert_eq!(claude.name.as_deref(), Some("claude session"));
    }

    #[test]
    fn test_list_unindexed_sessions_returns_stale() {
        let db = test_db();
        db.upsert_session(&sample_session()).unwrap();

        use crate::db::stats::StatsRow;
        db.upsert_stats(&StatsRow {
            session_id: "session-abc".into(),
            source: "cursor".into(),
            model: Some("gpt-4o".into()),
            input_tokens: Some(100),
            output_tokens: Some(50),
            computed_at: 1500,
            ..Default::default()
        })
        .unwrap();

        // Session updated_at (2000) > stats computed_at (1500) → stale
        let unindexed = db.list_unindexed_sessions().unwrap();
        assert_eq!(unindexed.len(), 1);
        assert_eq!(unindexed[0].id, "session-abc");
    }

    #[test]
    fn test_list_unindexed_sessions_by_project_returns_stale() {
        let db = test_db();
        db.upsert_session(&sample_session()).unwrap();

        use crate::db::stats::StatsRow;
        db.upsert_stats(&StatsRow {
            session_id: "session-abc".into(),
            source: "cursor".into(),
            input_tokens: Some(100),
            output_tokens: Some(50),
            computed_at: 1500,
            ..Default::default()
        })
        .unwrap();

        let unindexed = db
            .list_unindexed_sessions_by_project("test-project")
            .unwrap();
        assert_eq!(unindexed.len(), 1);

        // Non-stale after re-computing with fresh timestamp
        db.upsert_stats(&StatsRow {
            session_id: "session-abc".into(),
            source: "cursor".into(),
            input_tokens: Some(200),
            output_tokens: Some(100),
            computed_at: 3000,
            ..Default::default()
        })
        .unwrap();

        let unindexed = db
            .list_unindexed_sessions_by_project("test-project")
            .unwrap();
        assert!(unindexed.is_empty());
    }
}

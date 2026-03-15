use rusqlite::params;
use serde::{Deserialize, Serialize};

use super::Db;

/// Per-project settings stored as JSON in the `project_settings` table.
/// `None` means "use global default".
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transparency: Option<String>,
    #[serde(default)]
    pub ignored: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ProjectRow {
    pub id: String,
    pub path: String,
    pub name: String,
    pub git_remote: Option<String>,
    pub discovered_at: i64,
    pub last_seen_at: i64,
    pub last_scanned_at: i64,
    pub tools: Vec<String>,
}

impl Db {
    /// Lightweight insert that creates the project row only if it doesn't
    /// already exist.  Uses `INSERT OR IGNORE` so existing data is never
    /// overwritten — safe to call on every git operation.
    pub fn ensure_project(&self, id: &str, path: &str) -> Result<(), String> {
        let name = std::path::Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();
        let now = chrono::Utc::now().timestamp();
        self.conn
            .execute(
                "INSERT OR IGNORE INTO projects (id, path, name, discovered_at, last_seen_at, last_scanned_at, tools)
                 VALUES (?1, ?2, ?3, ?4, ?4, 0, '[]')",
                params![id, path, name, now],
            )
            .map_err(|e| format!("cannot ensure project: {e}"))?;
        Ok(())
    }

    pub fn upsert_project(&self, project: &ProjectRow) -> Result<(), String> {
        let tools_json = serde_json::to_string(&project.tools).unwrap_or_else(|_| "[]".to_string());

        self.conn
            .execute(
                "INSERT INTO projects (id, path, name, git_remote, discovered_at, last_seen_at, last_scanned_at, tools)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(id) DO UPDATE SET
                     last_seen_at = excluded.last_seen_at,
                     git_remote = COALESCE(excluded.git_remote, projects.git_remote),
                     tools = excluded.tools",
                params![
                    project.id,
                    project.path,
                    project.name,
                    project.git_remote,
                    project.discovered_at,
                    project.last_seen_at,
                    project.last_scanned_at,
                    tools_json,
                ],
            )
            .map_err(|e| format!("cannot upsert project: {e}"))?;
        Ok(())
    }

    pub fn get_project_by_id(&self, id: &str) -> Result<Option<ProjectRow>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, path, name, git_remote, discovered_at, last_seen_at, last_scanned_at, tools
                 FROM projects WHERE id = ?1",
            )
            .map_err(|e| format!("cannot prepare: {e}"))?;

        let result = stmt
            .query_row(params![id], |row| Ok(row_to_project(row)))
            .optional()
            .map_err(|e| format!("cannot query project: {e}"))?;

        Ok(result)
    }

    pub fn get_project_by_path(&self, path: &str) -> Result<Option<ProjectRow>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, path, name, git_remote, discovered_at, last_seen_at, last_scanned_at, tools
                 FROM projects WHERE path = ?1",
            )
            .map_err(|e| format!("cannot prepare: {e}"))?;

        let result = stmt
            .query_row(params![path], |row| Ok(row_to_project(row)))
            .optional()
            .map_err(|e| format!("cannot query project: {e}"))?;

        Ok(result)
    }

    pub fn list_projects(&self) -> Result<Vec<ProjectRow>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, path, name, git_remote, discovered_at, last_seen_at, last_scanned_at, tools
                 FROM projects ORDER BY last_seen_at DESC",
            )
            .map_err(|e| format!("cannot prepare: {e}"))?;

        let rows = stmt
            .query_map([], |row| Ok(row_to_project(row)))
            .map_err(|e| format!("cannot list projects: {e}"))?;

        let mut projects = Vec::new();
        for row in rows {
            projects.push(row.map_err(|e| format!("row error: {e}"))?);
        }
        Ok(projects)
    }

    pub fn delete_project(&self, id: &str) -> Result<(), String> {
        self.conn
            .execute_batch("BEGIN")
            .map_err(|e| format!("cannot begin transaction: {e}"))?;

        let result = (|| -> Result<(), String> {
            self.conn
                .execute("DELETE FROM session_stats WHERE (session_id, source) IN (SELECT id, source FROM sessions WHERE project_id = ?1)", params![id])
                .map_err(|e| format!("cannot delete session_stats: {e}"))?;
            self.conn
                .execute("DELETE FROM sessions WHERE project_id = ?1", params![id])
                .map_err(|e| format!("cannot delete sessions: {e}"))?;
            self.conn
                .execute("DELETE FROM events WHERE project_id = ?1", params![id])
                .map_err(|e| format!("cannot delete events: {e}"))?;
            self.conn
                .execute(
                    "DELETE FROM project_settings WHERE project_id = ?1",
                    params![id],
                )
                .map_err(|e| format!("cannot delete settings: {e}"))?;
            self.conn
                .execute("DELETE FROM projects WHERE id = ?1", params![id])
                .map_err(|e| format!("cannot delete project: {e}"))?;
            Ok(())
        })();

        match result {
            Ok(()) => {
                self.conn
                    .execute_batch("COMMIT")
                    .map_err(|e| format!("cannot commit: {e}"))?;
                Ok(())
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }

    #[cfg(test)]
    pub fn update_project_tools(&self, id: &str, tools: &[String]) -> Result<(), String> {
        let tools_json = serde_json::to_string(tools).unwrap_or_else(|_| "[]".to_string());
        self.conn
            .execute(
                "UPDATE projects SET tools = ?1 WHERE id = ?2",
                params![tools_json, id],
            )
            .map_err(|e| format!("cannot update tools: {e}"))?;
        Ok(())
    }

    pub fn get_project_settings(&self, project_id: &str) -> Result<ProjectSettings, String> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT settings FROM project_settings WHERE project_id = ?1",
                params![project_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("cannot read project_settings: {e}"))?
            .flatten();

        match json {
            Some(s) => serde_json::from_str(&s).map_err(|e| format!("bad project settings: {e}")),
            None => Ok(ProjectSettings::default()),
        }
    }

    pub fn set_project_settings(
        &self,
        project_id: &str,
        settings: &ProjectSettings,
    ) -> Result<(), String> {
        let json =
            serde_json::to_string(settings).map_err(|e| format!("serialize settings: {e}"))?;
        self.conn
            .execute(
                "INSERT INTO project_settings (project_id, settings)
                 VALUES (?1, ?2)
                 ON CONFLICT(project_id) DO UPDATE SET settings = excluded.settings",
                params![project_id, json],
            )
            .map_err(|e| format!("cannot upsert project_settings: {e}"))?;
        Ok(())
    }

    /// Look up project settings by repo path (resolves project_id first).
    pub fn get_project_settings_by_path(&self, path: &str) -> Result<ProjectSettings, String> {
        match self.get_project_by_path(path)? {
            Some(p) => self.get_project_settings(&p.id),
            None => Ok(ProjectSettings::default()),
        }
    }

    pub fn update_project_last_scanned(&self, id: &str, timestamp: i64) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE projects SET last_scanned_at = ?1 WHERE id = ?2",
                params![timestamp, id],
            )
            .map_err(|e| format!("cannot update last_scanned_at: {e}"))?;
        Ok(())
    }
}

/// Derive a project ID from a filesystem path (e.g. "/Users/ted/my-app" → "Users-ted-my-app").
pub fn path_to_project_id(path: &str) -> String {
    path.trim_start_matches('/').replace(['/', '\\'], "-")
}

fn row_to_project(row: &rusqlite::Row) -> ProjectRow {
    let tools_json: String = row.get(7).unwrap_or_default();
    let tools: Vec<String> = serde_json::from_str(&tools_json).unwrap_or_default();

    ProjectRow {
        id: row.get(0).unwrap_or_default(),
        path: row.get(1).unwrap_or_default(),
        name: row.get(2).unwrap_or_default(),
        git_remote: row.get(3).unwrap_or_default(),
        discovered_at: row.get(4).unwrap_or(0),
        last_seen_at: row.get(5).unwrap_or(0),
        last_scanned_at: row.get(6).unwrap_or(0),
        tools,
    }
}

use rusqlite::OptionalExtension;

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Db {
        Db::open_in_memory().unwrap()
    }

    fn sample_project() -> ProjectRow {
        ProjectRow {
            id: "Users-test-project".into(),
            path: "/Users/test/project".into(),
            name: "project".into(),
            git_remote: Some("git@github.com:test/project.git".into()),
            discovered_at: 1000,
            last_seen_at: 2000,
            last_scanned_at: 0,
            tools: vec!["cursor".into(), "claude".into()],
        }
    }

    #[test]
    fn test_upsert_and_get() {
        let db = test_db();
        let p = sample_project();
        db.upsert_project(&p).unwrap();

        let found = db.get_project_by_id("Users-test-project").unwrap().unwrap();
        assert_eq!(found.path, "/Users/test/project");
        assert_eq!(found.name, "project");
        assert_eq!(found.tools, vec!["cursor", "claude"]);
    }

    #[test]
    fn test_upsert_updates_last_seen() {
        let db = test_db();
        let mut p = sample_project();
        db.upsert_project(&p).unwrap();

        p.last_seen_at = 9999;
        db.upsert_project(&p).unwrap();

        let found = db.get_project_by_id("Users-test-project").unwrap().unwrap();
        assert_eq!(found.last_seen_at, 9999);
    }

    #[test]
    fn test_get_by_path() {
        let db = test_db();
        db.upsert_project(&sample_project()).unwrap();

        let found = db
            .get_project_by_path("/Users/test/project")
            .unwrap()
            .unwrap();
        assert_eq!(found.id, "Users-test-project");
    }

    #[test]
    fn test_list_projects() {
        let db = test_db();
        db.upsert_project(&sample_project()).unwrap();

        let mut p2 = sample_project();
        p2.id = "other-project".into();
        p2.path = "/other".into();
        p2.name = "other".into();
        p2.last_seen_at = 5000;
        db.upsert_project(&p2).unwrap();

        let list = db.list_projects().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, "other-project"); // most recent first
    }

    #[test]
    fn test_delete_project() {
        let db = test_db();
        db.upsert_project(&sample_project()).unwrap();
        db.delete_project("Users-test-project").unwrap();
        let found = db.get_project_by_id("Users-test-project").unwrap();
        assert!(found.is_none());
    }

    #[test]
    fn test_update_tools() {
        let db = test_db();
        db.upsert_project(&sample_project()).unwrap();
        db.update_project_tools("Users-test-project", &["zed".into()])
            .unwrap();
        let found = db.get_project_by_id("Users-test-project").unwrap().unwrap();
        assert_eq!(found.tools, vec!["zed"]);
    }

    #[test]
    fn test_update_last_scanned() {
        let db = test_db();
        db.upsert_project(&sample_project()).unwrap();
        db.update_project_last_scanned("Users-test-project", 12345)
            .unwrap();
        let found = db.get_project_by_id("Users-test-project").unwrap().unwrap();
        assert_eq!(found.last_scanned_at, 12345);
    }

    #[test]
    fn test_get_nonexistent() {
        let db = test_db();
        let found = db.get_project_by_id("does-not-exist").unwrap();
        assert!(found.is_none());
    }

    #[test]
    fn test_get_by_path_nonexistent() {
        let db = test_db();
        let found = db.get_project_by_path("/no/such/path").unwrap();
        assert!(found.is_none());
    }

    #[test]
    fn test_upsert_preserves_git_remote_on_null() {
        let db = test_db();
        let p = sample_project();
        db.upsert_project(&p).unwrap();

        let mut p2 = p.clone();
        p2.git_remote = None;
        p2.last_seen_at = 5000;
        db.upsert_project(&p2).unwrap();

        let found = db.get_project_by_id("Users-test-project").unwrap().unwrap();
        assert_eq!(
            found.git_remote.as_deref(),
            Some("git@github.com:test/project.git")
        );
        assert_eq!(found.last_seen_at, 5000);
    }

    #[test]
    fn test_empty_tools() {
        let db = test_db();
        let mut p = sample_project();
        p.tools = vec![];
        db.upsert_project(&p).unwrap();

        let found = db.get_project_by_id("Users-test-project").unwrap().unwrap();
        assert!(found.tools.is_empty());
    }

    #[test]
    fn test_delete_cascades_sessions_and_stats() {
        let db = test_db();
        db.upsert_project(&sample_project()).unwrap();

        use crate::db::sessions::SessionRow;
        db.upsert_session(&SessionRow {
            id: "s1".into(),
            source: "cursor".into(),
            project_id: "Users-test-project".into(),
            name: Some("test".into()),
            mode: None,
            model: None,
            created_at: Some(1000),
            updated_at: Some(2000),
            message_count: 3,
            first_message: None,
            indexed_at: 3000,
        })
        .unwrap();

        use crate::db::stats::StatsRow;
        db.upsert_stats(&StatsRow {
            session_id: "s1".into(),
            source: "cursor".into(),
            model: Some("gpt-4o".into()),
            input_tokens: Some(100),
            output_tokens: Some(50),
            ..Default::default()
        })
        .unwrap();

        db.delete_project("Users-test-project").unwrap();

        let sessions = db.list_sessions_by_project("Users-test-project").unwrap();
        assert!(sessions.is_empty());
        let stats = db.get_stats("s1", "cursor").unwrap();
        assert!(stats.is_none());
    }

    #[test]
    fn test_delete_project_does_not_nuke_other_projects_stats() {
        let db = test_db();

        let mut pa = sample_project();
        pa.id = "proj-a".into();
        pa.path = "/proj-a".into();
        pa.name = "proj-a".into();
        db.upsert_project(&pa).unwrap();

        let mut pb = sample_project();
        pb.id = "proj-b".into();
        pb.path = "/proj-b".into();
        pb.name = "proj-b".into();
        db.upsert_project(&pb).unwrap();

        use crate::db::sessions::SessionRow;
        db.upsert_session(&SessionRow {
            id: "sa".into(),
            source: "composer".into(),
            project_id: "proj-a".into(),
            name: Some("session a".into()),
            mode: None,
            model: None,
            created_at: Some(1000),
            updated_at: Some(2000),
            message_count: 3,
            first_message: None,
            indexed_at: 3000,
        })
        .unwrap();

        db.upsert_session(&SessionRow {
            id: "sb".into(),
            source: "composer".into(),
            project_id: "proj-b".into(),
            name: Some("session b".into()),
            mode: None,
            model: None,
            created_at: Some(1000),
            updated_at: Some(2000),
            message_count: 5,
            first_message: None,
            indexed_at: 3000,
        })
        .unwrap();

        use crate::db::stats::StatsRow;
        db.upsert_stats(&StatsRow {
            session_id: "sa".into(),
            source: "composer".into(),
            model: Some("gpt-4o".into()),
            input_tokens: Some(100),
            output_tokens: Some(50),
            ..Default::default()
        })
        .unwrap();

        db.upsert_stats(&StatsRow {
            session_id: "sb".into(),
            source: "composer".into(),
            model: Some("gpt-4o".into()),
            input_tokens: Some(200),
            output_tokens: Some(80),
            ..Default::default()
        })
        .unwrap();

        db.delete_project("proj-a").unwrap();

        let stats_b = db.get_stats("sb", "composer").unwrap();
        assert!(
            stats_b.is_some(),
            "proj-b stats must survive deletion of proj-a"
        );
        assert_eq!(stats_b.unwrap().input_tokens, Some(200));

        let stats_a = db.get_stats("sa", "composer").unwrap();
        assert!(stats_a.is_none(), "proj-a stats must be deleted");

        let sessions_b = db.list_sessions_by_project("proj-b").unwrap();
        assert_eq!(sessions_b.len(), 1);
    }

    #[test]
    fn test_list_projects_ordering() {
        let db = test_db();

        let mut p1 = sample_project();
        p1.id = "old-proj".into();
        p1.path = "/old".into();
        p1.last_seen_at = 1000;
        db.upsert_project(&p1).unwrap();

        let mut p2 = sample_project();
        p2.id = "mid-proj".into();
        p2.path = "/mid".into();
        p2.last_seen_at = 5000;
        db.upsert_project(&p2).unwrap();

        let mut p3 = sample_project();
        p3.id = "new-proj".into();
        p3.path = "/new".into();
        p3.last_seen_at = 9000;
        db.upsert_project(&p3).unwrap();

        let list = db.list_projects().unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].id, "new-proj");
        assert_eq!(list[1].id, "mid-proj");
        assert_eq!(list[2].id, "old-proj");
    }

    #[test]
    fn test_ensure_project_creates_if_missing() {
        let db = test_db();
        db.ensure_project("Users-new-repo", "/Users/new/repo")
            .unwrap();

        let found = db.get_project_by_id("Users-new-repo").unwrap().unwrap();
        assert_eq!(found.path, "/Users/new/repo");
        assert_eq!(found.name, "repo");
        assert!(found.tools.is_empty());
    }

    #[test]
    fn test_ensure_project_does_not_overwrite_existing() {
        let db = test_db();
        db.upsert_project(&sample_project()).unwrap();

        db.ensure_project("Users-test-project", "/Users/test/project")
            .unwrap();

        let found = db.get_project_by_id("Users-test-project").unwrap().unwrap();
        assert_eq!(found.tools, vec!["cursor", "claude"]);
        assert_eq!(
            found.git_remote.as_deref(),
            Some("git@github.com:test/project.git")
        );
    }

    #[test]
    fn test_ensure_project_allows_event_insert() {
        let db = test_db();
        db.ensure_project("Users-fresh-repo", "/Users/fresh/repo")
            .unwrap();

        use crate::db::events::EventRow;
        db.insert_event(&EventRow {
            id: None,
            event: "git.push".into(),
            project_id: Some("Users-fresh-repo".into()),
            timestamp: 1234,
            data: None,
            synced: false,
        })
        .unwrap();
    }

    #[test]
    fn test_delete_nonexistent_project_succeeds() {
        let db = test_db();
        let result = db.delete_project("does-not-exist");
        assert!(
            result.is_ok(),
            "deleting nonexistent project should not error"
        );
    }
}

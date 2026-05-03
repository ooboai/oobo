#![allow(dead_code)]

use rusqlite::params;

use super::Db;

#[derive(Debug, Clone)]
pub struct EventRow {
    pub id: Option<i64>,
    pub event: String,
    pub project_id: Option<String>,
    pub timestamp: i64,
    pub data: Option<String>,
    pub synced: bool,
}

impl Db {
    pub fn insert_event(&self, event: &EventRow) -> Result<i64, String> {
        self.conn
            .execute(
                "INSERT INTO events (event, project_id, timestamp, data, synced)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    event.event,
                    event.project_id,
                    event.timestamp,
                    event.data,
                    event.synced as i32,
                ],
            )
            .map_err(|e| format!("cannot insert event: {e}"))?;

        Ok(self.conn.last_insert_rowid())
    }

    pub fn list_unsynced_events(&self, limit: i64) -> Result<Vec<EventRow>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, event, project_id, timestamp, data, synced
                 FROM events WHERE synced = 0
                 ORDER BY timestamp ASC
                 LIMIT ?1",
            )
            .map_err(|e| format!("cannot prepare: {e}"))?;

        let rows = stmt
            .query_map(params![limit], |row| {
                let synced_int: i32 = row.get(5)?;
                Ok(EventRow {
                    id: Some(row.get(0)?),
                    event: row.get(1)?,
                    project_id: row.get(2)?,
                    timestamp: row.get(3)?,
                    data: row.get(4)?,
                    synced: synced_int != 0,
                })
            })
            .map_err(|e| format!("cannot query events: {e}"))?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| format!("row error: {e}"))?);
        }
        Ok(out)
    }

    pub fn mark_events_synced(&self, ids: &[i64]) -> Result<(), String> {
        if ids.is_empty() {
            return Ok(());
        }

        let placeholders: String = std::iter::repeat_n("?", ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!("UPDATE events SET synced = 1 WHERE id IN ({placeholders})");
        let params: Vec<Box<dyn rusqlite::types::ToSql>> = ids
            .iter()
            .map(|id| Box::new(*id) as Box<dyn rusqlite::types::ToSql>)
            .collect();
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        self.conn
            .execute(&sql, param_refs.as_slice())
            .map_err(|e| format!("cannot mark synced: {e}"))?;
        Ok(())
    }

    pub fn count_events(&self) -> Result<i64, String> {
        self.conn
            .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
            .map_err(|e| format!("cannot count events: {e}"))
    }

    pub fn list_recent_events(&self, limit: i64) -> Result<Vec<EventRow>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, event, project_id, timestamp, data, synced
                 FROM events ORDER BY timestamp DESC LIMIT ?1",
            )
            .map_err(|e| format!("cannot prepare: {e}"))?;

        let rows = stmt
            .query_map(params![limit], |row| {
                let synced_int: i32 = row.get(5)?;
                Ok(EventRow {
                    id: Some(row.get(0)?),
                    event: row.get(1)?,
                    project_id: row.get(2)?,
                    timestamp: row.get(3)?,
                    data: row.get(4)?,
                    synced: synced_int != 0,
                })
            })
            .map_err(|e| format!("cannot query events: {e}"))?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| format!("row error: {e}"))?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::projects::ProjectRow;

    fn test_db() -> Db {
        let db = Db::open_in_memory().unwrap();
        db.upsert_project(&ProjectRow {
            id: "proj".into(),
            path: "/proj".into(),
            name: "proj".into(),
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

    #[test]
    fn test_insert_and_count() {
        let db = test_db();
        db.insert_event(&EventRow {
            id: None,
            event: "commit".into(),
            project_id: Some("proj".into()),
            timestamp: 1000,
            data: Some(r#"{"sha":"abc"}"#.into()),
            synced: false,
        })
        .unwrap();

        assert_eq!(db.count_events().unwrap(), 1);
    }

    #[test]
    fn test_unsynced() {
        let db = test_db();
        db.insert_event(&EventRow {
            id: None,
            event: "commit".into(),
            project_id: Some("proj".into()),
            timestamp: 1000,
            data: None,
            synced: false,
        })
        .unwrap();

        let events = db.list_unsynced_events(100).unwrap();
        assert_eq!(events.len(), 1);
        assert!(!events[0].synced);
    }

    #[test]
    fn test_mark_synced() {
        let db = test_db();
        let id = db
            .insert_event(&EventRow {
                id: None,
                event: "commit".into(),
                project_id: Some("proj".into()),
                timestamp: 1000,
                data: None,
                synced: false,
            })
            .unwrap();

        db.mark_events_synced(&[id]).unwrap();

        let events = db.list_unsynced_events(100).unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn test_mark_events_synced_partial() {
        let db = test_db();

        let id1 = db
            .insert_event(&EventRow {
                id: None,
                event: "commit".into(),
                project_id: Some("proj".into()),
                timestamp: 1000,
                data: None,
                synced: false,
            })
            .unwrap();

        let id2 = db
            .insert_event(&EventRow {
                id: None,
                event: "session_start".into(),
                project_id: Some("proj".into()),
                timestamp: 2000,
                data: None,
                synced: false,
            })
            .unwrap();

        let id3 = db
            .insert_event(&EventRow {
                id: None,
                event: "session_end".into(),
                project_id: Some("proj".into()),
                timestamp: 3000,
                data: None,
                synced: false,
            })
            .unwrap();

        db.mark_events_synced(&[id1, id2]).unwrap();

        let unsynced = db.list_unsynced_events(100).unwrap();
        assert_eq!(unsynced.len(), 1);
        assert_eq!(unsynced[0].id, Some(id3));
        assert!(!unsynced[0].synced);

        let all = db.list_recent_events(100).unwrap();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn test_recent_events() {
        let db = test_db();
        for i in 0..5 {
            db.insert_event(&EventRow {
                id: None,
                event: format!("event-{i}"),
                project_id: Some("proj".into()),
                timestamp: i * 100,
                data: None,
                synced: false,
            })
            .unwrap();
        }

        let recent = db.list_recent_events(3).unwrap();
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].event, "event-4"); // most recent first
    }
}

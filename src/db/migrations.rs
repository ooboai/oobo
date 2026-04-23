use rusqlite::Connection;
use std::path::Path;

const LATEST_VERSION: i32 = 9;

pub fn run(conn: &Connection) -> Result<(), String> {
    run_with_path(conn, None)
}

/// Run migrations, optionally with the db file path so we can take a
/// safety backup before applying schema-changing migrations.
pub fn run_with_path(conn: &Connection, db_path: Option<&Path>) -> Result<(), String> {
    ensure_schema_version_table(conn)?;

    let current = get_version(conn)?;
    if current >= LATEST_VERSION {
        return Ok(());
    }

    // Take a best-effort backup before applying migrations to a real,
    // non-empty database. Skipped for in-memory connections (no path) and
    // for brand-new dbs (current == 0).
    if current > 0 {
        if let Some(p) = db_path {
            if let Err(e) = backup_db(p, current) {
                eprintln!("oobo: warning: db backup failed before migration: {e}");
            }
        }
    }

    if current < 1 {
        migrate_v1(conn)?;
    }
    if current < 2 {
        migrate_v2(conn)?;
    }
    if current < 3 {
        migrate_v3(conn)?;
    }
    if current < 4 {
        migrate_v4(conn)?;
    }

    if current < 5 {
        migrate_v5(conn)?;
    }
    if current < 6 {
        migrate_v6(conn)?;
    }
    if current < 7 {
        migrate_v7(conn)?;
    }
    if current < 8 {
        migrate_v8(conn)?;
    }
    if current < 9 {
        migrate_v9(conn)?;
    }

    set_version(conn, LATEST_VERSION)?;
    Ok(())
}

fn backup_db(src: &Path, from_version: i32) -> Result<(), String> {
    if !src.exists() {
        return Ok(());
    }
    let parent = src.parent().unwrap_or(Path::new("."));
    let stem = src
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("oobo.db");
    let ts = chrono::Utc::now().format("%Y%m%dT%H%M%S");
    let backup = parent.join(format!("{stem}.pre-v{from_version}.{ts}.bak"));
    std::fs::copy(src, &backup).map_err(|e| format!("copy backup: {e}"))?;
    eprintln!("oobo: migrated db backup saved to {}", backup.display());
    Ok(())
}

fn ensure_schema_version_table(conn: &Connection) -> Result<(), String> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL);")
        .map_err(|e| format!("cannot create schema_version: {e}"))?;

    let count: i32 = conn
        .query_row("SELECT COUNT(*) FROM schema_version", [], |r| r.get(0))
        .map_err(|e| format!("cannot query schema_version: {e}"))?;

    if count == 0 {
        conn.execute("INSERT INTO schema_version (version) VALUES (0)", [])
            .map_err(|e| format!("cannot init schema_version: {e}"))?;
    }

    Ok(())
}

fn get_version(conn: &Connection) -> Result<i32, String> {
    conn.query_row("SELECT version FROM schema_version", [], |r| r.get(0))
        .map_err(|e| format!("cannot read schema version: {e}"))
}

fn set_version(conn: &Connection, version: i32) -> Result<(), String> {
    conn.execute("UPDATE schema_version SET version = ?1", [version])
        .map_err(|e| format!("cannot update schema version: {e}"))?;
    Ok(())
}

fn migrate_v1(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS projects (
            id TEXT PRIMARY KEY,
            path TEXT NOT NULL UNIQUE,
            name TEXT NOT NULL,
            git_remote TEXT,
            discovered_at INTEGER NOT NULL,
            last_seen_at INTEGER NOT NULL,
            last_scanned_at INTEGER DEFAULT 0,
            tools TEXT NOT NULL DEFAULT '[]'
        );

        CREATE TABLE IF NOT EXISTS sessions (
            id TEXT NOT NULL,
            source TEXT NOT NULL,
            project_id TEXT NOT NULL REFERENCES projects(id),
            name TEXT,
            mode TEXT,
            model TEXT,
            created_at INTEGER,
            updated_at INTEGER,
            message_count INTEGER DEFAULT 0,
            first_message TEXT,
            indexed_at INTEGER NOT NULL,
            PRIMARY KEY (id, source)
        );

        CREATE TABLE IF NOT EXISTS session_stats (
            session_id TEXT NOT NULL,
            source TEXT NOT NULL,
            model TEXT,
            input_tokens INTEGER,
            output_tokens INTEGER,
            cache_read_tokens INTEGER,
            cache_creation_tokens INTEGER,
            is_estimated INTEGER DEFAULT 0,
            token_source TEXT DEFAULT 'native',
            duration_secs INTEGER,
            files_touched TEXT DEFAULT '[]',
            tool_call_count INTEGER DEFAULT 0,
            computed_at INTEGER NOT NULL,
            PRIMARY KEY (session_id, source),
            FOREIGN KEY (session_id, source) REFERENCES sessions(id, source)
        );

        CREATE TABLE IF NOT EXISTS events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            event TEXT NOT NULL,
            project_id TEXT REFERENCES projects(id),
            timestamp INTEGER NOT NULL,
            data TEXT,
            synced INTEGER DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS project_settings (
            project_id TEXT PRIMARY KEY REFERENCES projects(id),
            settings TEXT NOT NULL DEFAULT '{}'
        );

        CREATE INDEX IF NOT EXISTS idx_sessions_project ON sessions(project_id);
        CREATE INDEX IF NOT EXISTS idx_sessions_source ON sessions(source);
        CREATE INDEX IF NOT EXISTS idx_events_project ON events(project_id);
        CREATE INDEX IF NOT EXISTS idx_events_synced ON events(synced);
        ",
    )
    .map_err(|e| format!("migration v1 failed: {e}"))?;

    Ok(())
}

fn migrate_v2(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "
        -- Per-commit AI attribution from Cursor's ai-code-tracking.db
        CREATE TABLE IF NOT EXISTS ai_commits (
            commit_hash TEXT NOT NULL,
            branch_name TEXT NOT NULL,
            project_id TEXT REFERENCES projects(id),
            commit_message TEXT,
            commit_date TEXT,
            lines_added INTEGER DEFAULT 0,
            lines_deleted INTEGER DEFAULT 0,
            ai_lines_added INTEGER DEFAULT 0,
            ai_lines_deleted INTEGER DEFAULT 0,
            tab_lines_added INTEGER DEFAULT 0,
            tab_lines_deleted INTEGER DEFAULT 0,
            human_lines_added INTEGER DEFAULT 0,
            human_lines_deleted INTEGER DEFAULT 0,
            ai_percentage REAL,
            source TEXT NOT NULL DEFAULT 'cursor',
            ingested_at INTEGER NOT NULL,
            PRIMARY KEY (commit_hash, branch_name)
        );

        CREATE INDEX IF NOT EXISTS idx_ai_commits_project ON ai_commits(project_id);
        CREATE INDEX IF NOT EXISTS idx_ai_commits_date ON ai_commits(commit_date);

        -- OpenTelemetry events from Claude Code and other tools
        CREATE TABLE IF NOT EXISTS otel_events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            event_name TEXT NOT NULL,
            session_id TEXT,
            model TEXT,
            input_tokens INTEGER,
            output_tokens INTEGER,
            cache_read_tokens INTEGER,
            cache_creation_tokens INTEGER,
            duration_ms INTEGER,
            tool_name TEXT,
            tool_success INTEGER,
            prompt_length INTEGER,
            account_uuid TEXT,
            timestamp INTEGER NOT NULL,
            raw_attributes TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_otel_events_session ON otel_events(session_id);
        CREATE INDEX IF NOT EXISTS idx_otel_events_name ON otel_events(event_name);
        CREATE INDEX IF NOT EXISTS idx_otel_events_ts ON otel_events(timestamp);
        ",
    )
    .map_err(|e| format!("migration v2 failed: {e}"))?;

    Ok(())
}

fn migrate_v3(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "
        -- Remote API usage data (Anthropic Admin API, OpenAI Usage API, etc.)
        CREATE TABLE IF NOT EXISTS api_usage (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            source TEXT NOT NULL,
            date TEXT NOT NULL,
            model TEXT NOT NULL DEFAULT '',
            input_tokens INTEGER DEFAULT 0,
            output_tokens INTEGER DEFAULT 0,
            cache_read_tokens INTEGER DEFAULT 0,
            cache_creation_tokens INTEGER DEFAULT 0,
            requests INTEGER DEFAULT 0,
            fetched_at INTEGER NOT NULL,
            UNIQUE(source, date, model)
        );

        CREATE INDEX IF NOT EXISTS idx_api_usage_source ON api_usage(source);
        CREATE INDEX IF NOT EXISTS idx_api_usage_date ON api_usage(date);
        ",
    )
    .map_err(|e| format!("migration v3 failed: {e}"))?;

    Ok(())
}

fn migrate_v4(conn: &Connection) -> Result<(), String> {
    let has_column: bool = conn
        .prepare("PRAGMA table_info(ai_commits)")
        .map(|mut stmt| {
            let names: Vec<String> = stmt
                .query_map([], |row| row.get::<_, String>(1))
                .map(|rows| rows.filter_map(|r| r.ok()).collect())
                .unwrap_or_default();
            names.iter().any(|n| n == "commit_epoch")
        })
        .unwrap_or(false);

    if !has_column {
        conn.execute_batch("ALTER TABLE ai_commits ADD COLUMN commit_epoch INTEGER;")
            .map_err(|e| format!("migration v4 (alter): {e}"))?;
    }

    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS git_activity (
            project_id TEXT NOT NULL REFERENCES projects(id),
            date TEXT NOT NULL,
            commits INTEGER DEFAULT 0,
            lines_added INTEGER DEFAULT 0,
            lines_deleted INTEGER DEFAULT 0,
            files_changed INTEGER DEFAULT 0,
            authors TEXT DEFAULT '[]',
            ai_assisted_commits INTEGER DEFAULT 0,
            ingested_at INTEGER NOT NULL,
            PRIMARY KEY (project_id, date)
        );

        CREATE INDEX IF NOT EXISTS idx_git_activity_date ON git_activity(date);
        CREATE INDEX IF NOT EXISTS idx_git_activity_project ON git_activity(project_id);
        CREATE INDEX IF NOT EXISTS idx_ai_commits_epoch ON ai_commits(commit_epoch);
        ",
    )
    .map_err(|e| format!("migration v4 failed: {e}"))?;

    Ok(())
}

fn migrate_v5(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS anchors (
            commit_hash TEXT PRIMARY KEY,
            branch TEXT,
            author TEXT,
            committed_at INTEGER,
            message TEXT,
            files_changed TEXT DEFAULT '[]',
            lines_added INTEGER DEFAULT 0,
            lines_deleted INTEGER DEFAULT 0,
            session_ids TEXT DEFAULT '[]',
            summary TEXT,
            intent TEXT,
            reasoning TEXT,
            transparency_mode TEXT DEFAULT 'off',
            oobo_version TEXT,
            raw_json TEXT,
            created_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS anchor_sessions (
            commit_hash TEXT NOT NULL REFERENCES anchors(commit_hash),
            session_id TEXT NOT NULL,
            agent TEXT NOT NULL,
            model TEXT,
            link_type TEXT NOT NULL DEFAULT 'inferred',
            input_tokens INTEGER,
            output_tokens INTEGER,
            cache_read_tokens INTEGER,
            cache_creation_tokens INTEGER,
            duration_secs INTEGER,
            tool_calls INTEGER,
            files_touched TEXT,
            is_subagent INTEGER DEFAULT 0,
            PRIMARY KEY (commit_hash, session_id)
        );

        CREATE INDEX IF NOT EXISTS idx_anchors_branch ON anchors(branch);
        CREATE INDEX IF NOT EXISTS idx_anchors_committed_at ON anchors(committed_at);
        CREATE INDEX IF NOT EXISTS idx_anchor_sessions_session ON anchor_sessions(session_id);
        ",
    )
    .map_err(|e| format!("migration v5 failed: {e}"))?;

    Ok(())
}

fn migrate_v6(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS hydration_state (
            project_root TEXT PRIMARY KEY,
            last_hydrated_at INTEGER NOT NULL,
            anchor_count INTEGER NOT NULL DEFAULT 0
        );
        ",
    )
    .map_err(|e| format!("migration v6 failed: {e}"))?;

    Ok(())
}

fn migrate_v7(conn: &Connection) -> Result<(), String> {
    // v7 was originally a duplicate of v5 (anchors + anchor_sessions tables).
    // Since v5 already created them with IF NOT EXISTS, v7 was a no-op.
    // Kept as placeholder to preserve version numbering for existing databases.
    conn.execute_batch(
        "
        CREATE INDEX IF NOT EXISTS idx_sessions_created ON sessions(created_at);
        CREATE INDEX IF NOT EXISTS idx_sessions_updated ON sessions(updated_at);
        CREATE INDEX IF NOT EXISTS idx_events_timestamp ON events(timestamp);
        CREATE INDEX IF NOT EXISTS idx_session_stats_source ON session_stats(source);
        ",
    )
    .map_err(|e| format!("migration v7 failed: {e}"))?;

    Ok(())
}

fn migrate_v8(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "
        ALTER TABLE anchor_sessions ADD COLUMN parent_session_id TEXT;
        ALTER TABLE anchor_sessions ADD COLUMN subagent_type TEXT;
        ",
    )
    .map_err(|e| format!("migration v8 failed: {e}"))?;
    Ok(())
}

/// v9: stable project ids.
///
/// Rewrites `projects.id` (and every referencing table) so the identifier
/// is derived from the canonicalized git remote URL when possible, and
/// falls back to a path-based id otherwise. Projects that point to the
/// same canonical remote are merged: the row with the most recent
/// `last_seen_at` wins and child rows are re-parented.
fn migrate_v9(conn: &Connection) -> Result<(), String> {
    // FKs off for the duration of this transaction; caller (Db::init)
    // flips them back on after `run` returns.
    conn.execute_batch("PRAGMA foreign_keys = OFF;")
        .map_err(|e| format!("v9 fk off: {e}"))?;

    // Canonicalizer duplicated inline to keep migrations self-contained
    // (project.rs may evolve; migration logic must not).
    fn canonicalize_remote(url: &str) -> String {
        let mut s = url.trim().to_string();
        s = s.trim_end_matches(".git").to_string();
        if let Some(at) = s.find('@') {
            if let Some(colon) = s[at..].find(':') {
                let host = &s[at + 1..at + colon];
                let path = &s[at + colon + 1..];
                s = format!("{host}/{path}");
            }
        }
        for scheme in ["https://", "http://", "ssh://", "git://"] {
            if let Some(rest) = s.strip_prefix(scheme) {
                s = rest.to_string();
                break;
            }
        }
        if let Some(rest) = s.strip_prefix("git@") {
            s = rest.to_string();
        }
        s.trim_matches('/').to_lowercase()
    }

    fn path_to_id(path: &str) -> String {
        path.trim_matches('/')
            .replace(['/', '\\'], "-")
            .replace(' ', "_")
    }

    fn derive(remote: Option<&str>, path: &str) -> String {
        match remote {
            Some(r) if !r.trim().is_empty() => format!("r:{}", canonicalize_remote(r)),
            _ => format!("p:{}", path_to_id(path)),
        }
    }

    #[derive(Clone)]
    struct P {
        id: String,
        path: String,
        remote: Option<String>,
        last_seen: i64,
    }

    let mut stmt = conn
        .prepare("SELECT id, path, git_remote, last_seen_at FROM projects")
        .map_err(|e| format!("v9 select projects: {e}"))?;
    let rows: Vec<P> = stmt
        .query_map([], |r| {
            Ok(P {
                id: r.get(0)?,
                path: r.get::<_, String>(1).unwrap_or_default(),
                remote: r.get::<_, Option<String>>(2).unwrap_or(None),
                last_seen: r.get::<_, i64>(3).unwrap_or(0),
            })
        })
        .map_err(|e| format!("v9 query projects: {e}"))?
        .filter_map(Result::ok)
        .collect();
    drop(stmt);

    // Skip rows whose id is already in the new namespace (idempotent reruns
    // e.g. if someone manually bumped then this migration reran).
    let todo: Vec<P> = rows
        .into_iter()
        .filter(|p| !p.id.starts_with("r:") && !p.id.starts_with("p:"))
        .collect();

    if todo.is_empty() {
        return Ok(());
    }

    // Group by new id to detect collisions.
    use std::collections::HashMap;
    let mut groups: HashMap<String, Vec<P>> = HashMap::new();
    for p in todo {
        let new_id = derive(p.remote.as_deref(), &p.path);
        groups.entry(new_id).or_default().push(p);
    }

    let tx = conn
        .unchecked_transaction()
        .map_err(|e| format!("v9 begin tx: {e}"))?;

    for (new_id, mut group) in groups {
        // winner = most recent last_seen; stable on id for determinism.
        group.sort_by(|a, b| b.last_seen.cmp(&a.last_seen).then(a.id.cmp(&b.id)));
        let winner = group.remove(0);
        let losers = group;

        // 1. Re-parent losers' child rows to the winner's old id first.
        for l in &losers {
            tx.execute(
                "UPDATE ai_commits SET project_id = ?1 WHERE project_id = ?2",
                rusqlite::params![&winner.id, &l.id],
            )
            .map_err(|e| format!("v9 ai_commits reparent: {e}"))?;
            tx.execute(
                "UPDATE sessions SET project_id = ?1 WHERE project_id = ?2",
                rusqlite::params![&winner.id, &l.id],
            )
            .map_err(|e| format!("v9 sessions reparent: {e}"))?;
            tx.execute(
                "UPDATE events SET project_id = ?1 WHERE project_id = ?2",
                rusqlite::params![&winner.id, &l.id],
            )
            .map_err(|e| format!("v9 events reparent: {e}"))?;
            // git_activity has PK (project_id, date): INSERT-IGNORE merged
            // rows into the winner, then drop the loser's.
            tx.execute(
                "INSERT OR IGNORE INTO git_activity \
                 (project_id, date, commits, lines_added, lines_deleted, \
                  files_changed, authors, ai_assisted_commits, ingested_at) \
                 SELECT ?1, date, commits, lines_added, lines_deleted, \
                        files_changed, authors, ai_assisted_commits, ingested_at \
                 FROM git_activity WHERE project_id = ?2",
                rusqlite::params![&winner.id, &l.id],
            )
            .map_err(|e| format!("v9 git_activity merge: {e}"))?;
            tx.execute(
                "DELETE FROM git_activity WHERE project_id = ?1",
                [&l.id],
            )
            .map_err(|e| format!("v9 git_activity drop loser: {e}"))?;
            // project_settings: prefer winner's existing row; import loser's
            // only if winner has none.
            tx.execute(
                "INSERT OR IGNORE INTO project_settings (project_id, settings) \
                 SELECT ?1, settings FROM project_settings WHERE project_id = ?2",
                rusqlite::params![&winner.id, &l.id],
            )
            .map_err(|e| format!("v9 settings merge: {e}"))?;
            tx.execute(
                "DELETE FROM project_settings WHERE project_id = ?1",
                [&l.id],
            )
            .map_err(|e| format!("v9 settings drop loser: {e}"))?;
            tx.execute("DELETE FROM projects WHERE id = ?1", [&l.id])
                .map_err(|e| format!("v9 drop loser project: {e}"))?;
        }

        // 2. Rename the winner's id to the new stable id.
        if winner.id != new_id {
            tx.execute(
                "UPDATE ai_commits SET project_id = ?1 WHERE project_id = ?2",
                rusqlite::params![&new_id, &winner.id],
            )
            .map_err(|e| format!("v9 ai_commits rename: {e}"))?;
            tx.execute(
                "UPDATE sessions SET project_id = ?1 WHERE project_id = ?2",
                rusqlite::params![&new_id, &winner.id],
            )
            .map_err(|e| format!("v9 sessions rename: {e}"))?;
            tx.execute(
                "UPDATE events SET project_id = ?1 WHERE project_id = ?2",
                rusqlite::params![&new_id, &winner.id],
            )
            .map_err(|e| format!("v9 events rename: {e}"))?;
            tx.execute(
                "UPDATE git_activity SET project_id = ?1 WHERE project_id = ?2",
                rusqlite::params![&new_id, &winner.id],
            )
            .map_err(|e| format!("v9 git_activity rename: {e}"))?;
            tx.execute(
                "UPDATE project_settings SET project_id = ?1 WHERE project_id = ?2",
                rusqlite::params![&new_id, &winner.id],
            )
            .map_err(|e| format!("v9 settings rename: {e}"))?;
            tx.execute(
                "UPDATE projects SET id = ?1 WHERE id = ?2",
                rusqlite::params![&new_id, &winner.id],
            )
            .map_err(|e| format!("v9 projects rename: {e}"))?;
        }
    }

    tx.commit().map_err(|e| format!("v9 commit: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_migration_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();
        run(&conn).unwrap();
        let version = get_version(&conn).unwrap();
        assert_eq!(version, LATEST_VERSION);
    }

    #[test]
    fn test_migration_v1_creates_tables() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();

        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        assert!(tables.contains(&"projects".to_string()));
        assert!(tables.contains(&"sessions".to_string()));
        assert!(tables.contains(&"session_stats".to_string()));
        assert!(tables.contains(&"events".to_string()));
        assert!(tables.contains(&"project_settings".to_string()));
    }

    #[test]
    fn test_migration_v9_rewrites_ids_by_remote() {
        let conn = Connection::open_in_memory().unwrap();
        // Seed v8 schema.
        ensure_schema_version_table(&conn).unwrap();
        migrate_v1(&conn).unwrap();
        migrate_v2(&conn).unwrap();
        migrate_v3(&conn).unwrap();
        migrate_v4(&conn).unwrap();
        migrate_v5(&conn).unwrap();
        migrate_v6(&conn).unwrap();
        migrate_v7(&conn).unwrap();
        migrate_v8(&conn).unwrap();

        // Path-keyed project with a remote.
        conn.execute(
            "INSERT INTO projects (id, path, name, git_remote, discovered_at, last_seen_at) \
             VALUES ('Users-me-widget', '/Users/me/widget', 'widget', \
                     'git@github.com:acme/widget.git', 1000, 1000)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO project_settings (project_id, settings) VALUES ('Users-me-widget', '{}')",
            [],
        )
        .unwrap();

        migrate_v9(&conn).unwrap();

        let new_id: String = conn
            .query_row(
                "SELECT id FROM projects WHERE path = '/Users/me/widget'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(new_id, "r:github.com/acme/widget");

        let settings_id: String = conn
            .query_row("SELECT project_id FROM project_settings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(settings_id, "r:github.com/acme/widget");
    }

    #[test]
    fn test_migration_v9_merges_collisions() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_schema_version_table(&conn).unwrap();
        migrate_v1(&conn).unwrap();
        migrate_v2(&conn).unwrap();
        migrate_v3(&conn).unwrap();
        migrate_v4(&conn).unwrap();
        migrate_v5(&conn).unwrap();
        migrate_v6(&conn).unwrap();
        migrate_v7(&conn).unwrap();
        migrate_v8(&conn).unwrap();

        // Two legacy rows pointing to the same canonical remote (SSH vs HTTPS).
        conn.execute(
            "INSERT INTO projects (id, path, name, git_remote, discovered_at, last_seen_at) \
             VALUES ('old-a', '/Users/me/widget-a', 'widget', \
                     'git@github.com:acme/widget.git', 1000, 2000)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO projects (id, path, name, git_remote, discovered_at, last_seen_at) \
             VALUES ('old-b', '/Users/me/widget-b', 'widget', \
                     'https://github.com/acme/widget.git', 1000, 1000)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ai_commits (commit_hash, branch_name, project_id, source, ingested_at) \
             VALUES ('h1', 'main', 'old-a', 'cursor', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ai_commits (commit_hash, branch_name, project_id, source, ingested_at) \
             VALUES ('h2', 'main', 'old-b', 'cursor', 1)",
            [],
        )
        .unwrap();

        migrate_v9(&conn).unwrap();

        let proj_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM projects", [], |r| r.get(0))
            .unwrap();
        assert_eq!(proj_count, 1, "collisions should merge to a single row");

        let remaining_id: String = conn
            .query_row("SELECT id FROM projects", [], |r| r.get(0))
            .unwrap();
        assert_eq!(remaining_id, "r:github.com/acme/widget");

        let reparented: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM ai_commits WHERE project_id = 'r:github.com/acme/widget'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(reparented, 2, "both ai_commits should re-parent");
    }

    #[test]
    fn test_migration_v9_idempotent_on_rerun() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();
        conn.execute(
            "INSERT INTO projects (id, path, name, git_remote, discovered_at, last_seen_at) \
             VALUES ('r:github.com/acme/already', '/p', 'p', \
                     'git@github.com:acme/already.git', 1, 1)",
            [],
        )
        .unwrap();
        // Second run should not touch the already-stable id.
        migrate_v9(&conn).unwrap();
        let id: String = conn
            .query_row("SELECT id FROM projects WHERE path = '/p'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(id, "r:github.com/acme/already");
    }

    #[test]
    fn test_migration_v7_creates_indexes() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();

        let indexes: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='index' AND name LIKE 'idx_%' ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        assert!(indexes.contains(&"idx_sessions_created".to_string()));
        assert!(indexes.contains(&"idx_sessions_updated".to_string()));
        assert!(indexes.contains(&"idx_events_timestamp".to_string()));
        assert!(indexes.contains(&"idx_session_stats_source".to_string()));
    }
}

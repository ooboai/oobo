use rusqlite::Connection;

const LATEST_VERSION: i32 = 7;

pub fn run(conn: &Connection) -> Result<(), String> {
    ensure_schema_version_table(conn)?;

    let current = get_version(conn)?;
    if current >= LATEST_VERSION {
        return Ok(());
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

    set_version(conn, LATEST_VERSION)?;
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
    .map_err(|e| format!("migration v7 failed: {e}"))?;

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
}

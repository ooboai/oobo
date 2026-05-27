use rusqlite::Connection;
use std::path::Path;

const LATEST_VERSION: i32 = 13;

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
    if current < 10 {
        migrate_v10(conn)?;
    }
    if current < 11 {
        migrate_v11(conn)?;
    }
    if current < 12 {
        migrate_v12(conn)?;
    }
    if current < 13 {
        migrate_v13(conn)?;
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

/// v10: hook_sessions table. Moves per-session hook state (previously in
/// `.git/oobo-sessions/<sid>.json`) into the DB. Existing legacy files
/// are NOT touched by the migration itself  --  they're lazily imported on
/// the first read/write of each session (see `hooks::store`).
fn migrate_v10(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS hook_sessions (
            project_id TEXT NOT NULL,
            session_id TEXT NOT NULL,
            payload    TEXT NOT NULL,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (project_id, session_id)
        );

        CREATE INDEX IF NOT EXISTS idx_hook_sessions_updated
            ON hook_sessions(updated_at);
        CREATE INDEX IF NOT EXISTS idx_hook_sessions_session
            ON hook_sessions(session_id);
        ",
    )
    .map_err(|e| format!("migration v10 failed: {e}"))?;
    Ok(())
}

/// v11: turn-level capture + delta-based anchor contributions.
///
/// This is the big structural shift that turns oobo from a
/// "session-per-commit snapshot" store into a first-principles
/// "anchor = commit + work done since last anchor" store.
///
/// ### What changes conceptually
///
/// - `turns` is the new atomic unit of capture. Every assistant reply,
///   with its **per-call** token counts exactly as the model billed,
///   lives here. No cumulative sums anywhere in storage.
/// - `sessions` gains `parent_session_id` / `parent_source` /
///   `parent_turn_id` / `subagent_kind` so subagents are first-class
///   children of their spawning turn.
/// - `anchor_contributions` replaces the intent of `anchor_sessions`:
///   one row per (commit, session) storing the **window of turns**
///   `[first_turn_index, last_turn_index]` that produced the commit,
///   with denormalized deltas. Never cumulative.
/// - `anchor_contribution_files` records per-(commit, session) file
///   attributions so we can answer "which session wrote which file".
/// - Views `v_turn_spend`, `v_session_totals`, `v_anchor_totals`,
///   `v_project_totals` expose the one canonical definition of
///   "tokens" that every reader must go through.
///
/// ### What does NOT change
///
/// - `anchors` (shape + orphan-branch semantics)
/// - `anchor_sessions` and `actions` / `action_sessions` remain in place
///   for one release as legacy readers; they are not written to anymore
///   once the L3 attribution path lands. v12 will drop them.
/// - `projects`, `ai_commits`, `events`, `hook_sessions`, all other
///   runtime/analytics tables.
///
/// ### Why additive (not destructive)
///
/// Rollback is trivial: flipping the LATEST_VERSION back and dropping
/// the new objects restores the prior behaviour completely. The
/// attribution switchover happens in later commits; this migration is
/// schema-only so it's safe to ship in isolation.
fn migrate_v11(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "
        ---------------------------------------------------------------
        -- turns: atomic unit of LLM capture. One row per model call.
        ---------------------------------------------------------------
        CREATE TABLE IF NOT EXISTS turns (
            id                      TEXT PRIMARY KEY,
            session_id              TEXT NOT NULL,
            source                  TEXT NOT NULL,
            turn_index              INTEGER NOT NULL,
            role                    TEXT,     -- user | assistant | tool | system
            started_at              INTEGER,
            ended_at                INTEGER,
            model                   TEXT,
            -- Per-call token deltas (exactly as the model API reports them).
            input_tokens            INTEGER,  -- non-cached prompt tokens THIS call
            cache_read_tokens       INTEGER,  -- cached prompt tokens read THIS call
            cache_creation_tokens   INTEGER,  -- tokens written to cache THIS call
            output_tokens           INTEGER,  -- generated tokens THIS call
            cost_usd                REAL,     -- computed from model pricing
            tool_call_count         INTEGER DEFAULT 0,
            thinking_ms             INTEGER,
            message_preview         TEXT,     -- redacted snippet for search/UI
            raw_ref                 TEXT,     -- pointer back to source artifact
            ingested_at             INTEGER NOT NULL,
            UNIQUE (session_id, source, turn_index)
        );

        CREATE INDEX IF NOT EXISTS idx_turns_session
            ON turns(session_id, source, turn_index);
        CREATE INDEX IF NOT EXISTS idx_turns_started
            ON turns(started_at);
        CREATE INDEX IF NOT EXISTS idx_turns_ended
            ON turns(ended_at);

        ---------------------------------------------------------------
        -- anchor_contributions: one row per (commit, session), storing
        -- the DELTA of work done between the previous anchor for this
        -- session and this commit. Never cumulative.
        ---------------------------------------------------------------
        CREATE TABLE IF NOT EXISTS anchor_contributions (
            commit_hash            TEXT NOT NULL REFERENCES anchors(commit_hash),
            session_id             TEXT NOT NULL,
            source                 TEXT NOT NULL,
            link_type              TEXT NOT NULL DEFAULT 'inferred',
            -- [first_turn_index, last_turn_index] inclusive: the window
            -- of session turns that contributed to this commit.
            first_turn_index       INTEGER NOT NULL,
            last_turn_index        INTEGER NOT NULL,
            -- Denormalized totals across [first, last]. Always deltas.
            input_tokens           INTEGER,
            cache_read_tokens      INTEGER,
            cache_creation_tokens  INTEGER,
            output_tokens          INTEGER,
            cost_usd               REAL,
            tool_call_count        INTEGER,
            duration_secs          INTEGER,
            is_subagent            INTEGER NOT NULL DEFAULT 0,
            parent_session_id      TEXT,
            parent_source          TEXT,
            subagent_kind          TEXT,
            PRIMARY KEY (commit_hash, session_id, source)
        );

        CREATE INDEX IF NOT EXISTS idx_contributions_session
            ON anchor_contributions(session_id, source);
        CREATE INDEX IF NOT EXISTS idx_contributions_parent
            ON anchor_contributions(parent_session_id, parent_source);

        ---------------------------------------------------------------
        -- anchor_contribution_files: per-(commit, session) file-level
        -- attribution. Lets the TUI answer 'which session wrote which
        -- file in this commit'.
        ---------------------------------------------------------------
        CREATE TABLE IF NOT EXISTS anchor_contribution_files (
            commit_hash   TEXT NOT NULL,
            session_id    TEXT NOT NULL,
            source        TEXT NOT NULL,
            path          TEXT NOT NULL,
            lines_added   INTEGER DEFAULT 0,
            lines_deleted INTEGER DEFAULT 0,
            PRIMARY KEY (commit_hash, session_id, source, path)
        );

        CREATE INDEX IF NOT EXISTS idx_contrib_files_commit
            ON anchor_contribution_files(commit_hash);
        ",
    )
    .map_err(|e| format!("migration v11 (new tables): {e}"))?;

    // Add subagent hierarchy columns to `sessions`. SQLite cannot add a
    // column "IF NOT EXISTS"; do a feature-test via PRAGMA for idempotency.
    for (col, def) in [
        ("parent_session_id", "TEXT"),
        ("parent_source",     "TEXT"),
        ("parent_turn_id",    "TEXT"),
        ("subagent_kind",     "TEXT"),
    ] {
        if !column_exists(conn, "sessions", col)? {
            conn.execute_batch(&format!(
                "ALTER TABLE sessions ADD COLUMN {col} {def};"
            ))
            .map_err(|e| format!("migration v11 (alter sessions.{col}): {e}"))?;
        }
    }

    conn.execute_batch(
        "
        CREATE INDEX IF NOT EXISTS idx_sessions_parent
            ON sessions(parent_session_id, parent_source);

        ---------------------------------------------------------------
        -- CANONICAL VIEWS: the ONLY legitimate source of 'how many
        -- tokens'. Every reader must go through these. Writing raw
        -- SUM() aggregations elsewhere in the codebase is a lint bug.
        ---------------------------------------------------------------

        -- Per-turn billed tokens: exactly what the API charged for
        -- this single call (no sliding cumulative).
        DROP VIEW IF EXISTS v_turn_spend;
        CREATE VIEW v_turn_spend AS
        SELECT
            t.id,
            t.session_id,
            t.source,
            t.turn_index,
            t.model,
            t.role,
            t.started_at,
            t.ended_at,
            COALESCE(t.input_tokens,          0)
              + COALESCE(t.cache_read_tokens,    0)
              + COALESCE(t.cache_creation_tokens, 0)
              + COALESCE(t.output_tokens,        0)  AS billed_tokens,
            -- 'New work' = output + cache_creation (novel content produced).
            COALESCE(t.output_tokens,        0)
              + COALESCE(t.cache_creation_tokens, 0) AS new_work_tokens,
            -- 'Context' = input + cache_read (size of conversation this call).
            COALESCE(t.input_tokens,       0)
              + COALESCE(t.cache_read_tokens, 0)     AS context_tokens,
            t.cost_usd,
            t.tool_call_count
        FROM turns t;

        DROP VIEW IF EXISTS v_session_totals;
        CREATE VIEW v_session_totals AS
        SELECT
            vs.session_id,
            vs.source,
            COUNT(*)                      AS turns,
            SUM(vs.billed_tokens)         AS billed_tokens,
            SUM(vs.new_work_tokens)       AS new_work_tokens,
            MAX(vs.context_tokens)        AS max_context_tokens,
            SUM(COALESCE(vs.cost_usd, 0)) AS cost_usd,
            SUM(COALESCE(vs.tool_call_count, 0)) AS tool_calls,
            MIN(vs.started_at)            AS first_turn_at,
            MAX(COALESCE(vs.ended_at, vs.started_at)) AS last_turn_at
        FROM v_turn_spend vs
        GROUP BY vs.session_id, vs.source;

        DROP VIEW IF EXISTS v_anchor_totals;
        CREATE VIEW v_anchor_totals AS
        SELECT
            ac.commit_hash,
            COUNT(*)                                    AS contributing_sessions,
            SUM(CASE WHEN ac.is_subagent = 0 THEN 1 ELSE 0 END) AS top_level_sessions,
            SUM(CASE WHEN ac.is_subagent = 1 THEN 1 ELSE 0 END) AS subagents,
            SUM(COALESCE(ac.input_tokens,0)
              + COALESCE(ac.cache_read_tokens,0)
              + COALESCE(ac.cache_creation_tokens,0)
              + COALESCE(ac.output_tokens,0))           AS billed_tokens,
            SUM(COALESCE(ac.output_tokens,0)
              + COALESCE(ac.cache_creation_tokens,0))   AS new_work_tokens,
            SUM(COALESCE(ac.cost_usd, 0))               AS cost_usd,
            SUM(COALESCE(ac.tool_call_count, 0))        AS tool_calls,
            SUM(COALESCE(ac.duration_secs, 0))          AS duration_secs
        FROM anchor_contributions ac
        GROUP BY ac.commit_hash;

        DROP VIEW IF EXISTS v_project_totals;
        CREATE VIEW v_project_totals AS
        SELECT
            s.project_id,
            COUNT(DISTINCT s.id || '|' || s.source)   AS sessions,
            COALESCE(SUM(vst.turns), 0)               AS turns,
            COALESCE(SUM(vst.billed_tokens), 0)       AS billed_tokens,
            COALESCE(SUM(vst.new_work_tokens), 0)     AS new_work_tokens,
            COALESCE(SUM(vst.cost_usd), 0)            AS cost_usd,
            COALESCE(MAX(vst.last_turn_at), 0)        AS last_turn_at
        FROM sessions s
        LEFT JOIN v_session_totals vst
               ON vst.session_id = s.id AND vst.source = s.source
        GROUP BY s.project_id;
        ",
    )
    .map_err(|e| format!("migration v11 (views): {e}"))?;

    Ok(())
}

/// v12  --  Subagent inference substrate.
///
/// Two tiny additions enable heuristic parent/child detection for
/// sessions whose tool didn't expose the link explicitly:
///
/// 1. `turns.tool_names`  --  comma-joined list of tool_use names per
///    turn (e.g. `"Task"`, `"Read,Write"`). Makes it O(1) to locate
///    every parent turn that spawned a subagent without re-parsing
///    transcripts. NULL for turns without tool_use.
///
/// 2. `subagent_inferences`  --  immutable audit log for every
///    inference decision. Records which signals fired, the combined
///    score, and whether the link was applied. This preserves the
///    reasoning so a future run (or a human) can revisit borderline
///    cases without losing history.
fn migrate_v12(conn: &Connection) -> Result<(), String> {
    if !column_exists(conn, "turns", "tool_names")? {
        conn.execute_batch("ALTER TABLE turns ADD COLUMN tool_names TEXT;")
            .map_err(|e| format!("migration v12 (turns.tool_names): {e}"))?;
    }

    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS subagent_inferences (
            -- The orphan child that was evaluated.
            child_session_id    TEXT NOT NULL,
            child_source        TEXT NOT NULL,

            -- The parent we matched (or considered). Null when the
            -- row records a rejected evaluation with no top candidate.
            parent_session_id   TEXT,
            parent_source       TEXT,
            parent_turn_id      TEXT,

            -- Inferred subagent kind (e.g. 'task', 'explore', 'planner').
            -- Derived from Task tool args or template preamble.
            subagent_kind       TEXT,

            -- [0.0, 1.0]. Applied when score >= 0.6.
            score               REAL NOT NULL,

            -- JSON array of signal hits, e.g.
            --   [{\"kind\":\"task_tool_temporal\",\"weight\":0.7,\"gap_ms\":1820},
            --    {\"kind\":\"template_preamble\",\"weight\":0.4,\"match\":\"You are a task-focused agent\"}]
            signals_json        TEXT NOT NULL,

            -- True when we wrote parent_* fields back to sessions.
            applied             INTEGER NOT NULL DEFAULT 0,

            decided_at          INTEGER NOT NULL,

            PRIMARY KEY (child_session_id, child_source, decided_at)
        );

        CREATE INDEX IF NOT EXISTS idx_subagent_inferences_applied
            ON subagent_inferences(applied, decided_at DESC);
        ",
    )
    .map_err(|e| format!("migration v12 (subagent_inferences): {e}"))?;

    // oobo_state: tiny key/value table for cross-run flags that
    // don't belong in user settings (e.g. "backfill pending after
    // schema bump"). Keeping it separate from `project_settings`
    // means ephemeral operational state stays out of user-visible
    // config.
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS oobo_state (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        ",
    )
    .map_err(|e| format!("migration v12 (oobo_state): {e}"))?;

    // Mark that a full backfill is required. Any run at v12+ that
    // sees this flag will rebuild turns + contributions from the
    // native tool artifacts and clear the flag on success. This is
    // how we replace the old `oobo _rebuild` command: the upgrade
    // itself arms the trigger, and the very next invocation does
    // the work  --  once.
    conn.execute(
        "INSERT OR REPLACE INTO oobo_state (key, value) VALUES ('backfill_pending', '1')",
        [],
    )
    .map_err(|e| format!("migration v12 (arm backfill flag): {e}"))?;

    Ok(())
}

/// v13  --  Robust project identity + legacy drain.
///
/// 1. Adds `initial_commit_sha` and `historical_paths` columns to
///    `projects`. Together with the existing `git_remote`, projects
///    now survive folder renames even when there's no remote.
///
/// 2. Restructures `hook_sessions` into `active_sessions` with
///    first-class `tool`, `started_at`, `parent_session_id` columns
///    (no more opaque payload blob for key lookups).
///
/// 3. Arms a `drain_legacy_pending` flag so the next invocation walks
///    all known projects and imports `.git/oobo-sessions/*.json` files
///    into the DB, then deletes the marker files.
fn migrate_v13(conn: &Connection) -> Result<(), String> {
    // ── 1. projects: add initial_commit_sha + historical_paths ──
    if !column_exists(conn, "projects", "initial_commit_sha")? {
        conn.execute_batch("ALTER TABLE projects ADD COLUMN initial_commit_sha TEXT;")
            .map_err(|e| format!("v13 projects.initial_commit_sha: {e}"))?;
    }
    if !column_exists(conn, "projects", "historical_paths")? {
        conn.execute_batch(
            "ALTER TABLE projects ADD COLUMN historical_paths TEXT NOT NULL DEFAULT '[]';",
        )
        .map_err(|e| format!("v13 projects.historical_paths: {e}"))?;
    }

    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_projects_initial_sha
             ON projects(initial_commit_sha);",
    )
    .map_err(|e| format!("v13 idx initial_sha: {e}"))?;

    // ── 2. active_sessions: replaces hook_sessions ──
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS active_sessions (
            session_id        TEXT NOT NULL,
            project_id        TEXT NOT NULL,
            tool              TEXT NOT NULL,
            started_at        INTEGER NOT NULL,
            last_event_at     INTEGER NOT NULL,
            state_json        TEXT NOT NULL,
            parent_session_id TEXT,
            PRIMARY KEY (project_id, session_id)
        );
        CREATE INDEX IF NOT EXISTS idx_active_sessions_project
            ON active_sessions(project_id);
        CREATE INDEX IF NOT EXISTS idx_active_sessions_updated
            ON active_sessions(last_event_at);
        ",
    )
    .map_err(|e| format!("v13 active_sessions: {e}"))?;

    // Migrate existing hook_sessions rows into the new table.
    let has_hook_sessions = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='hook_sessions'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
        > 0;

    if has_hook_sessions {
        let mut stmt = conn
            .prepare("SELECT project_id, session_id, payload, updated_at FROM hook_sessions")
            .map_err(|e| format!("v13 read hook_sessions: {e}"))?;
        let rows: Vec<(String, String, String, i64)> = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            })
            .map_err(|e| format!("v13 iter hook_sessions: {e}"))?
            .filter_map(|r| r.ok())
            .collect();

        for (pid, sid, payload, updated_at) in &rows {
            let (tool, started_at, parent_sid) = extract_from_payload(payload);
            conn.execute(
                "INSERT OR IGNORE INTO active_sessions \
                 (session_id, project_id, tool, started_at, last_event_at, \
                  state_json, parent_session_id) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    sid,
                    pid,
                    tool,
                    started_at,
                    updated_at,
                    payload,
                    parent_sid,
                ],
            )
            .map_err(|e| format!("v13 insert active_session: {e}"))?;
        }
    }

    // ── 3. Ensure oobo_state exists + arm the legacy-drain flag ──
    // v12 creates oobo_state, but users already at v12 before that code
    // landed never got it. CREATE IF NOT EXISTS is safe either way.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS oobo_state (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );",
    )
    .map_err(|e| format!("v13 oobo_state ensure: {e}"))?;

    conn.execute(
        "INSERT OR REPLACE INTO oobo_state (key, value) VALUES ('drain_legacy_pending', '1')",
        [],
    )
    .map_err(|e| format!("v13 arm drain flag: {e}"))?;

    Ok(())
}

fn extract_from_payload(json: &str) -> (String, i64, Option<String>) {
    let v: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
    let tool = v["agent"]
        .as_str()
        .unwrap_or("unknown")
        .to_string();
    let started = v["started_at"].as_i64().unwrap_or(0);
    let parent = v["parent_session_id"]
        .as_str()
        .map(|s| s.to_string());
    (tool, started, parent)
}

/// True if `table` has a column named `col`. SQLite has no native
/// `ALTER TABLE ... ADD COLUMN IF NOT EXISTS`; this is the standard
/// feature-test workaround via `PRAGMA table_info`.
fn column_exists(conn: &Connection, table: &str, col: &str) -> Result<bool, String> {
    let sql = format!("PRAGMA table_info({table})");
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("pragma {table}: {e}"))?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .map_err(|e| format!("pragma {table} iter: {e}"))?;
    for name in rows.flatten() {
        if name == col {
            return Ok(true);
        }
    }
    Ok(false)
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

    #[test]
    fn test_migration_v11_creates_objects() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();

        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(tables.contains(&"turns".to_string()), "turns missing");
        assert!(
            tables.contains(&"anchor_contributions".to_string()),
            "anchor_contributions missing"
        );
        assert!(
            tables.contains(&"anchor_contribution_files".to_string()),
            "anchor_contribution_files missing"
        );

        let views: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='view' ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        for v in [
            "v_turn_spend",
            "v_session_totals",
            "v_anchor_totals",
            "v_project_totals",
        ] {
            assert!(views.contains(&v.to_string()), "view {v} missing");
        }

        assert!(column_exists(&conn, "sessions", "parent_session_id").unwrap());
        assert!(column_exists(&conn, "sessions", "parent_source").unwrap());
        assert!(column_exists(&conn, "sessions", "parent_turn_id").unwrap());
        assert!(column_exists(&conn, "sessions", "subagent_kind").unwrap());
    }

    #[test]
    fn test_v11_views_roll_up_deltas_not_cumulative() {
        // This is THE property that 'anchor_sessions' historically got
        // wrong: storing cumulative session totals per commit, causing
        // project aggregates to multiply real work by commit count.
        //
        // With the new schema, each turn is a delta and contributions
        // reference a *window* of turns. Summing contributions yields
        // exactly SUM(turns_in_window) with no double counting.
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();

        // Seed a minimal project + session.
        conn.execute(
            "INSERT INTO projects (id, path, name, discovered_at, last_seen_at) \
             VALUES ('r:gh/acme/p', '/tmp/p', 'p', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, source, project_id, indexed_at) \
             VALUES ('s1', 'claude', 'r:gh/acme/p', 1)",
            [],
        )
        .unwrap();

        // 3 turns, 100 billed tokens each.
        for i in 0..3i64 {
            conn.execute(
                "INSERT INTO turns (id, session_id, source, turn_index, output_tokens, \
                 input_tokens, ingested_at) VALUES (?1, 's1', 'claude', ?2, 60, 40, 1)",
                rusqlite::params![format!("t{i}"), i],
            )
            .unwrap();
        }

        // Two anchors. First covers turns 0..=0; second covers 1..=2.
        conn.execute(
            "INSERT INTO anchors (commit_hash, created_at) VALUES ('c1', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO anchors (commit_hash, created_at) VALUES ('c2', 2)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO anchor_contributions \
             (commit_hash, session_id, source, first_turn_index, last_turn_index, \
              input_tokens, output_tokens) \
             VALUES ('c1', 's1', 'claude', 0, 0, 40, 60),\
                    ('c2', 's1', 'claude', 1, 2, 80, 120)",
            [],
        )
        .unwrap();

        let session_billed: i64 = conn
            .query_row(
                "SELECT billed_tokens FROM v_session_totals WHERE session_id='s1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(session_billed, 300, "3 turns * 100 = 300, no dup");

        let c1_billed: i64 = conn
            .query_row(
                "SELECT billed_tokens FROM v_anchor_totals WHERE commit_hash='c1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let c2_billed: i64 = conn
            .query_row(
                "SELECT billed_tokens FROM v_anchor_totals WHERE commit_hash='c2'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(c1_billed + c2_billed, session_billed,
            "sum of per-anchor deltas must equal session total exactly");

        let project_billed: i64 = conn
            .query_row(
                "SELECT billed_tokens FROM v_project_totals WHERE project_id='r:gh/acme/p'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(project_billed, session_billed,
            "project total == session total; no double counting across commits");
    }

    #[test]
    fn test_v11_idempotent_rerun() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();
        // Calling migrate_v11 again directly must be a no-op (ALTER guarded).
        migrate_v11(&conn).unwrap();
        assert!(column_exists(&conn, "sessions", "parent_session_id").unwrap());
    }
}

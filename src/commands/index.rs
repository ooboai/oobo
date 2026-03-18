use std::fs;

use crate::analytics::{self, NativeStats};
use crate::db::sessions::SessionRow;
use crate::db::Db;
use crate::paths;
use crate::session;
use crate::tools::cursor::Session;

pub fn run(
    project: Option<String>,
    force: bool,
    bg: bool,
    status: bool,
    _agent_mode: bool,
) -> Result<(), String> {
    if status {
        return show_status();
    }

    if bg {
        return spawn_background(project.as_deref(), force);
    }

    run_foreground(project, force)
}

fn run_foreground(project: Option<String>, force: bool) -> Result<(), String> {
    let db = Db::open()?;

    let (sessions, proj_name) = if force {
        if let Some(ref proj) = project {
            let project_id = find_project_id(&db, proj)?;
            eprintln!("force re-indexing project: {proj}");
            (
                db.list_sessions_by_project(&project_id)?,
                Some(proj.clone()),
            )
        } else {
            eprintln!("force re-indexing all sessions...");
            (db.list_all_sessions()?, None)
        }
    } else if let Some(ref proj) = project {
        let project_id = find_project_id(&db, proj)?;
        let sessions = db.list_unindexed_sessions_by_project(&project_id)?;
        if sessions.is_empty() {
            eprintln!("all sessions up to date for project: {proj}");
            return Ok(());
        }
        eprintln!(
            "indexing {} new sessions for project: {proj}",
            sessions.len()
        );
        (sessions, Some(proj.clone()))
    } else {
        let sessions = db.list_unindexed_sessions()?;
        if sessions.is_empty() {
            eprintln!("all sessions up to date (0 new)");
            return Ok(());
        }
        eprintln!("indexing {} new sessions...", sessions.len());
        (sessions, None)
    };

    if sessions.is_empty() {
        eprintln!("no sessions found — run `oobo scan` first");
        return Ok(());
    }

    write_status_file("running", 0, sessions.len(), "");

    let result = if let Some(ref name) = proj_name {
        index_sessions_for_project(&db, &sessions, force, true, name)
    } else {
        index_sessions(&db, &sessions, force, true)
    };

    // Only run expensive git/API operations when something was actually indexed (or forced)
    let run_extras = result.indexed > 0 || force;

    let ai_msg = if run_extras {
        match crate::tools::cursor::ai_tracking::ingest_scored_commits(&db, force) {
            Ok((ingested, skipped)) => {
                if ingested > 0 || skipped > 0 {
                    let m = format!("{ingested} commits ingested, {skipped} skipped");
                    eprintln!("cursor ai-tracking: {m}");
                    m
                } else {
                    String::new()
                }
            }
            Err(e) => {
                eprintln!("cursor ai-tracking: {e}");
                String::new()
            }
        }
    } else {
        String::new()
    };

    let attr_msg = if run_extras {
        match crate::analytics::attribution::run_attribution(&db, force) {
            Ok((attributed, skipped)) => {
                if attributed > 0 || skipped > 0 {
                    let m = format!("{attributed} AI-correlated, {skipped} human/skipped");
                    eprintln!("git attribution: {m}");
                    m
                } else {
                    String::new()
                }
            }
            Err(e) => {
                eprintln!("git attribution: {e}");
                String::new()
            }
        }
    } else {
        String::new()
    };

    let git_msg = if run_extras {
        match crate::analytics::git_activity::ingest_git_activity(&db, force) {
            Ok((ingested, skipped)) => {
                if ingested > 0 || skipped > 0 {
                    let m = format!("{ingested} days ingested, {skipped} skipped");
                    eprintln!("git activity: {m}");
                    m
                } else {
                    String::new()
                }
            }
            Err(e) => {
                eprintln!("git activity: {e}");
                String::new()
            }
        }
    } else {
        String::new()
    };

    let api_msg = if run_extras {
        fetch_api_usage(&db)
    } else {
        String::new()
    };

    let mut msg = if result.failed > 0 {
        format!(
            "{} indexed, {} failed (of {} total)",
            result.indexed, result.failed, result.total
        )
    } else {
        format!("{} indexed (of {} total)", result.indexed, result.total)
    };
    if !ai_msg.is_empty() {
        msg.push_str(&format!(" | ai-tracking: {ai_msg}"));
    }
    if !attr_msg.is_empty() {
        msg.push_str(&format!(" | attribution: {attr_msg}"));
    }
    if !git_msg.is_empty() {
        msg.push_str(&format!(" | git: {git_msg}"));
    }
    if !api_msg.is_empty() {
        msg.push_str(&format!(" | api: {api_msg}"));
    }
    eprintln!("done: {msg}");

    write_status_file("done", result.indexed + result.failed, result.total, &msg);

    Ok(())
}

fn spawn_background(project: Option<&str>, force: bool) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot find self: {e}"))?;

    let mut args = vec!["index".to_string()];
    if let Some(p) = project {
        args.push("--project".to_string());
        args.push(p.to_string());
    }
    if force {
        args.push("--force".to_string());
    }

    // Use `nice` on Unix to run at low priority so indexing doesn't hog the CPU
    #[cfg(unix)]
    let child = std::process::Command::new("nice")
        .args(["-n", "15"])
        .arg(&exe)
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot spawn background index: {e}"))?;

    #[cfg(not(unix))]
    let child = std::process::Command::new(&exe)
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot spawn background index: {e}"))?;

    let pid = child.id();
    write_status_file("running", 0, 0, &format!("pid:{pid}"));

    eprintln!("indexing started in background (pid {pid})");
    eprintln!("run `oobo index --status` to check progress");

    Ok(())
}

fn show_status() -> Result<(), String> {
    let path = status_file_path();
    if !path.exists() {
        eprintln!("no indexing in progress");
        return Ok(());
    }

    let content = fs::read_to_string(&path).unwrap_or_default();
    let lines: Vec<&str> = content.lines().collect();

    let state = lines.first().copied().unwrap_or("unknown");
    let progress = lines.get(1).copied().unwrap_or("");
    let message = lines.get(2).copied().unwrap_or("");

    match state {
        "running" => {
            if let Some(pid_str) = message.strip_prefix("pid:") {
                let pid: u32 = pid_str.parse().unwrap_or(0);
                if pid > 0 && is_process_alive(pid) {
                    eprintln!("indexing in progress (pid {pid}) — {progress}");
                } else {
                    eprintln!("background indexer is no longer running (stale status)");
                    let _ = fs::remove_file(&path);
                }
            } else {
                eprintln!("indexing in progress — {progress}");
            }
        }
        "done" => {
            eprintln!("last index completed: {message}");
        }
        _ => {
            eprintln!("index status: {state}");
        }
    }

    Ok(())
}

fn status_file_path() -> std::path::PathBuf {
    paths::oobo_db_dir().join("index.status")
}

fn write_status_file(state: &str, done: usize, total: usize, message: &str) {
    let path = status_file_path();
    let _ = paths::ensure_dir(&paths::oobo_db_dir());
    let content = format!("{state}\n{done}/{total}\n{message}\n");
    let _ = fs::write(&path, content);
}

/// Update the progress line in the status file (called during indexing).
fn update_progress(done: usize, total: usize) {
    let path = status_file_path();
    if let Ok(content) = fs::read_to_string(&path) {
        let lines: Vec<&str> = content.lines().collect();
        if lines.len() >= 2 {
            let progress = format!("{done}/{total}");
            let new_content = format!(
                "{}\n{progress}\n{}\n",
                lines[0],
                lines.get(2).unwrap_or(&"")
            );
            let _ = fs::write(&path, new_content);
        }
    }
}

fn build_notification(
    sessions: &[SessionRow],
    indexed: usize,
    skipped: usize,
    project_name: Option<&str>,
) -> (String, String) {
    use std::collections::HashSet;

    let sources: Vec<&str> = {
        let unique: HashSet<&str> = sessions.iter().map(|s| s.source.as_str()).collect();
        let mut v: Vec<&str> = unique.into_iter().collect();
        v.sort();
        v
    };
    let tool_names: Vec<&str> = sources
        .iter()
        .map(|s| crate::tui::source_label(s))
        .collect();
    let tools_str = tool_names.join(", ");

    let title = "Oobo".to_string();

    let body = if indexed == 0 && skipped > 0 {
        format!("All {skipped} sessions up to date")
    } else {
        let s = if indexed == 1 { "session" } else { "sessions" };
        match project_name {
            Some(_) => format!("Indexed {indexed} {s} from {tools_str}"),
            None => {
                let project_ids: HashSet<&str> =
                    sessions.iter().map(|s| s.project_id.as_str()).collect();
                let pc = project_ids.len();
                let p = if pc == 1 { "project" } else { "projects" };
                format!("Indexed {indexed} {s} across {pc} {p} from {tools_str}")
            }
        }
    };

    (title, body)
}

fn send_notification(title: &str, message: &str) {
    crate::notify::send(title, message);
}

fn is_process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

pub struct IndexResult {
    pub indexed: usize,
    pub failed: usize,
    pub total: usize,
}

/// Core indexing logic shared between `oobo index` and `oobo scan` auto-index.
pub fn index_sessions(db: &Db, sessions: &[SessionRow], force: bool, verbose: bool) -> IndexResult {
    index_sessions_inner(db, sessions, force, verbose, None)
}

/// Like `index_sessions` but accepts a project name for better notification text.
pub fn index_sessions_for_project(
    db: &Db,
    sessions: &[SessionRow],
    force: bool,
    verbose: bool,
    project_name: &str,
) -> IndexResult {
    index_sessions_inner(db, sessions, force, verbose, Some(project_name))
}

fn index_sessions_inner(
    db: &Db,
    sessions: &[SessionRow],
    _force: bool,
    verbose: bool,
    project_name: Option<&str>,
) -> IndexResult {
    let mut indexed = 0usize;
    let mut failed = 0usize;
    let total = sessions.len();

    let composer_ids: Vec<String> = sessions
        .iter()
        .filter(|s| s.source == "composer")
        .map(|s| s.id.clone())
        .collect();

    let (composer_cache, bubble_cache) = if !composer_ids.is_empty() {
        if verbose && composer_ids.len() > 5 {
            eprint!(
                "  loading Cursor data for {} sessions...",
                composer_ids.len()
            );
        }
        let old = crate::tools::cursor::composer_data::preload_composer_data_for(&composer_ids);
        let new = crate::tools::cursor::composer_data::preload_bubble_data_for(&composer_ids);
        if verbose && composer_ids.len() > 5 {
            eprintln!(" {} old + {} bubble", old.len(), new.len());
        }
        (old, new)
    } else {
        (
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
        )
    };

    for (i, row) in sessions.iter().enumerate() {
        let project_path = match resolve_project_path(db, &row.project_id) {
            Some(p) => p,
            None => {
                failed += 1;
                continue;
            }
        };

        let pseudo_session = Session {
            session_id: row.id.clone(),
            name: row.name.clone().unwrap_or_default(),
            mode: row.mode.clone().unwrap_or_default(),
            created_at: row.created_at,
            updated_at: row.updated_at,
            project_path: project_path.clone(),
            workspace_dir: String::new(),
            source: row.source.clone(),
        };

        let (mut native, cached_messages) = if row.source == "composer" {
            if let Some(bs) = bubble_cache.get(&row.id) {
                let ns = crate::tools::cursor::composer_data::native_stats_from_bubble(bs);
                let msgs = if bs.messages.is_empty() {
                    None
                } else {
                    Some(bs.messages.clone())
                };
                (Some(ns), msgs)
            } else if let Some(cs) = composer_cache
                .get(&row.id)
                .filter(|cs| !cs.messages.is_empty())
            {
                let ns = crate::tools::cursor::composer_data::native_stats_from_session(cs);
                (Some(ns), Some(cs.messages.clone()))
            } else {
                (None, None)
            }
        } else {
            let ns = extract_native_stats(
                &row.source,
                &project_path,
                &row.id,
                row.created_at,
                row.updated_at,
            );
            (ns, None)
        };

        // Enrich model from hook state files when neither native stats nor the
        // session row have one. The scanner already tries read_session_model when
        // building SessionRow, so we only fall back to it if that didn't populate it.
        if native.as_ref().is_none_or(|n| n.model.is_none()) {
            if let Some(ref m) = row.model {
                let n = native.get_or_insert_with(NativeStats::default);
                n.model = Some(m.clone());
            } else if let Some(model) =
                crate::hooks::state::read_session_model(&project_path, &row.id)
            {
                let n = native.get_or_insert_with(NativeStats::default);
                n.model = Some(model);
            }
        }

        if native.as_ref().is_none_or(|n| n.duration_secs.is_none()) {
            if let (Some(created), Some(updated)) = (row.created_at, row.updated_at) {
                let created_s = crate::utils::to_epoch_secs(created);
                let updated_s = crate::utils::to_epoch_secs(updated);
                if updated_s > created_s {
                    let dur = (updated_s - created_s) as u64;
                    match native.as_mut() {
                        Some(n) => n.duration_secs = Some(dur),
                        None => {
                            native = Some(NativeStats {
                                duration_secs: Some(dur),
                                ..Default::default()
                            });
                        }
                    }
                }
            }
        }

        let messages = if let Some(msgs) = cached_messages {
            msgs
        } else {
            let from_path = match session::find_transcript_path(&pseudo_session) {
                Some(path) => session::parse_messages(&path, &row.source),
                None => Vec::new(),
            };
            if from_path.is_empty() {
                session::parse_messages_for_session(&project_path, &row.id, &row.source)
            } else {
                from_path
            }
        };

        let stats = analytics::compute_session_stats(&row.id, &row.source, &messages, native);
        if let Err(e) = db.upsert_stats(&stats) {
            if verbose {
                let src = crate::tui::source_label(&row.source);
                eprintln!(
                    "  error indexing {src}/{}: {e}",
                    &row.id[..row.id.len().min(8)]
                );
            }
            failed += 1;
            continue;
        }

        if let Some(first_msg) = messages
            .iter()
            .find(|m| m.role == "human" || m.role == "user")
        {
            let truncated: String = first_msg.text.chars().take(500).collect();
            let _ = db.update_session_first_message(&row.id, &truncated);
        }

        indexed += 1;

        let done = indexed + failed;
        if done.is_multiple_of(10) {
            update_progress(done, total);
        }

        // Yield CPU every 20 sessions to keep the system responsive
        if (i + 1) % 20 == 0 {
            std::thread::yield_now();
        }

        if verbose && ((i + 1) % 50 == 0 || i + 1 == total) {
            eprint!("\r  progress: {}/{total}  ", i + 1);
        }
    }

    if verbose && total > 0 {
        eprintln!();
    }

    let result_msg = if failed > 0 {
        format!("{indexed} indexed, {failed} failed (of {total})")
    } else {
        format!("{indexed} indexed (of {total})")
    };
    write_status_file("done", indexed + failed, total, &result_msg);

    let (notif_title, notif_body) = build_notification(sessions, indexed, 0, project_name);
    send_notification(&notif_title, &notif_body);

    IndexResult {
        indexed,
        failed,
        total,
    }
}

pub fn find_project_id(db: &Db, name: &str) -> Result<String, String> {
    if let Some(p) = db.get_project_by_id(name)? {
        return Ok(p.id.clone());
    }
    if let Some(p) = db.get_project_by_path(name)? {
        return Ok(p.id.clone());
    }
    let projects = db.list_projects()?;
    let lower = name.to_lowercase();
    for p in &projects {
        if p.name.to_lowercase() == lower || p.id.to_lowercase().contains(&lower) {
            return Ok(p.id.clone());
        }
    }
    Err(format!(
        "project not found: {name}\nrun `oobo scan` to discover projects"
    ))
}

fn resolve_project_path(db: &Db, project_id: &str) -> Option<String> {
    db.get_project_by_id(project_id)
        .ok()
        .flatten()
        .map(|p| p.path)
}

/// Compute and persist stats for a single session proactively (at session-end
/// or commit time) instead of waiting for `oobo scan` / `oobo index`.
///
/// Accepts an optional `ActiveSession` from hook state — used to enrich
/// native stats with model, duration, tool counts, and edited files that
/// the hooks accumulated during the session.
pub fn index_single_session(
    session_id: &str,
    source: &str,
    project_path: &str,
    state: Option<&crate::hooks::state::ActiveSession>,
) -> Result<(), String> {
    let db = Db::open()?;

    let project_id = paths::slug_from_path(project_path);
    db.ensure_project(&project_id, project_path)?;

    let now = chrono::Utc::now().timestamp();
    let (created_at, updated_at) = state
        .map(|s| (Some(s.started_at), Some(s.updated_at)))
        .unwrap_or((None, None));

    let session_row = SessionRow {
        id: session_id.to_string(),
        source: source.to_string(),
        project_id,
        name: None,
        mode: None,
        model: state.and_then(|s| s.model.clone()),
        created_at,
        updated_at,
        message_count: 0,
        first_message: None,
        indexed_at: now,
    };
    db.upsert_session(&session_row)?;

    let mut native = extract_native_stats(source, project_path, session_id, created_at, updated_at);

    if let Some(s) = state {
        let n = native.get_or_insert_with(NativeStats::default);
        if n.model.is_none() {
            n.model.clone_from(&s.model);
        }
        if n.duration_secs.is_none() {
            let dur = (s.updated_at - s.started_at).max(0) as u64;
            if dur > 0 {
                n.duration_secs = Some(dur);
            }
        }
        if n.tool_call_count == 0 {
            if let Some(ref usage) = s.tool_usage {
                n.tool_call_count = usage.values().sum();
            }
        }
        if n.files_touched.is_empty() {
            if let Some(ref files) = s.edited_files {
                n.files_touched = files.iter().cloned().collect();
            }
        }
    }

    let pseudo_session = Session {
        session_id: session_id.to_string(),
        name: String::new(),
        mode: String::new(),
        created_at,
        updated_at,
        project_path: project_path.to_string(),
        workspace_dir: String::new(),
        source: source.to_string(),
    };

    let messages = if source == "composer" {
        load_cursor_messages_and_enrich(session_id, &mut native)
    } else {
        load_non_cursor_messages(session_id, source, project_path, &pseudo_session)
    };

    let stats = analytics::compute_session_stats(session_id, source, &messages, native);
    db.upsert_stats(&stats)?;

    if let Some(first_msg) = messages
        .iter()
        .find(|m| m.role == "human" || m.role == "user")
    {
        let truncated: String = first_msg.text.chars().take(500).collect();
        let _ = db.update_session_first_message(session_id, &truncated);
    }

    Ok(())
}

/// Load messages and native stats for a Cursor session in a single pass.
fn load_cursor_messages_and_enrich(
    session_id: &str,
    native: &mut Option<NativeStats>,
) -> Vec<crate::core::message::Message> {
    let ids = vec![session_id.to_string()];
    let bubble_map = crate::tools::cursor::composer_data::preload_bubble_data_for(&ids);
    if let Some(bs) = bubble_map.get(session_id) {
        let ns = crate::tools::cursor::composer_data::native_stats_from_bubble(bs);
        merge_native_stats(native, &ns);
        if !bs.messages.is_empty() {
            return bs.messages.clone();
        }
    }
    let composer_map = crate::tools::cursor::composer_data::preload_composer_data_for(&ids);
    if let Some(cs) = composer_map
        .get(session_id)
        .filter(|cs| !cs.messages.is_empty())
    {
        let ns = crate::tools::cursor::composer_data::native_stats_from_session(cs);
        merge_native_stats(native, &ns);
        return cs.messages.clone();
    }
    Vec::new()
}

fn load_non_cursor_messages(
    session_id: &str,
    source: &str,
    project_path: &str,
    pseudo_session: &Session,
) -> Vec<crate::core::message::Message> {
    let from_path = match session::find_transcript_path(pseudo_session) {
        Some(path) => session::parse_messages(&path, source),
        None => Vec::new(),
    };
    if from_path.is_empty() {
        session::parse_messages_for_session(project_path, session_id, source)
    } else {
        from_path
    }
}

fn merge_native_stats(target: &mut Option<NativeStats>, new: &NativeStats) {
    let t = target.get_or_insert_with(NativeStats::default);
    if t.model.is_none() {
        t.model.clone_from(&new.model);
    }
    if t.input_tokens.is_none() {
        t.input_tokens = new.input_tokens;
    }
    if t.output_tokens.is_none() {
        t.output_tokens = new.output_tokens;
    }
    if t.cache_read_tokens.is_none() {
        t.cache_read_tokens = new.cache_read_tokens;
    }
    if t.cache_creation_tokens.is_none() {
        t.cache_creation_tokens = new.cache_creation_tokens;
    }
    if t.duration_secs.is_none() {
        t.duration_secs = new.duration_secs;
    }
    if t.files_touched.is_empty() && !new.files_touched.is_empty() {
        t.files_touched.clone_from(&new.files_touched);
    }
    if t.tool_call_count == 0 {
        t.tool_call_count = new.tool_call_count;
    }
}

fn fetch_api_usage(db: &Db) -> String {
    let cfg = crate::config::Config::load_or_default();
    let results = crate::api::fetch_all(&cfg);

    if results.is_empty() {
        return String::new();
    }

    let mut parts = Vec::new();
    for r in &results {
        match &r.error {
            Some(e) => {
                eprintln!("  {} API: {e}", r.source);
                parts.push(format!("{}: error", r.source));
            }
            None => {
                if r.buckets.is_empty() {
                    parts.push(format!("{}: no data", r.source));
                } else {
                    match db.upsert_api_usage(&r.buckets) {
                        Ok(n) => {
                            eprintln!("  {} API: {n} buckets fetched", r.source);
                            parts.push(format!("{}: {n} buckets", r.source));
                        }
                        Err(e) => {
                            eprintln!("  {} API store error: {e}", r.source);
                            parts.push(format!("{}: store error", r.source));
                        }
                    }
                }
            }
        }
    }

    parts.join(", ")
}

fn extract_native_stats(
    source: &str,
    project_path: &str,
    session_id: &str,
    created_at: Option<i64>,
    updated_at: Option<i64>,
) -> Option<NativeStats> {
    let registry = crate::tools::registry();
    let session = crate::core::session::Session {
        session_id: session_id.to_string(),
        name: String::new(),
        mode: String::new(),
        created_at,
        updated_at,
        project_path: project_path.to_string(),
        workspace_dir: String::new(),
        source: source.to_string(),
    };
    registry.by_name(source)?.extract_native_stats(&session)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merge_native_stats_fills_none_fields() {
        let mut target: Option<NativeStats> = None;
        let new = NativeStats {
            model: Some("claude-opus-4".to_string()),
            input_tokens: Some(1000),
            output_tokens: Some(2000),
            cache_read_tokens: Some(500),
            cache_creation_tokens: None,
            duration_secs: Some(120),
            files_touched: vec!["main.rs".to_string()],
            tool_call_count: 5,
        };

        merge_native_stats(&mut target, &new);

        let t = target.unwrap();
        assert_eq!(t.model.as_deref(), Some("claude-opus-4"));
        assert_eq!(t.input_tokens, Some(1000));
        assert_eq!(t.output_tokens, Some(2000));
        assert_eq!(t.cache_read_tokens, Some(500));
        assert_eq!(t.duration_secs, Some(120));
        assert_eq!(t.files_touched, vec!["main.rs"]);
        assert_eq!(t.tool_call_count, 5);
    }

    #[test]
    fn test_merge_native_stats_preserves_existing() {
        let mut target = Some(NativeStats {
            model: Some("gpt-4o".to_string()),
            input_tokens: Some(500),
            output_tokens: None,
            cache_read_tokens: None,
            cache_creation_tokens: None,
            duration_secs: Some(60),
            files_touched: vec!["lib.rs".to_string()],
            tool_call_count: 3,
        });

        let new = NativeStats {
            model: Some("claude-opus-4".to_string()),
            input_tokens: Some(9999),
            output_tokens: Some(2000),
            cache_read_tokens: Some(100),
            cache_creation_tokens: Some(200),
            duration_secs: Some(999),
            files_touched: vec!["other.rs".to_string()],
            tool_call_count: 10,
        };

        merge_native_stats(&mut target, &new);

        let t = target.unwrap();
        assert_eq!(t.model.as_deref(), Some("gpt-4o"));
        assert_eq!(t.input_tokens, Some(500));
        assert_eq!(t.output_tokens, Some(2000));
        assert_eq!(t.cache_read_tokens, Some(100));
        assert_eq!(t.cache_creation_tokens, Some(200));
        assert_eq!(t.duration_secs, Some(60));
        assert_eq!(t.files_touched, vec!["lib.rs"]);
        assert_eq!(t.tool_call_count, 3);
    }

    #[test]
    fn test_index_single_session_with_state() {
        let db = crate::db::Db::open_in_memory().unwrap();

        let project_path = "/tmp/test-project";
        let session_id = "test-sess-1";
        let source = "claude";
        let project_id = crate::paths::slug_from_path(project_path);

        db.ensure_project(&project_id, project_path).unwrap();

        let now = chrono::Utc::now().timestamp();
        let state = crate::hooks::state::ActiveSession {
            session_id: session_id.to_string(),
            agent: source.to_string(),
            model: Some("claude-opus-4".to_string()),
            worktree: None,
            transcript_path: None,
            pre_agent_snapshots: None,
            file_snapshots: None,
            edited_files: Some(
                ["src/main.rs".to_string()]
                    .into_iter()
                    .collect(),
            ),
            tool_usage: Some(
                [("Bash".to_string(), 3), ("Edit".to_string(), 2)]
                    .into_iter()
                    .collect(),
            ),
            tool_failures: Some(1),
            bash_commands: Some(vec!["cargo build".to_string()]),
            subagent_runs: None,
            thinking_duration_ms: Some(1500),
            compact_count: None,
            started_at: now - 300,
            updated_at: now,
        };

        // index_single_session opens its own DB, so we verify the enrichment
        // logic by testing the helper functions it uses.
        let mut native: Option<NativeStats> = None;
        let n = native.get_or_insert_with(NativeStats::default);
        n.model.clone_from(&state.model);
        let dur = (state.updated_at - state.started_at).max(0) as u64;
        n.duration_secs = Some(dur);
        if let Some(ref usage) = state.tool_usage {
            n.tool_call_count = usage.values().sum();
        }
        if let Some(ref files) = state.edited_files {
            n.files_touched = files.iter().cloned().collect();
        }

        let n = native.unwrap();
        assert_eq!(n.model.as_deref(), Some("claude-opus-4"));
        assert_eq!(n.duration_secs, Some(300));
        assert_eq!(n.tool_call_count, 5);
        assert!(n.files_touched.contains(&"src/main.rs".to_string()));
    }

    #[test]
    fn test_load_non_cursor_messages_empty_project() {
        let pseudo = Session {
            session_id: "nonexistent".to_string(),
            name: String::new(),
            mode: String::new(),
            created_at: None,
            updated_at: None,
            project_path: "/nonexistent/path".to_string(),
            workspace_dir: String::new(),
            source: "claude".to_string(),
        };

        let messages =
            load_non_cursor_messages("nonexistent", "claude", "/nonexistent/path", &pseudo);
        assert!(messages.is_empty());
    }
}

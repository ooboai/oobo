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

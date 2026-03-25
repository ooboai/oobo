use crate::cli::{OutputMode, SessionAction};
use crate::config::Config;
use crate::db::Db;
use crate::session;

pub fn run(cfg: &Config, action: SessionAction, mode: OutputMode) -> Result<(), String> {
    match action {
        SessionAction::List { all, tool, limit } => match mode {
            OutputMode::Agent => list_agent(cfg, all, tool.as_deref(), limit),
            OutputMode::Json => list_json(cfg, all, tool.as_deref(), limit),
            OutputMode::Tui => list_tui(cfg, all),
        },
        SessionAction::Show { id } => match mode {
            OutputMode::Agent => show_agent(&id),
            OutputMode::Json => show_json(&id),
            OutputMode::Tui => {
                let s = session::find_session_any(&id)?;
                crate::tui::sessions::run_show(s)
            }
        },
        SessionAction::Search { query, all, limit } => search(&query, cfg, all, mode, limit),
        SessionAction::Export { id, format, out } => export(&id, &format, out.as_deref()),
    }
}

fn list_tui(cfg: &Config, all: bool) -> Result<(), String> {
    let (sessions, show_all) = if all {
        (session::all_sessions(cfg), true)
    } else {
        let root = crate::tools::cursor::get_project_root();
        let s = session::all_for_project(&root, cfg);
        if s.is_empty() {
            let proj = std::path::Path::new(&root)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&root);
            eprintln!("No sessions for \"{proj}\" — showing all sessions\n");
            (session::all_sessions(cfg), true)
        } else {
            (s, false)
        }
    };

    crate::tui::sessions::run_list(sessions, show_all)
}

fn list_agent(
    cfg: &Config,
    all: bool,
    tool_filter: Option<&str>,
    limit: Option<usize>,
) -> Result<(), String> {
    let (mut sessions, scope_all) = if all {
        (session::all_sessions(cfg), true)
    } else {
        let root = crate::tools::cursor::get_project_root();
        let s = session::all_for_project(&root, cfg);
        if s.is_empty() {
            (session::all_sessions(cfg), true)
        } else {
            (s, false)
        }
    };

    if let Some(tool) = tool_filter {
        let lower = tool.to_lowercase();
        sessions.retain(|s| {
            s.source.to_lowercase() == lower
                || crate::tui::source_label(&s.source).to_lowercase() == lower
        });
    }

    if let Some(n) = limit {
        sessions.truncate(n);
    }

    let db = Db::open().ok();
    let mut stats_map = db
        .as_ref()
        .and_then(|d| d.get_stats_bulk(&[]).ok())
        .unwrap_or_default();

    index_missing_sessions(&sessions, &mut stats_map);

    if scope_all {
        println!("# scope: all");
    } else {
        println!("# scope: project");
    }
    println!("# session_id | name | source | model | in_tokens | out_tokens | updated");
    for s in &sessions {
        let st = stats_map.get(&(s.session_id.clone(), s.source.clone()));
        let model = st.and_then(|st| st.model.as_deref()).unwrap_or("");
        let in_tok = st
            .and_then(|st| st.input_tokens)
            .map(crate::tui::format_tokens)
            .unwrap_or_default();
        let out_tok = st
            .and_then(|st| st.output_tokens)
            .map(crate::tui::format_tokens)
            .unwrap_or_default();
        let name = crate::utils::sanitize_pipe(&crate::utils::truncate_name(&s.name, 60));
        println!(
            "{} | {} | {} | {} | {} | {} | {}",
            s.session_id,
            name,
            crate::tui::source_label(&s.source),
            model,
            in_tok,
            out_tok,
            s.updated_at_iso(),
        );
    }
    Ok(())
}

fn list_json(
    cfg: &Config,
    all: bool,
    tool_filter: Option<&str>,
    limit: Option<usize>,
) -> Result<(), String> {
    let mut sessions = if all {
        session::all_sessions(cfg)
    } else {
        let root = crate::tools::cursor::get_project_root();
        let s = session::all_for_project(&root, cfg);
        if s.is_empty() {
            session::all_sessions(cfg)
        } else {
            s
        }
    };

    if let Some(tool) = tool_filter {
        let lower = tool.to_lowercase();
        sessions.retain(|s| {
            s.source.to_lowercase() == lower
                || crate::tui::source_label(&s.source).to_lowercase() == lower
        });
    }

    if let Some(n) = limit {
        sessions.truncate(n);
    }

    let db = Db::open().ok();
    let mut stats_map = db
        .as_ref()
        .and_then(|d| d.get_stats_bulk(&[]).ok())
        .unwrap_or_default();

    // Index sessions without stats inline so the output is complete.
    index_missing_sessions(&sessions, &mut stats_map);

    let peer_map = compute_peer_map(&sessions, &stats_map);

    let items: Vec<serde_json::Value> = sessions
        .iter()
        .map(|s| {
            let st = stats_map.get(&(s.session_id.clone(), s.source.clone()));

            let mut obj = serde_json::json!({
                "session_id": s.session_id,
                "name": s.name,
                "source": s.source,
                "mode": s.mode,
                "project_path": s.project_path,
                "created_at": s.created_at_iso(),
                "updated_at": s.updated_at_iso(),
            });

            if let Some(ref pid) = s.parent_session_id {
                obj["parent_session_id"] = serde_json::json!(pid);
            }
            if let Some(ref stype) = s.subagent_type {
                obj["subagent_type"] = serde_json::json!(stype);
            }

            if let Some(st) = st {
                obj["model"] = serde_json::json!(st.model);
                obj["input_tokens"] = serde_json::json!(st.input_tokens);
                obj["output_tokens"] = serde_json::json!(st.output_tokens);
                obj["duration_secs"] = serde_json::json!(st.duration_secs);
                obj["is_estimated"] = serde_json::json!(st.is_estimated);
                obj["files_touched"] = serde_json::json!(st.files_touched);
                obj["tool_calls"] = serde_json::json!(st.tool_call_count);
            }

            if let Some(peers) = peer_map.get(&s.session_id) {
                if !peers.is_empty() {
                    obj["peer_session_ids"] = serde_json::json!(peers);
                }
            }

            obj
        })
        .collect();

    crate::utils::print_json(&items);
    Ok(())
}

fn search(
    query: &str,
    cfg: &Config,
    all: bool,
    mode: OutputMode,
    limit: usize,
) -> Result<(), String> {
    let (sessions, scope_all) = if all {
        (session::all_sessions(cfg), true)
    } else {
        let root = crate::tools::cursor::get_project_root();
        let s = session::all_for_project(&root, cfg);
        if s.is_empty() {
            (session::all_sessions(cfg), true)
        } else {
            (s, false)
        }
    };

    let lower_query = query.to_lowercase();
    let db = Db::open().ok();
    let mut stats_map = db
        .as_ref()
        .and_then(|d| d.get_stats_bulk(&[]).ok())
        .unwrap_or_default();

    index_missing_sessions(&sessions, &mut stats_map);

    let mut matches: Vec<(
        &crate::tools::cursor::Session,
        Option<&crate::db::stats::StatsRow>,
        &str,
    )> = Vec::new();

    for s in &sessions {
        let st = stats_map.get(&(s.session_id.clone(), s.source.clone()));

        if s.name.to_lowercase().contains(&lower_query)
            || s.session_id.to_lowercase().contains(&lower_query)
            || s.source.to_lowercase().contains(&lower_query)
            || s.mode.to_lowercase().contains(&lower_query)
        {
            matches.push((s, st, "name"));
            continue;
        }

        let db_row = db
            .as_ref()
            .and_then(|d| d.get_session(&s.session_id, &s.source).ok())
            .flatten();
        if let Some(ref row) = db_row {
            if let Some(ref first_msg) = row.first_message {
                if first_msg.to_lowercase().contains(&lower_query) {
                    matches.push((s, st, "first_message"));
                    continue;
                }
            }
        }
    }

    matches.truncate(limit);

    match mode {
        OutputMode::Agent => {
            if scope_all {
                println!("# scope: all");
            } else {
                println!("# scope: project");
            }
            println!("# session_id | name | source | model | matched_on | updated");
            for (s, st, matched_on) in &matches {
                let model = st.and_then(|st| st.model.as_deref()).unwrap_or("");
                let name = crate::utils::sanitize_pipe(&crate::utils::truncate_name(&s.name, 60));
                println!(
                    "{} | {} | {} | {} | {} | {}",
                    s.session_id,
                    name,
                    crate::tui::source_label(&s.source),
                    model,
                    matched_on,
                    s.updated_at_iso(),
                );
            }
        }
        OutputMode::Json => {
            let peer_map = compute_peer_map(&sessions, &stats_map);
            let items: Vec<serde_json::Value> = matches
                .iter()
                .map(|(s, st, matched_on)| {
                    let mut obj = serde_json::json!({
                        "session_id": s.session_id,
                        "name": s.name,
                        "source": s.source,
                        "mode": s.mode,
                        "project_path": s.project_path,
                        "created_at": s.created_at_iso(),
                        "updated_at": s.updated_at_iso(),
                        "matched_on": matched_on,
                    });
                    if let Some(st) = st {
                        obj["model"] = serde_json::json!(st.model);
                        obj["input_tokens"] = serde_json::json!(st.input_tokens);
                        obj["output_tokens"] = serde_json::json!(st.output_tokens);
                    }
                    if let Some(ref pid) = s.parent_session_id {
                        obj["parent_session_id"] = serde_json::json!(pid);
                    }
                    if let Some(ref stype) = s.subagent_type {
                        obj["subagent_type"] = serde_json::json!(stype);
                    }
                    if let Some(peers) = peer_map.get(&s.session_id) {
                        if !peers.is_empty() {
                            obj["peer_session_ids"] = serde_json::json!(peers);
                        }
                    }
                    obj
                })
                .collect();
            crate::utils::print_json(&items);
        }
        OutputMode::Tui => {
            if matches.is_empty() {
                eprintln!("no sessions matching \"{query}\"");
                return Ok(());
            }
            println!("{:<10} {:<10} {:<12} NAME", "ID", "SOURCE", "UPDATED");
            for (s, _, _) in &matches {
                println!(
                    "{:<10} {:<10} {:<12} {}",
                    &s.session_id[..s.session_id.len().min(8)],
                    crate::tui::source_label(&s.source),
                    s.updated_at_iso(),
                    crate::utils::truncate_name(&s.name, 60),
                );
            }
            eprintln!("\n{} result(s)", matches.len());
        }
    }

    Ok(())
}

fn show_agent(id: &str) -> Result<(), String> {
    let s = session::find_session_any(id)?;

    let db = Db::open().ok();
    let stats = db
        .as_ref()
        .and_then(|d| d.get_stats(&s.session_id, &s.source).ok())
        .flatten();

    let msg_count = session::count_messages(&s);

    println!("session_id: {}", s.session_id);
    println!("name: {}", crate::utils::sanitize_pipe(&s.name));
    println!("source: {}", crate::tui::source_label(&s.source));
    println!("mode: {}", s.mode);
    println!("project_path: {}", s.project_path);
    if let Some(ref pid) = s.parent_session_id {
        println!("parent_session_id: {pid}");
    }
    if let Some(ref stype) = s.subagent_type {
        println!("subagent_type: {stype}");
    }
    if let Some(ref st) = stats {
        let model = st.model.as_deref().unwrap_or("unknown");
        println!("model: {model}");
        let in_t = st.input_tokens.unwrap_or(0);
        let out_t = st.output_tokens.unwrap_or(0);
        println!(
            "tokens: {}/{}",
            crate::tui::format_tokens(in_t),
            crate::tui::format_tokens(out_t)
        );
        if let Some(dur) = st.duration_secs {
            println!("duration: {}", crate::tui::format_duration(dur));
        }
        if !st.files_touched.is_empty() {
            println!("files: {}", st.files_touched.len());
        }
        if st.tool_call_count > 0 {
            println!("tool_calls: {}", st.tool_call_count);
        }
    }
    println!("messages: {msg_count}");
    println!("created: {}", s.created_at_iso());
    println!("updated: {}", s.updated_at_iso());

    Ok(())
}

fn show_json(id: &str) -> Result<(), String> {
    let s = session::find_session_any(id)?;
    let transcript_path = session::find_transcript_path(&s);
    let mut messages = transcript_path
        .as_ref()
        .map(|p| session::parse_messages(p, &s.source))
        .unwrap_or_default();

    if messages.is_empty() && s.source == "composer" {
        let ids = vec![s.session_id.clone()];
        let bubble_map = crate::tools::cursor::composer_data::preload_bubble_data_for(&ids);
        if let Some(bs) = bubble_map.get(&s.session_id) {
            messages = bs.messages.clone();
        }
        if messages.is_empty() {
            let composer_map = crate::tools::cursor::composer_data::preload_composer_data_for(&ids);
            if let Some(cs) = composer_map.get(&s.session_id) {
                messages = cs.messages.clone();
            }
        }
    }

    let db = Db::open().ok();
    let stats = db
        .as_ref()
        .and_then(|d| d.get_stats(&s.session_id, &s.source).ok())
        .flatten();

    let mut obj = serde_json::json!({
        "session_id": s.session_id,
        "name": s.name,
        "mode": s.mode,
        "created_at": s.created_at_iso(),
        "updated_at": s.updated_at_iso(),
        "project_path": s.project_path,
        "source": s.source,
        "message_count": messages.len(),
        "messages": messages,
    });

    if let Some(ref pid) = s.parent_session_id {
        obj["parent_session_id"] = serde_json::json!(pid);
    }
    if let Some(ref stype) = s.subagent_type {
        obj["subagent_type"] = serde_json::json!(stype);
    }

    if let Some(ref st) = stats {
        obj["model"] = serde_json::json!(st.model);
        obj["input_tokens"] = serde_json::json!(st.input_tokens);
        obj["output_tokens"] = serde_json::json!(st.output_tokens);
        obj["duration_secs"] = serde_json::json!(st.duration_secs);
        obj["is_estimated"] = serde_json::json!(st.is_estimated);
        obj["files_touched"] = serde_json::json!(st.files_touched);
        obj["tool_calls"] = serde_json::json!(st.tool_call_count);
    }

    if !s.project_path.is_empty() && !s.is_subagent() {
        let cfg = crate::config::Config::load_or_default();
        let project_sessions = session::all_for_project(&s.project_path, &cfg);
        let keys: Vec<(String, String)> = project_sessions
            .iter()
            .map(|ps| (ps.session_id.clone(), ps.source.clone()))
            .collect();
        let mut stats_map = db
            .as_ref()
            .and_then(|d| d.get_stats_bulk(&keys).ok())
            .unwrap_or_default();
        index_missing_sessions(&project_sessions, &mut stats_map);
        let peer_map = compute_peer_map(&project_sessions, &stats_map);
        if let Some(peers) = peer_map.get(&s.session_id) {
            if !peers.is_empty() {
                obj["peer_session_ids"] = serde_json::json!(peers);
            }
        }
    }

    crate::utils::print_json(&obj);
    Ok(())
}

fn export(id: &str, format: &str, out: Option<&str>) -> Result<(), String> {
    let s = session::find_session_any(id)?;
    let transcript_path = session::find_transcript_path(&s);

    let content = match format {
        "json" => {
            let messages = transcript_path
                .as_ref()
                .map(|p| session::parse_messages(p, &s.source))
                .unwrap_or_default();

            let obj = serde_json::json!({
                "session_id": s.session_id,
                "name": s.name,
                "mode": s.mode,
                "created_at": s.created_at_iso(),
                "updated_at": s.updated_at_iso(),
                "project_path": s.project_path,
                "source": s.source,
                "messages": messages,
            });

            serde_json::to_string_pretty(&obj).unwrap_or_default()
        }
        _ => {
            let transcript_text = match transcript_path.as_ref() {
                Some(p) => {
                    let text = session::read_transcript(p, 500, &s.source);
                    if text.is_empty() {
                        session::read_transcript_for_session(
                            &s.project_path,
                            &s.session_id,
                            500,
                            &s.source,
                        )
                    } else {
                        text
                    }
                }
                None => session::read_transcript_for_session(
                    &s.project_path,
                    &s.session_id,
                    500,
                    &s.source,
                ),
            };

            let title = if s.name.is_empty() {
                "Untitled Session"
            } else {
                &s.name
            };

            format!(
                "# {title}\n\n\
                 - **Session ID:** {}\n\
                 - **Source:** {}\n\
                 - **Mode:** {}\n\
                 - **Created:** {}\n\
                 - **Updated:** {}\n\
                 - **Project:** {}\n\n\
                 ---\n\n\
                 {transcript_text}",
                s.session_id,
                s.source,
                s.mode,
                s.created_at_iso(),
                s.updated_at_iso(),
                s.project_path,
            )
        }
    };

    match out {
        Some(path) => {
            std::fs::write(path, &content).map_err(|e| format!("cannot write {path}: {e}"))?;
            println!("Exported to {path}");
        }
        None => {
            println!("{content}");
        }
    }

    Ok(())
}

const INLINE_INDEX_CAP: usize = 20;

/// Compute peer_session_ids for each session by comparing file interactions.
/// Tries ephemeral session state files first (active sessions), then falls
/// back to `files_touched` from the stats DB (historical sessions).
fn compute_peer_map(
    sessions: &[crate::tools::cursor::Session],
    stats_map: &std::collections::HashMap<(String, String), crate::db::stats::StatsRow>,
) -> std::collections::HashMap<String, Vec<String>> {
    use std::collections::HashMap;

    if sessions.len() < 2 {
        return HashMap::new();
    }

    let mut by_project: HashMap<&str, Vec<&crate::tools::cursor::Session>> = HashMap::new();
    for s in sessions {
        if !s.project_path.is_empty() && !s.is_subagent() {
            by_project.entry(&s.project_path).or_default().push(s);
        }
    }

    let mut all_peers: HashMap<String, Vec<String>> = HashMap::new();

    for group in by_project.values() {
        if group.len() < 2 {
            continue;
        }

        let inputs: Vec<crate::core::anchor::SessionFiles> = group
            .iter()
            .map(|s| {
                let (edited, read) =
                    crate::hooks::state::get_file_sets(&s.project_path, &s.session_id);

                if edited.is_empty() && read.is_empty() {
                    if let Some(st) = stats_map.get(&(s.session_id.clone(), s.source.clone())) {
                        return crate::core::anchor::SessionFiles {
                            session_id: s.session_id.clone(),
                            edited: st.files_touched.clone(),
                            read: Vec::new(),
                        };
                    }
                }

                crate::core::anchor::SessionFiles {
                    session_id: s.session_id.clone(),
                    edited,
                    read,
                }
            })
            .collect();

        let (_, peer_map) = crate::core::anchor::detect_interactions(&inputs);
        all_peers.extend(peer_map);
    }

    all_peers
}

/// Index sessions that have no stats or stale stats (blocking).
/// Capped at [`INLINE_INDEX_CAP`] to avoid blocking JSON/agent output.
fn index_missing_sessions(
    sessions: &[crate::tools::cursor::Session],
    stats_map: &mut std::collections::HashMap<(String, String), crate::db::stats::StatsRow>,
) {
    let needs_index: Vec<_> = sessions
        .iter()
        .filter(|s| {
            if s.project_path.is_empty() {
                return false;
            }
            let key = (s.session_id.clone(), s.source.clone());
            match stats_map.get(&key) {
                None => true,
                Some(st) => st.is_stale(s.updated_at),
            }
        })
        .take(INLINE_INDEX_CAP)
        .collect();

    if needs_index.is_empty() {
        return;
    }

    let indexed_keys: Vec<(String, String)> = needs_index
        .iter()
        .map(|s| (s.session_id.clone(), s.source.clone()))
        .collect();

    for s in &needs_index {
        if let Err(e) = crate::commands::index::index_single_session(
            &s.session_id,
            &s.source,
            &s.project_path,
            None,
        ) {
            eprintln!(
                "oobo: warning: could not index session {}: {e}",
                &s.session_id[..s.session_id.len().min(8)]
            );
        }
    }

    // Reload stats only for the sessions we just re-indexed.
    if let Ok(db) = Db::open() {
        for (sid, source) in &indexed_keys {
            if let Ok(Some(fresh)) = db.get_stats(sid, source) {
                stats_map.insert((sid.clone(), source.clone()), fresh);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_index_missing_sessions_skips_when_all_present() {
        let sessions = vec![crate::tools::cursor::Session {
            session_id: "s1".to_string(),
            name: "test".to_string(),
            mode: "agent".to_string(),
            created_at: Some(1000),
            updated_at: Some(2000),
            project_path: "/test".to_string(),
            workspace_dir: String::new(),
            source: "composer".to_string(),
            parent_session_id: None,
            subagent_type: None,
        }];

        let mut stats_map = std::collections::HashMap::new();
        stats_map.insert(
            ("s1".to_string(), "composer".to_string()),
            crate::db::stats::StatsRow {
                session_id: "s1".to_string(),
                source: "composer".to_string(),
                model: Some("claude-opus-4".to_string()),
                input_tokens: Some(1000),
                output_tokens: Some(2000),
                computed_at: 3000,
                ..Default::default()
            },
        );

        let initial_len = stats_map.len();
        index_missing_sessions(&sessions, &mut stats_map);
        assert_eq!(stats_map.len(), initial_len);
    }

    #[test]
    fn test_index_missing_sessions_skips_empty_project_path() {
        let sessions = vec![crate::tools::cursor::Session {
            session_id: "s2".to_string(),
            name: "orphan".to_string(),
            mode: "agent".to_string(),
            created_at: Some(1000),
            updated_at: Some(2000),
            project_path: String::new(),
            workspace_dir: String::new(),
            source: "composer".to_string(),
            parent_session_id: None,
            subagent_type: None,
        }];

        let mut stats_map = std::collections::HashMap::new();
        index_missing_sessions(&sessions, &mut stats_map);
        // Should not crash or add anything for empty project_path sessions.
    }
}

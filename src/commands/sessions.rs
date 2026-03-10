use crate::cli::SessionAction;
use crate::config::Config;
use crate::db::Db;
use crate::session;

pub fn run(cfg: &Config, action: SessionAction, agent: bool) -> Result<(), String> {
    match action {
        SessionAction::List { all, tool, limit } => {
            if agent {
                list_json(cfg, all, tool.as_deref(), limit)
            } else {
                list_tui(cfg, all)
            }
        }
        SessionAction::Show { id } => {
            if agent {
                show_json(&id)
            } else {
                let s = session::find_session_any(&id)?;
                crate::tui::sessions::run_show(s)
            }
        }
        SessionAction::Search { query, all, limit } => search(&query, cfg, all, agent, limit),
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
    let stats_map = db.as_ref().and_then(|d| d.get_stats_bulk(&[]).ok());

    let items: Vec<serde_json::Value> = sessions
        .iter()
        .map(|s| {
            let st = stats_map
                .as_ref()
                .and_then(|m| m.get(&(s.session_id.clone(), s.source.clone())));

            let mut obj = serde_json::json!({
                "session_id": s.session_id,
                "name": s.name,
                "source": s.source,
                "mode": s.mode,
                "project_path": s.project_path,
                "created_at": s.created_at_iso(),
                "updated_at": s.updated_at_iso(),
            });

            if let Some(st) = st {
                obj["model"] = serde_json::json!(st.model);
                obj["input_tokens"] = serde_json::json!(st.input_tokens);
                obj["output_tokens"] = serde_json::json!(st.output_tokens);
                obj["duration_secs"] = serde_json::json!(st.duration_secs);
                obj["is_estimated"] = serde_json::json!(st.is_estimated);
                obj["files_touched"] = serde_json::json!(st.files_touched);
                obj["tool_calls"] = serde_json::json!(st.tool_call_count);
            }

            obj
        })
        .collect();

    crate::utils::print_json(&items);
    Ok(())
}

fn search(query: &str, cfg: &Config, all: bool, agent: bool, limit: usize) -> Result<(), String> {
    let sessions = if all {
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

    let lower_query = query.to_lowercase();
    let db = Db::open().ok();
    let stats_map = db.as_ref().and_then(|d| d.get_stats_bulk(&[]).ok());

    let mut matches: Vec<(
        &crate::tools::cursor::Session,
        Option<&crate::db::stats::StatsRow>,
        &str,
    )> = Vec::new();

    for s in &sessions {
        let st = stats_map
            .as_ref()
            .and_then(|m| m.get(&(s.session_id.clone(), s.source.clone())));

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

    if agent {
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
                obj
            })
            .collect();
        crate::utils::print_json(&items);
    } else {
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

    if let Some(st) = stats {
        obj["model"] = serde_json::json!(st.model);
        obj["input_tokens"] = serde_json::json!(st.input_tokens);
        obj["output_tokens"] = serde_json::json!(st.output_tokens);
        obj["duration_secs"] = serde_json::json!(st.duration_secs);
        obj["is_estimated"] = serde_json::json!(st.is_estimated);
        obj["files_touched"] = serde_json::json!(st.files_touched);
        obj["tool_calls"] = serde_json::json!(st.tool_call_count);
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

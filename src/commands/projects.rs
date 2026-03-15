use std::io::IsTerminal;

use crate::cli::ProjectAction;
use crate::db::Db;

pub fn run(action: ProjectAction, agent: bool) -> Result<(), String> {
    let db = Db::open()?;

    match action {
        ProjectAction::List => {
            if agent {
                list_projects_json(&db)
            } else {
                list_projects(&db)
            }
        }
        ProjectAction::Show { name } => {
            if agent {
                show_project_json(&db, &name)
            } else {
                show_project(&db, &name)
            }
        }
        ProjectAction::Forget { name } => forget_project(&db, &name),
    }
}

fn list_projects(db: &Db) -> Result<(), String> {
    let projects = db.list_projects()?;

    if projects.is_empty() {
        eprintln!("no projects found — run `oobo scan` first");
        return Ok(());
    }

    let mut displays: Vec<crate::tui::projects::ProjectDisplay> = Vec::new();
    for p in &projects {
        let session_count = db.count_sessions_by_project(&p.id).unwrap_or(0);
        let stats = db.aggregate_stats_by_project(&p.id).unwrap_or_default();
        let tools: Vec<String> = p
            .tools
            .iter()
            .map(|t| crate::tui::source_label(t).to_string())
            .collect();

        let sessions_rows = db.list_sessions_by_project(&p.id).unwrap_or_default();
        let session_displays: Vec<crate::tui::projects::SessionDisplay> = sessions_rows
            .iter()
            .map(|s| {
                let s_stats = db.get_stats(&s.id, &s.source).ok().flatten();
                let tokens = s_stats
                    .as_ref()
                    .map(|st| st.input_tokens.unwrap_or(0) + st.output_tokens.unwrap_or(0))
                    .unwrap_or(0);
                let updated = s
                    .updated_at
                    .map(|ts| {
                        let secs = crate::utils::to_epoch_secs(ts);
                        chrono::DateTime::from_timestamp(secs, 0)
                            .map(|dt| dt.format("%Y-%m-%d").to_string())
                            .unwrap_or_else(|| "—".to_string())
                    })
                    .unwrap_or_else(|| "—".to_string());

                crate::tui::projects::SessionDisplay {
                    id: s.id.clone(),
                    source: s.source.clone(),
                    name: s.name.clone().unwrap_or_else(|| "(untitled)".to_string()),
                    tokens,
                    updated,
                }
            })
            .collect();

        displays.push(crate::tui::projects::ProjectDisplay {
            name: p.name.clone(),
            path: p.path.clone(),
            tools: tools.join(", "),
            session_count,
            tokens: stats.total_input_tokens + stats.total_output_tokens,
            input_tokens: stats.total_input_tokens,
            output_tokens: stats.total_output_tokens,
            sessions: session_displays,
        });
    }

    displays.sort_by(|a, b| b.tokens.cmp(&a.tokens));

    if std::io::stdin().is_terminal() {
        crate::tui::projects::run(displays)
    } else {
        print_plain(&displays)
    }
}

fn print_plain(projects: &[crate::tui::projects::ProjectDisplay]) -> Result<(), String> {
    println!(
        "{:<28} {:>8} {:<16} {:>10} {:>8}  PATH",
        "PROJECT", "SESSIONS", "TOOLS", "TOKENS", "COST"
    );
    for p in projects {
        let name = if p.name.chars().count() > 26 {
            let truncated: String = p.name.chars().take(25).collect();
            format!("{truncated}…")
        } else {
            p.name.clone()
        };
        println!(
            "{:<28} {:>8} {:<16} {:>10}  {}",
            name,
            p.session_count,
            p.tools,
            crate::tui::format_tokens(p.tokens),
            p.path
        );
    }
    println!("\n{} project(s)", projects.len());
    Ok(())
}

fn list_projects_json(db: &Db) -> Result<(), String> {
    let projects = db.list_projects()?;

    let items: Vec<serde_json::Value> = projects
        .iter()
        .map(|p| {
            let session_count = db.count_sessions_by_project(&p.id).unwrap_or(0);
            let stats = db.aggregate_stats_by_project(&p.id).unwrap_or_default();

            serde_json::json!({
                "id": p.id,
                "name": p.name,
                "path": p.path,
                "git_remote": p.git_remote,
                "tools": p.tools,
                "sessions": session_count,
                "input_tokens": stats.total_input_tokens,
                "output_tokens": stats.total_output_tokens,
                "total_tokens": stats.total_input_tokens + stats.total_output_tokens,
                "duration_secs": stats.total_duration_secs,
            })
        })
        .collect();

    crate::utils::print_json(&items);
    Ok(())
}

fn show_project_json(db: &Db, name: &str) -> Result<(), String> {
    let project = find_project(db, name)?;
    let stats = db.aggregate_stats_by_project(&project.id)?;
    let sessions = db.list_sessions_by_project(&project.id)?;

    let session_items: Vec<serde_json::Value> = sessions
        .iter()
        .map(|s| {
            let s_stats = db.get_stats(&s.id, &s.source).ok().flatten();
            let mut obj = serde_json::json!({
                "session_id": s.id,
                "source": s.source,
                "name": s.name,
                "mode": s.mode,
                "model": s.model,
                "created_at": s.created_at,
                "updated_at": s.updated_at,
                "message_count": s.message_count,
            });
            if let Some(st) = s_stats {
                obj["input_tokens"] = serde_json::json!(st.input_tokens);
                obj["output_tokens"] = serde_json::json!(st.output_tokens);
                obj["duration_secs"] = serde_json::json!(st.duration_secs);
                obj["files_touched"] = serde_json::json!(st.files_touched);
                obj["tool_calls"] = serde_json::json!(st.tool_call_count);
            }
            obj
        })
        .collect();

    let json = serde_json::json!({
        "id": project.id,
        "name": project.name,
        "path": project.path,
        "git_remote": project.git_remote,
        "tools": project.tools,
        "sessions": session_items,
        "stats": {
            "session_count": stats.session_count,
            "input_tokens": stats.total_input_tokens,
            "output_tokens": stats.total_output_tokens,
            "total_tokens": stats.total_input_tokens + stats.total_output_tokens,
            "duration_secs": stats.total_duration_secs,
        },
    });

    crate::utils::print_json(&json);
    Ok(())
}

fn show_project(db: &Db, name: &str) -> Result<(), String> {
    let project = find_project(db, name)?;

    let session_count = db.count_sessions_by_project(&project.id).unwrap_or(0);
    let stats = db.aggregate_stats_by_project(&project.id)?;
    let tools: Vec<String> = project
        .tools
        .iter()
        .map(|t| crate::tui::source_label(t).to_string())
        .collect();

    println!();
    println!("  \x1b[1;36mProject\x1b[0m  {}", project.name);
    println!("  \x1b[90mPath\x1b[0m     {}", project.path);
    if let Some(ref remote) = project.git_remote {
        println!("  \x1b[90mRemote\x1b[0m   {remote}");
    }
    println!("  \x1b[90mTools\x1b[0m    {}", tools.join(", "));
    println!("  \x1b[90mSessions\x1b[0m {session_count}");

    if stats.session_count > 0 {
        let total = stats.total_input_tokens + stats.total_output_tokens;
        println!();
        println!("  \x1b[1;36mAnalytics\x1b[0m");
        println!(
            "  \x1b[90mTokens\x1b[0m   {} in / {} out ({} total)",
            crate::tui::format_tokens(stats.total_input_tokens),
            crate::tui::format_tokens(stats.total_output_tokens),
            crate::tui::format_tokens(total),
        );
        if stats.total_duration_secs > 0 {
            println!(
                "  \x1b[90mTime\x1b[0m     {}",
                crate::tui::format_duration(stats.total_duration_secs)
            );
        }
    }

    let sessions = db.list_sessions_by_project(&project.id)?;
    if !sessions.is_empty() {
        println!();
        println!("  \x1b[1;36mRecent Sessions\x1b[0m");
        for s in sessions.iter().take(10) {
            let name_display = s.name.as_deref().unwrap_or("(untitled)");
            let display = if name_display.chars().count() > 50 {
                let truncated: String = name_display.chars().take(49).collect();
                format!("{truncated}…")
            } else {
                name_display.to_string()
            };
            let src = crate::tui::source_label(&s.source);
            println!(
                "  \x1b[90m{}\x1b[0m  \x1b[33m{:<8}\x1b[0m {}",
                &s.id[..s.id.len().min(8)],
                src,
                display
            );
        }
    }

    println!();
    Ok(())
}

fn forget_project(db: &Db, name: &str) -> Result<(), String> {
    let project = find_project(db, name)?;
    db.delete_project(&project.id)?;

    let project_dir = crate::paths::oobo_project_dir(&project.path);
    if project_dir.exists() {
        let _ = std::fs::remove_dir_all(&project_dir);
    }

    eprintln!("removed project: {} ({})", project.name, project.path);
    Ok(())
}

fn find_project(db: &Db, name: &str) -> Result<crate::db::projects::ProjectRow, String> {
    if let Some(p) = db.get_project_by_id(name)? {
        return Ok(p);
    }
    if let Some(p) = db.get_project_by_path(name)? {
        return Ok(p);
    }
    let projects = db.list_projects()?;
    let lower = name.to_lowercase();
    for p in &projects {
        if p.name.to_lowercase() == lower || p.id.to_lowercase().contains(&lower) {
            return Ok(p.clone());
        }
    }
    Err(format!(
        "project not found: {name}\nrun `oobo scan` to discover projects"
    ))
}

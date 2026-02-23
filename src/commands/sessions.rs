use crate::cli::SessionAction;
use crate::config::Config;
use crate::session;

pub fn run(cfg: &Config, action: SessionAction) -> Result<(), String> {
    match action {
        SessionAction::List { all } => list(cfg, all),
        SessionAction::Show { id, json } => {
            if json {
                show_json(&id)
            } else {
                let s = session::find_session_any(&id)?;
                crate::tui::sessions::run_show(s)
            }
        }
        SessionAction::Export { id, format, out } => export(&id, &format, out.as_deref()),
    }
}

fn list(cfg: &Config, all: bool) -> Result<(), String> {
    let sessions = if all {
        session::all_sessions(cfg)
    } else {
        let root = crate::cursor::get_project_root();
        let s = session::all_for_project(&root, cfg);
        if s.is_empty() {
            eprintln!("No sessions found for: {root}");
            eprintln!("Try: oobo sessions list --all");
            return Ok(());
        }
        s
    };

    crate::tui::sessions::run_list(sessions, all)
}

fn show_json(id: &str) -> Result<(), String> {
    let s = session::find_session_any(id)?;
    let transcript_path = session::find_transcript_path(&s);
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

    println!("{}", serde_json::to_string_pretty(&obj).unwrap_or_default());
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
            let transcript_text = transcript_path
                .as_ref()
                .map(|p| session::read_transcript(p, 500, &s.source))
                .unwrap_or_default();

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

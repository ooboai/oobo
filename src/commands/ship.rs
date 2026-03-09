use crate::config::Config;
use crate::git::interceptor;
use crate::remote;
use crate::remote::payload::*;
use crate::tools::cursor;

/// Manually sync current project's AI context to the dashboard.
pub fn run(cfg: &Config) -> Result<(), String> {
    if cfg.server.api_key.is_empty() {
        return Err("not configured — run `oobo setup` first".into());
    }

    let project_root = cursor::get_project_root();
    let project_name = std::path::Path::new(&project_root)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();

    println!("Syncing AI context for: {project_root}");

    let tools =
        interceptor::collect_all_tool_context(cfg, &project_root, cfg.telemetry.send_transcripts);

    for (name, ctx) in &tools {
        println!("  {name}: {} sessions", ctx.active_sessions);
    }

    if tools.is_empty() {
        println!("No AI sessions found for this project.");
        return Ok(());
    }

    let payload = EventPayload {
        event: "sync".into(),
        timestamp: chrono::Utc::now(),
        project: ProjectInfo {
            root: project_root,
            name: project_name,
        },
        git: GitInfo {
            operation: "sync".into(),
            branch: String::new(),
            commit_hash: String::new(),
            commit_message: String::new(),
            author: String::new(),
            files_changed: 0,
            insertions: 0,
            deletions: 0,
        },
        tools,
    };

    remote::send_event(cfg, &payload);
    println!("Sync event sent.");
    Ok(())
}

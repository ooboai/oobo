use std::io::IsTerminal;

use crate::cli::OutputMode;
use crate::config::Config;
use crate::remote;
use crate::tools::cursor;

pub fn run(cfg: &Config, mode: OutputMode) {
    match mode {
        OutputMode::Agent => run_agent(cfg),
        OutputMode::Json => run_json(cfg),
        OutputMode::Tui => {
            if std::io::stdin().is_terminal() {
                if let Err(e) = crate::tui::dash::run(cfg) {
                    eprintln!("error: {e}");
                }
            } else {
                run_plain(cfg);
            }
        }
    }
}

struct DashData {
    tools_enabled: Vec<String>,
    projects: usize,
    sessions: i64,
    total_tokens: i64,
}

fn gather_dash(cfg: &Config) -> DashData {
    let mut tools_enabled = Vec::new();
    for (key, _) in TOOLS {
        if tool_enabled(cfg, key) {
            tools_enabled.push(key.to_string());
        }
    }

    let (projects, sessions, total_tokens) = if let Ok(db) = crate::db::Db::open() {
        let p = db.list_projects().map(|v| v.len()).unwrap_or(0);
        let (s, t) = db
            .aggregate_stats_global()
            .map(|a| {
                (
                    a.session_count,
                    a.total_input_tokens + a.total_output_tokens,
                )
            })
            .unwrap_or((0, 0));
        (p, s, t)
    } else {
        (0, 0, 0)
    };

    DashData {
        tools_enabled,
        projects,
        sessions,
        total_tokens,
    }
}

fn run_agent(cfg: &Config) {
    let d = gather_dash(cfg);

    println!("version: {}", env!("CARGO_PKG_VERSION"));
    println!("config: {}", crate::config::Config::config_path().display());
    println!("data: {}", crate::paths::oobo_home().display());
    println!("server: {}", cfg.server.url);
    println!("tools: {}", d.tools_enabled.join(", "));
    println!("projects: {}", d.projects);
    println!("sessions: {}", d.sessions);
    println!("tokens: {}", crate::tui::format_tokens(d.total_tokens));
}

fn run_json(cfg: &Config) {
    let d = gather_dash(cfg);

    let json = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "config_path": crate::config::Config::config_path().display().to_string(),
        "data_dir": crate::paths::oobo_home().display().to_string(),
        "server_url": cfg.server.url,
        "alias_enabled": cfg.git.alias_enabled,
        "tools_enabled": d.tools_enabled,
        "projects": d.projects,
        "sessions": d.sessions,
        "total_tokens": d.total_tokens,
    });
    crate::utils::print_json(&json);
}

const TOOLS: &[(&str, &str)] = &[
    ("cursor", "Cursor"),
    ("claude", "Claude"),
    ("gemini", "Gemini CLI"),
    ("windsurf", "Windsurf"),
    ("trae", "Trae"),
    ("aider", "Aider"),
    ("copilot", "Copilot"),
    ("zed", "Zed"),
    ("codex", "Codex"),
    ("opencode", "OpenCode"),
];

fn tool_enabled(cfg: &Config, key: &str) -> bool {
    match key {
        "cursor" => cfg.cursor.enabled,
        "claude" => cfg.claude.enabled,
        "gemini" => cfg.gemini.enabled,
        "windsurf" => cfg.windsurf.enabled,
        "trae" => cfg.trae.enabled,
        "aider" => cfg.aider.enabled,
        "copilot" => cfg.copilot.enabled,
        "zed" => cfg.zed.enabled,
        "codex" => cfg.codex.enabled,
        "opencode" => cfg.opencode.enabled,
        _ => false,
    }
}

fn run_plain(cfg: &Config) {
    println!("oobo v{}", env!("CARGO_PKG_VERSION"));
    println!();
    println!("Configuration:");
    println!("  Config file:    {}", Config::config_path().display());
    println!("  Data dir:       {}", crate::paths::oobo_home().display());
    println!("  Server URL:     {}", cfg.server.url);
    println!(
        "  API key:        {}",
        if cfg.server.api_key.is_empty() {
            "(not set)"
        } else {
            "••••••••"
        }
    );
    println!("  Git path:       {}", cfg.git_path());
    println!("  Alias enabled:  {}", cfg.git.alias_enabled);
    for (key, label) in TOOLS {
        let enabled = tool_enabled(cfg, key);
        println!("  {label:<14}  {enabled}");
    }
    println!("  Telemetry:      {}", cfg.telemetry.enabled);
    println!();

    if let Ok(db) = crate::db::Db::open() {
        if let Ok(projects) = db.list_projects() {
            println!("Local Index:");
            println!("  Projects:       {}", projects.len());
            if let Ok(agg) = db.aggregate_stats_global() {
                println!("  Sessions:       {}", agg.session_count);
                if agg.total_input_tokens + agg.total_output_tokens > 0 {
                    println!(
                        "  Total tokens:   {}",
                        crate::tui::format_tokens(agg.total_input_tokens + agg.total_output_tokens)
                    );
                }
            }
            println!();
        }
    }

    let root = cursor::get_project_root();

    for (key, label) in TOOLS {
        if !tool_enabled(cfg, key) {
            continue;
        }
        let count = tool_session_count(key, &root);
        println!("{label}:");
        println!("  Project root:   {root}");
        match count {
            Ok(n) => println!("  Sessions:       {n}"),
            Err(e) => println!("  Sessions:       error ({e})"),
        }
        println!();
    }

    if !cfg.server.api_key.is_empty() {
        print!("Server:           ");
        match remote::check_connection(cfg) {
            Ok(msg) => println!("{msg}"),
            Err(e) => println!("error ({e})"),
        }
    } else {
        println!("Server:           (not configured — run `oobo setup`)");
    }
}

fn tool_session_count(key: &str, root: &str) -> Result<usize, String> {
    match key {
        "cursor" => cursor::sessions_for_project(root).map(|s| s.len()),
        "claude" => crate::tools::claude::sessions_for_project(root).map(|s| s.len()),
        "gemini" => crate::tools::gemini::sessions_for_project(root).map(|s| s.len()),
        "windsurf" => crate::tools::windsurf::sessions_for_project(root).map(|s| s.len()),
        "trae" => crate::tools::trae::sessions_for_project(root).map(|s| s.len()),
        "aider" => crate::tools::aider::sessions_for_project(root).map(|s| s.len()),
        "copilot" => crate::tools::copilot::sessions_for_project(root).map(|s| s.len()),
        "zed" => crate::tools::zed::sessions_for_project(root).map(|s| s.len()),
        "codex" => crate::tools::codex::sessions_for_project(root).map(|s| s.len()),
        "opencode" => crate::tools::opencode::sessions_for_project(root).map(|s| s.len()),
        _ => Ok(0),
    }
}

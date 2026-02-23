use std::io::IsTerminal;

use crate::config::Config;
use crate::cursor;
use crate::server;

pub fn run(cfg: &Config) {
    if std::io::stdin().is_terminal() {
        if let Err(e) = crate::tui::dash::run(cfg) {
            eprintln!("error: {e}");
        }
    } else {
        run_plain(cfg);
    }
}

const TOOLS: &[(&str, &str)] = &[
    ("cursor", "Cursor"),
    ("claude", "Claude"),
    ("windsurf", "Windsurf"),
    ("trae", "Trae"),
    ("aider", "Aider"),
    ("continue", "Continue"),
    ("copilot", "Copilot"),
    ("zed", "Zed"),
    ("codex", "Codex"),
];

fn tool_enabled(cfg: &Config, key: &str) -> bool {
    match key {
        "cursor" => cfg.cursor.enabled,
        "claude" => cfg.claude.enabled,
        "windsurf" => cfg.windsurf.enabled,
        "trae" => cfg.trae.enabled,
        "aider" => cfg.aider.enabled,
        "continue" => cfg.continue_dev.enabled,
        "copilot" => cfg.copilot.enabled,
        "zed" => cfg.zed.enabled,
        "codex" => cfg.codex.enabled,
        _ => false,
    }
}

fn run_plain(cfg: &Config) {
    println!("oobo v{}", env!("CARGO_PKG_VERSION"));
    println!();
    println!("Configuration:");
    println!("  Config file:    {}", Config::config_path().display());
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
        match server::check_connection(cfg) {
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
        "claude" => crate::claude::sessions_for_project(root).map(|s| s.len()),
        "windsurf" => crate::windsurf::sessions_for_project(root).map(|s| s.len()),
        "trae" => crate::trae::sessions_for_project(root).map(|s| s.len()),
        "aider" => crate::aider::sessions_for_project(root).map(|s| s.len()),
        "continue" => crate::continue_dev::sessions_for_project(root).map(|s| s.len()),
        "copilot" => crate::copilot::sessions_for_project(root).map(|s| s.len()),
        "zed" => crate::zed::sessions_for_project(root).map(|s| s.len()),
        "codex" => crate::codex::sessions_for_project(root).map(|s| s.len()),
        _ => Ok(0),
    }
}

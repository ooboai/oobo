use std::io::IsTerminal;

use crate::config::Config;
use crate::tui::setup::ScanInfo;

pub fn run_setup() -> Result<(), String> {
    if let Err(e) = crate::commands::agent::ensure_skill_file() {
        eprintln!("oobo: warning: could not install skill file: {e}");
    }

    eprintln!("  scanning for AI tools...");
    let detected = detect_tools();
    let (projects, sessions) = run_initial_scan();

    if detected.is_empty() {
        eprintln!("  no existing sessions found — you can enable tools during setup");
    } else {
        let labels: Vec<String> = detected
            .iter()
            .map(|(key, count)| {
                let label = tool_label(key);
                format!("{label} ({count})")
            })
            .collect();
        eprintln!("  found: {}", labels.join(", "));
    }
    if projects > 0 {
        eprintln!("  {projects} project(s), {sessions} session(s)");
    }
    eprintln!();

    let cfg = Config::load_or_default();
    let scan = ScanInfo {
        detected,
        projects,
        sessions,
    };

    let new_cfg = if std::io::stdout().is_terminal() {
        match crate::tui::setup::run_setup_wizard(&cfg, scan)? {
            Some(c) => c,
            None => {
                println!();
                println!("  Setup cancelled.");
                println!();
                return Ok(());
            }
        }
    } else {
        eprintln!("  non-interactive environment detected — using defaults");
        crate::tui::setup::build_default_config(&cfg, scan)
    };

    new_cfg.save()?;

    println!();
    println!(
        "  Configuration saved to {}",
        Config::config_path().display()
    );

    if new_cfg.git.alias_enabled {
        if let Err(e) = crate::alias::install_alias() {
            eprintln!("oobo: warning: could not install alias: {e}");
        } else {
            println!("  Git alias installed (git = oobo)");
        }
    }

    println!("  Agent skill installed at ~/.agents/skills/oobo/");

    let hooks_installed = crate::hooks::install::install_all_agent_hooks();
    if !hooks_installed.is_empty() {
        println!();
        println!("  Agent lifecycle hooks installed:");
        for h in &hooks_installed {
            println!("    {h}");
        }
    }

    if let Some(root) = crate::git::proxy::project_root(&new_cfg) {
        match crate::hooks::install::install_project_hooks(&root) {
            Ok(hooks) => {
                println!();
                println!("  Git hooks installed for this project:");
                for h in &hooks {
                    println!("    {h}");
                }
            }
            Err(e) => {
                eprintln!("oobo: warning: could not install git hooks: {e}");
            }
        }
    }

    println!();
    println!("  You're all set! Try:");
    println!("    oobo             -- see your anchor feed");
    println!("    oobo anchors     -- enriched commit history");
    println!("    oobo search <q>  -- find any past session");
    println!();
    Ok(())
}

fn detect_tools() -> Vec<(String, usize)> {
    let reg = crate::tools::registry();
    let mut found = Vec::new();
    for tool in reg.all() {
        if let Ok(sessions) = tool.all_sessions() {
            if !sessions.is_empty() {
                found.push((tool.config_key().to_string(), sessions.len()));
            }
        }
    }
    found
}

fn run_initial_scan() -> (usize, usize) {
    let cfg = Config::load_or_default();
    match crate::db::Db::open() {
        Ok(db) => match crate::scanner::full_scan(&db, &cfg) {
            Ok(result) => (result.projects_found, result.sessions_found),
            Err(_) => (0, 0),
        },
        Err(_) => (0, 0),
    }
}

fn tool_label(key: &str) -> &str {
    let reg = crate::tools::registry();
    for tool in reg.all() {
        if tool.config_key() == key {
            return tool.display_name();
        }
    }
    key
}

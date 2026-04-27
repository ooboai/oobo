use std::collections::HashSet;
use std::io::IsTerminal;

use crate::cli::OutputMode;
use crate::config::Config;
use crate::tui::setup::{ProjectChoice, ScanInfo};

pub struct SetupOptions {
    pub non_interactive: bool,
    pub reindex: bool,
    pub uninstall_alias: bool,
    pub repair: bool,
    pub mode: OutputMode,
}

/// Top-level dispatcher for `oobo setup [flags]`. Flags are composable.
pub fn run_setup_with(opts: SetupOptions) -> Result<i32, String> {
    if opts.uninstall_alias && !opts.repair && !opts.reindex && !opts.non_interactive_wizard() {
        crate::alias::uninstall_alias()?;
        return Ok(0);
    }

    let mut exit_code = 0;

    if opts.repair {
        if let Err(e) = run_repair(&opts) {
            eprintln!("anchor: error: repair failed: {e}");
            exit_code = 1;
        }
    }

    if opts.reindex {
        match run_reindex(&opts) {
            Ok(code) if code != 0 => exit_code = code,
            Err(e) => {
                eprintln!("anchor: error: reindex failed: {e}");
                exit_code = 1;
            }
            _ => {}
        }
    }

    if opts.uninstall_alias {
        if let Err(e) = crate::alias::uninstall_alias() {
            eprintln!("anchor: warning: could not uninstall alias: {e}");
        }
    }

    if !opts.repair && !opts.reindex && !opts.uninstall_alias {
        run_setup(opts.non_interactive)?;
    }

    Ok(exit_code)
}

impl SetupOptions {
    fn non_interactive_wizard(&self) -> bool {
        self.non_interactive
    }
}

// ── Repair path ────────────────────────────────────────────────────────────

fn run_repair(opts: &SetupOptions) -> Result<(), String> {
    let cfg = Config::load_or_default();
    let mode = opts.mode;

    let root = crate::git::proxy::project_root(&cfg);
    let Some(path) = root else {
        if matches!(mode, OutputMode::Tui) {
            println!("not in a git repository — cd into a project first.");
        }
        return Ok(());
    };

    let project_id = crate::project::id_for_root(&path);
    let name = std::path::Path::new(&path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");

    let hooks_result = crate::hooks::install::install_project_hooks(&path);
    let hooks_status = match &hooks_result {
        Ok(_) => "ok",
        Err(_) => "failed",
    };

    let orphan_status = if crate::git::orphan::branch_exists(&path) {
        "ok"
    } else if crate::git::orphan::remote_branch_exists(&path) {
        match crate::git::orphan::fetch_and_reconcile(&path) {
            Ok(_) => "rebuilt from remote",
            Err(_) => "missing",
        }
    } else {
        "missing (no remote branch)"
    };

    match mode {
        OutputMode::Agent => {
            println!(
                "repair {} hooks={} orphan={}",
                name, hooks_status, orphan_status
            );
        }
        OutputMode::Json => {
            let json = serde_json::json!({
                "project": { "id": project_id, "name": name, "path": path },
                "hooks": hooks_status,
                "orphan": orphan_status,
            });
            crate::utils::print_json(&json);
        }
        OutputMode::Tui => {
            println!(
                "  {:<20} hooks {} · orphan {}",
                name, hooks_status, orphan_status
            );
        }
    }

    if matches!(mode, OutputMode::Tui) {
        println!();
        println!("repair complete.");
    }
    Ok(())
}

// ── Reindex path ───────────────────────────────────────────────────────────

fn run_reindex(opts: &SetupOptions) -> Result<i32, String> {
    let mode = opts.mode;
    if matches!(mode, OutputMode::Tui) {
        println!("reindex is no longer needed — anchor data lives on the orphan branch.");
    }
    Ok(0)
}

pub fn run_setup(non_interactive: bool) -> Result<(), String> {
    if let Err(e) = crate::commands::agent::ensure_skill_file() {
        eprintln!("anchor: warning: could not install skill file: {e}");
    }

    eprintln!("  scanning for AI tools and projects...");
    let scan = run_initial_scan();

    if scan.detected.is_empty() {
        eprintln!("  no existing sessions found — you can enable tools during setup");
    } else {
        let labels: Vec<String> = scan
            .detected
            .iter()
            .map(|(key, count)| {
                let label = tool_label(key);
                format!("{label} ({count})")
            })
            .collect();
        eprintln!("  found: {}", labels.join(", "));
    }
    if scan.projects > 0 {
        eprintln!(
            "  {} project(s), {} session(s)",
            scan.projects, scan.sessions
        );
    }
    eprintln!();

    let cfg = Config::load_or_default();

    let outcome = if !non_interactive && std::io::stdout().is_terminal() {
        match crate::tui::setup::run_setup_wizard(&cfg, scan)? {
            Some(outcome) => outcome,
            None => {
                println!();
                println!("  Setup cancelled.");
                println!();
                return Ok(());
            }
        }
    } else {
        eprintln!("  non-interactive environment detected — using defaults");
        crate::tui::setup::build_default_outcome(&cfg, scan)
    };

    let new_cfg = outcome.config.clone();
    new_cfg.save()?;

    let (enabled_projects, disabled_projects) = apply_project_choices(&outcome.projects)?;

    println!();
    println!(
        "  Configuration saved to {}",
        Config::config_path().display()
    );
    if !outcome.projects.is_empty() {
        println!("  Projects enabled: {enabled_projects}");
        println!("  Projects disabled: {disabled_projects}");
    }

    if new_cfg.git.alias_enabled {
        if let Err(e) = crate::alias::install_alias() {
            eprintln!("anchor: warning: could not install alias: {e}");
        } else {
            println!("  Git alias installed (git = anchor)");
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

    install_selected_project_hooks(&new_cfg, &outcome.projects);

    println!();
    println!("  You're all set! Try:");
    println!("    anchor             -- see your anchor feed");
    println!("    anchor anchors     -- enriched commit history");
    println!("    anchor search <q>  -- find any past session");
    println!("    anchor enable      -- enable the current repo later");
    println!("    anchor disable     -- make anchor stay quiet in this repo");
    println!();
    Ok(())
}

fn run_initial_scan() -> ScanInfo {
    let reg = crate::tools::registry();
    let mut detected: Vec<(String, usize)> = Vec::new();
    let mut project_map: std::collections::HashMap<String, (HashSet<String>, usize)> =
        std::collections::HashMap::new();

    for tool in reg.all() {
        let sessions = match tool.all_sessions() {
            Ok(s) => s,
            Err(_) => continue,
        };
        let top_level_count = sessions.iter().filter(|s| !s.is_subagent()).count();
        if top_level_count > 0 {
            detected.push((tool.config_key().to_string(), top_level_count));
        }
        for session in &sessions {
            if session.project_path.is_empty() || session.is_subagent() {
                continue;
            }
            let entry = project_map
                .entry(session.project_path.clone())
                .or_insert_with(|| (HashSet::new(), 0));
            entry.0.insert(tool.config_key().to_string());
            entry.1 += 1;
        }
    }

    let total_projects = project_map.len();
    let total_sessions: usize = project_map.values().map(|(_, c)| c).sum();

    let mut project_choices: Vec<ProjectChoice> = project_map
        .into_iter()
        .filter_map(|(path, (tools, session_count))| {
            let name = std::path::Path::new(&path)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();
            let id = crate::project::id_for_root(&path);
            let already_enabled = crate::project_config::is_enabled(&path);
            let has_sessions = session_count > 0;
            Some(ProjectChoice {
                id,
                name,
                path,
                tools: tools.into_iter().collect(),
                sessions: session_count,
                enabled: already_enabled || has_sessions,
            })
        })
        .collect();

    project_choices.sort_by(|a, b| b.sessions.cmp(&a.sessions));

    ScanInfo {
        detected,
        projects: total_projects,
        sessions: total_sessions,
        project_choices,
    }
}

fn apply_project_choices(projects: &[ProjectChoice]) -> Result<(usize, usize), String> {
    let mut enabled = 0usize;
    let mut disabled = 0usize;
    for project in projects {
        if project.enabled {
            crate::project_config::set_enabled(&project.path, &project.id, true)?;
            enabled += 1;
        } else {
            if crate::project_config::exists(&project.path) {
                crate::project_config::set_enabled(&project.path, &project.id, false)?;
            }
            disabled += 1;
        }
    }
    Ok((enabled, disabled))
}

fn install_selected_project_hooks(cfg: &Config, projects: &[ProjectChoice]) {
    if projects.is_empty() {
        if let Some(root) = crate::git::proxy::project_root(cfg) {
            install_project_hooks_with_status(&root, "this project");
        }
        return;
    }

    let enabled: Vec<&ProjectChoice> = projects.iter().filter(|p| p.enabled).collect();
    if enabled.is_empty() {
        println!();
        println!("  No project git hooks installed (all projects disabled).");
        return;
    }

    println!();
    println!("  Git hooks installed for enabled projects:");
    for project in enabled {
        install_project_hooks_with_status(&project.path, &project.name);
    }
}

fn install_project_hooks_with_status(root: &str, label: &str) {
    match crate::hooks::install::install_project_hooks(root) {
        Ok(hooks) => {
            if hooks.is_empty() {
                println!("    {label}: already up to date");
            } else {
                println!("    {label}: {}", hooks.join(", "));
            }
        }
        Err(e) => {
            eprintln!("anchor: warning: could not install git hooks for {label}: {e}");
        }
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

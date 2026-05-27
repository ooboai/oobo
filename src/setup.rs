use std::collections::HashSet;
use std::io::IsTerminal;

use crate::cli::OutputMode;
use crate::config::Config;
use crate::error::{CliError, CmdResult};
use crate::tui::setup::{ProjectChoice, ScanInfo};

pub struct SetupOptions {
    pub non_interactive: bool,
    pub reindex: bool,
    pub repair: bool,
    pub mode: OutputMode,
}

/// Top-level dispatcher for `oobo setup [flags]`. Flags are composable.
pub fn run_setup_with(opts: &SetupOptions) -> CmdResult {
    if opts.repair {
        run_repair(opts);
    }

    if opts.reindex {
        run_reindex(opts);
    }

    if !opts.repair && !opts.reindex {
        run_setup(opts.non_interactive)?;
    }

    Ok(0)
}

// ── Repair path ────────────────────────────────────────────────────────────

fn run_repair(opts: &SetupOptions) {
    let cfg = Config::load_or_default();
    let mode = opts.mode;

    let root = crate::git::proxy::project_root(&cfg);
    let Some(path) = root else {
        eprintln!("oobo: not inside a git repository.");
        return;
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
            Ok(()) => "rebuilt from remote",
            Err(_) => "missing",
        }
    } else {
        "missing (no remote branch)"
    };

    match mode {
        OutputMode::Agent => {
            println!("repair {name} hooks={hooks_status} orphan={orphan_status}");
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
            println!("  {name:<20} hooks {hooks_status} · orphan {orphan_status}");
        }
    }

    if matches!(mode, OutputMode::Tui) {
        println!();
        println!("repair complete.");
    }
}

// ── Reindex path ───────────────────────────────────────────────────────────

fn run_reindex(opts: &SetupOptions) {
    let mode = opts.mode;
    if matches!(mode, OutputMode::Tui) {
        println!("reindex is no longer needed  --  oobo data lives on the orphan branch.");
    }
}

pub fn run_setup(non_interactive: bool) -> Result<(), CliError> {
    if let Err(e) = crate::commands::agent::ensure_skill_file() {
        eprintln!("oobo: warning: could not install skill file: {e}");
    }

    eprintln!("  scanning for AI tools and projects...");
    let scan = run_initial_scan();

    if scan.detected.is_empty() {
        eprintln!("  no existing sessions found  --  you can enable tools during setup");
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

    let can_interactive = std::io::stdout().is_terminal()
        && (std::io::stdin().is_terminal() || std::path::Path::new("/dev/tty").exists());
    let outcome = if !non_interactive && can_interactive {
        if let Some(outcome) = crate::tui::setup::run_setup_wizard(&cfg, scan)? {
            outcome
        } else {
            println!();
            println!("  Setup cancelled.");
            println!();
            return Ok(());
        }
    } else {
        eprintln!("  non-interactive environment detected  --  using defaults");
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
        let unchanged: usize = outcome
            .projects
            .iter()
            .filter(|p| p.state == crate::tui::setup::ProjectState::Unchanged)
            .count();
        if enabled_projects > 0 || disabled_projects > 0 {
            println!("  Projects enabled: {enabled_projects}, disabled: {disabled_projects}");
        }
        if unchanged > 0 {
            println!("  Projects unchanged: {unchanged}");
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
    println!("    oobo               -- see your memory feed");
    println!("    oobo anchor show <sha>  -- drill into a commit");
    println!("    oobo recall <q>    -- find any past session");
    println!("    oobo enable        -- enable the current repo later");
    println!("    oobo disable       -- make oobo stay quiet in this repo");
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
        .filter(|(path, _)| {
            let p = std::path::Path::new(path);
            p.is_absolute() && p.parent().is_some() && path != "/"
        })
        .map(|(path, (tools, session_count))| {
            let name = std::path::Path::new(&path)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();
            let id = crate::project::id_for_root(&path);
            let had_config = crate::project_config::exists(&path);
            ProjectChoice {
                id,
                name,
                path,
                tools: tools.into_iter().collect(),
                sessions: session_count,
                state: crate::tui::setup::ProjectState::Disabled,
                had_config,
            }
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

fn apply_project_choices(projects: &[ProjectChoice]) -> Result<(usize, usize), CliError> {
    use crate::tui::setup::ProjectState;
    let mut enabled = 0usize;
    let mut disabled = 0usize;
    for project in projects {
        if project.path.is_empty() || project.path == "/" {
            continue;
        }
        match project.state {
            ProjectState::Enabled => {
                crate::project_config::set_enabled(&project.path, &project.id, true)?;
                enabled += 1;
            }
            ProjectState::Disabled => {
                if crate::project_config::exists(&project.path) {
                    crate::project_config::set_enabled(&project.path, &project.id, false)?;
                }
                disabled += 1;
            }
            ProjectState::Unchanged => {
                // Don't touch the project config at all
            }
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

    use crate::tui::setup::ProjectState;
    let active: Vec<&ProjectChoice> = projects
        .iter()
        .filter(|p| match p.state {
            ProjectState::Enabled => true,
            ProjectState::Unchanged => crate::project_config::is_enabled(&p.path),
            ProjectState::Disabled => false,
        })
        .collect();
    if active.is_empty() {
        println!();
        println!("  No project git hooks installed (all projects disabled).");
        return;
    }

    println!();
    println!("  Git hooks refreshed for active projects:");
    for project in active {
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
            eprintln!("oobo: warning: could not install git hooks for {label}: {e}");
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

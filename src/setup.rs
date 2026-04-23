use std::io::IsTerminal;

use crate::cli::OutputMode;
use crate::config::Config;
use crate::tui::setup::ScanInfo;

pub struct SetupOptions {
    pub non_interactive: bool,
    pub reindex: bool,
    pub uninstall_alias: bool,
    pub repair: bool,
    pub mode: OutputMode,
}

/// Top-level dispatcher for `oobo setup [flags]`. Flags are composable.
pub fn run_setup_with(opts: SetupOptions) -> Result<i32, String> {
    // `--uninstall-alias` is headless. If combined with other flags we still
    // run those; but on its own it should short-circuit like `oobo alias uninstall`.
    if opts.uninstall_alias && !opts.repair && !opts.reindex && !opts.non_interactive_wizard() {
        crate::alias::uninstall_alias()?;
        return Ok(0);
    }

    let mut exit_code = 0;

    if opts.repair {
        if let Err(e) = run_repair(&opts) {
            eprintln!("oobo: error: repair failed: {e}");
            exit_code = 1;
        }
    }

    if opts.reindex {
        match run_reindex(&opts) {
            Ok(code) if code != 0 => exit_code = code,
            Err(e) => {
                eprintln!("oobo: error: reindex failed: {e}");
                exit_code = 1;
            }
            _ => {}
        }
    }

    if opts.uninstall_alias {
        if let Err(e) = crate::alias::uninstall_alias() {
            eprintln!("oobo: warning: could not uninstall alias: {e}");
        }
    }

    // Run the wizard only when no composable flag was supplied, or when the
    // user explicitly asked for `--non-interactive` without anything else.
    if !opts.repair && !opts.reindex && !opts.uninstall_alias {
        run_setup()?;
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
    let db = crate::db::Db::open()?;
    let projects = db.list_projects().unwrap_or_default();
    let mut healthy = 0usize;

    let mode = opts.mode;
    if matches!(mode, OutputMode::Tui) {
        println!("repairing {} projects...", projects.len());
    }

    for p in &projects {
        let settings = db.get_project_settings(&p.id).unwrap_or_default();
        if settings.ignored {
            continue;
        }

        let hooks_result = crate::hooks::install::install_project_hooks(&p.path);
        let hooks_status = match &hooks_result {
            Ok(_) => "ok",
            Err(_) => "failed",
        };

        // Orphan branch check is best-effort.
        let orphan_status = if crate::git::orphan::branch_exists(&p.path) {
            "ok"
        } else {
            // Try to recreate if non-interactive OR non-TTY.
            if opts.non_interactive || !std::io::stdout().is_terminal() {
                match crate::git::orphan::fetch_and_reconcile(&p.path) {
                    Ok(_) => "rebuilt",
                    Err(_) => "missing",
                }
            } else {
                "missing (run again with --non-interactive to rebuild)"
            }
        };

        match mode {
            OutputMode::Agent => {
                println!("repair {} hooks={} orphan={}", p.name, hooks_status, orphan_status);
            }
            OutputMode::Json => {
                let json = serde_json::json!({
                    "project": { "id": p.id, "name": p.name, "path": p.path },
                    "hooks": hooks_status,
                    "orphan": orphan_status,
                });
                crate::utils::print_json(&json);
            }
            OutputMode::Tui => {
                println!(
                    "  {:<20} hooks {} · orphan {}",
                    p.name, hooks_status, orphan_status
                );
            }
        }

        if matches!(hooks_status, "ok") && matches!(orphan_status, "ok" | "rebuilt") {
            healthy += 1;
        }
    }

    let _ = cfg;
    if matches!(mode, OutputMode::Tui) {
        println!();
        println!("{} projects healthy.", healthy);
    }
    Ok(())
}

// ── Reindex path ───────────────────────────────────────────────────────────

fn run_reindex(opts: &SetupOptions) -> Result<i32, String> {
    let cfg = Config::load_or_default();
    let db = crate::db::Db::open()?;
    let projects = db.list_projects().unwrap_or_default();
    let mode = opts.mode;

    if projects.is_empty() {
        if matches!(mode, OutputMode::Tui) {
            println!("no projects to reindex. run `oobo setup` first.");
        }
        return Ok(0);
    }

    if matches!(mode, OutputMode::Tui) {
        println!("reindexing {} projects...", projects.len());
    }

    let mut failures = 0usize;
    for p in &projects {
        let settings = db.get_project_settings(&p.id).unwrap_or_default();
        if settings.ignored {
            eprintln!("oobo: skipping disabled project '{}'", p.name);
            continue;
        }
        let start = std::time::Instant::now();
        let result = crate::scanner::scan_project(&db, &cfg, &p.path);
        let elapsed = start.elapsed();

        match (&result, mode) {
            (Ok(r), OutputMode::Agent) => {
                println!(
                    "reindex {} {} {}ms ok",
                    p.name,
                    r.sessions_found,
                    elapsed.as_millis()
                );
            }
            (Ok(r), OutputMode::Json) => {
                let json = serde_json::json!({
                    "project": { "id": p.id, "name": p.name },
                    "sessions": r.sessions_found,
                    "elapsed_ms": elapsed.as_millis(),
                    "status": "ok",
                });
                crate::utils::print_json(&json);
            }
            (Ok(r), OutputMode::Tui) => {
                println!(
                    "  ✓ {:<20} {} sessions  {:.1}s",
                    p.name,
                    r.sessions_found,
                    elapsed.as_secs_f64()
                );
            }
            (Err(e), _) => {
                failures += 1;
                eprintln!("  ✗ {}: {}", p.name, e);
            }
        }
    }

    if matches!(mode, OutputMode::Tui) {
        println!();
        println!(
            "done. {} projects reindexed ({} failures).",
            projects.len() - failures,
            failures
        );
    }
    if failures > 0 {
        Ok(1)
    } else {
        Ok(0)
    }
}

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
